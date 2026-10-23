//! Entities placed into a world: each Entity's (or EntityGroup's own) Visuals resolved through an [`Archive`] to the
//! meshes of their LODSelector (or a Mesh directly), converted through the program's stated frame. Nothing here
//! chooses a level of detail, applies a material override or selects by DataLayer: placements keep their overrides
//! per level and their Entity's DataLayerActions as stored.
//!
//! A LODSelectorInstance's five slots line up one to one with its LODSelector's levels (the Riders Republic notes:
//! 2,839 of 2,839), a slot holding a MeshInstanceData where its level is a Mesh. What a Visual places depends on
//! where its instance matrices are:
//!
//! - In no slot: the object's every mesh level, at the owner's T R s (column-major matrix, rotation R, translation
//!   T, scale s): one placement of a prototype whose own [`AnvilMesh`] is the finest mesh level and whose
//!   [`AnvilLods`] holds the others.
//! - In one slot: once per instance matrix I, at T + R (I v) (the instances in the owner's frame: per-instance bounds
//!   contain the instance's mesh bounds under R I on 38,839 of 38,877 group instances). Whether s applies to
//!   instances isn't established (one family of 32 fits without it; the census finds no instanced Entity with s
//!   other than 1), so an instanced owner with s other than 1 is refused. In the six Riders Republic forges the
//!   matrices are in slot 0 and every other mesh slot holds none (the census: 349,009 instanced Visuals of the 576,844
//!   Entities read). What each instance places is the caller's stated [`InstancedLevels`].
//! - In more than one slot, in the LODSelectorInstance itself or in an ImpostorInstanceData: not seen, refused.
//!
//! Levels referring to an Impostor are counted, not placed. An Entity or EntityGroup whose `IsHidden` or `IsPhantom`
//! is set isn't placed at all ([`Placed::hidden_or_phantom`]): on Riders Republic those are UI, HUD and editor helpers
//! (docs/placement.md §19.1). A Visual whose MeshInstanceData is a fake (`IsFake`: the far terrain's cells) places as
//! any other, each placement marked [`AnvilFake`] for the program to leave out or keep.
//!
//! Collision: each RigidBodyComponent's shape (the reference at its offset 748) is placed once at its owner's T R, as
//! a prototype holding the shape's collision in the owner's local frame; an EntityGroup's (a merged shape spanning
//! its instances, the notes find) likewise. MeshShapes, SphereShapes, CapsuleShapes and BoxShapes (at their own
//! transform) are read, a ReferenceListShape placing each child at its transform in the list; another type is counted
//! ([`Placed::shapes_unread`]), and so are the RigidBodies of owners with a scale other than 1, whose placement isn't
//! established ([`Placed::scaled_collision`]). An instanced Entity's is placed at each instance or not, as the caller
//! states ([`InstancedCollision`]). Each collision placement carries its RigidBody's filter ([`AnvilCollisionFilter`]:
//! the registry's `QueryFlags`, `CollisionLayerBits` and `IsGround`); a RigidBody whose `QueryFlags` set `WaterLevel`
//! and not `Static` is a water surface (counted, [`Placed::water_bodies`]), placed as the others are: which collision
//! a destination takes is `select-anvil-entities`'.

use std::collections::{BTreeMap, HashMap, HashSet};

use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Name, Transform, World};

use crate::components::{
    AnvilBounds, AnvilCollisionFilter, AnvilCollisionOnly, AnvilCollisionShape, AnvilDataLayers,
    AnvilDecalModel, AnvilDeferredDecals, AnvilEntityState, AnvilFake, AnvilLodDistances, AnvilLods,
    AnvilMaterial, AnvilMaterialOverrides, AnvilMesh, AnvilWater, BoundTexture, CollisionMaterialRef,
    DataLayerAction, DeferredDecal, MaterialBinding, MaterialValue, ShapeGeometry, TextureFrom,
};
use crate::read::archive::{Archive, At};
use crate::read::ddecal::{
    DDECAL_MODEL, HOUDINI_DECAL_INSTANCER, STATIC_DDECAL, StoredDecalModel, StoredDeferredDecals,
};
use crate::read::entity::{
    CollisionFilter, ComponentBody, ENTITY, ENTITY_GROUP, Instance as Shown, MeshInstance, Slot,
    StoredEntity, StoredEntityGroup, Visual, query,
};
use crate::read::lod::{LOD_SELECTOR, StoredLodSelector};
use crate::read::material::{Payload, StoredMaterial, StoredTextureSet};
use crate::read::mesh::{MATERIAL, MESH, StoredMesh, max_culling_distance};
use crate::read::mesh_shape::{MESH_SHAPE, MeshShape};
use crate::read::meshes::anvil_mesh;
use crate::read::shape::{
    BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE, StoredBox, StoredCapsule,
    StoredReferenceList, StoredSphere,
};
use crate::read::shapes::anvil_mesh_shape;
use crate::read::template::{self, BindingKind};
use crate::read::textures;
use crate::read::water::{StoredWater, WATER_COMPONENT};

/// Why an Entity can't be placed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] crate::read::archive::Error),
    #[error("{at:?}: type {found:#010x}, not {what}")]
    Type { at: At, found: u32, what: &'static str },
    #[error("{at:?}: {source}")]
    Entity {
        at: At,
        #[source]
        source: crate::read::entity::Error,
    },
    #[error("{at:?}: {source}")]
    Lod {
        at: At,
        #[source]
        source: crate::read::lod::Error,
    },
    #[error("{at:?}: {source}")]
    Mesh {
        at: At,
        #[source]
        source: crate::read::mesh::Error,
    },
    #[error(transparent)]
    Meshes(#[from] crate::read::meshes::Error),
    #[error("{0:?}: instanced with scale {1}: whether the scale applies to its instances isn't established")]
    InstancedScale(At, f32),
    #[error("{0:?}: instance matrices {1}: not seen")]
    Instances(At, &'static str),
    #[error("{0:?}: slot {1} holds a MeshInstanceData but its object's level {1} isn't a Mesh")]
    Slot(At, u8),
    #[error("{at:?}: {source}")]
    Shape {
        at: At,
        #[source]
        source: crate::read::mesh_shape::Error,
    },
    #[error(transparent)]
    Shapes(#[from] crate::read::shapes::Error),
    #[error("{at:?}: {source}")]
    Material {
        at: At,
        #[source]
        source: crate::read::material::MaterialError,
    },
    #[error("{at:?}: {source}")]
    Bindings {
        at: At,
        #[source]
        source: crate::read::template::BindingError,
    },
    #[error("{at:?}: {source}")]
    Permutations {
        at: At,
        #[source]
        source: crate::read::template::PermutationError,
    },
    #[error("{at:?}: {source}")]
    Features {
        at: At,
        #[source]
        source: crate::read::template::FeatureError,
    },
    #[error(transparent)]
    TextureMap(#[from] crate::read::textures::Error),
    #[error("{at:?}: {source}")]
    TextureSet {
        at: At,
        #[source]
        source: crate::read::material::TextureSetError,
    },
    #[error("{at:?}: {source}")]
    ShapeBody {
        at: At,
        #[source]
        source: crate::read::shape::Error,
    },
}

/// What an instanced Visual places at each instance: the caller's stated choice, as whether the game draws an
/// object's other levels at its instances isn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstancedLevels {
    /// The level whose slot holds the matrices; the object's other mesh levels aren't placed
    /// ([`Placed::levels_unplaced`]).
    Slot,
    /// Every mesh level from the slot's on, as for a Visual without instances. The levels a LODSelector's dependencies
    /// fill before it aren't placed at the instances ([`Placed::dependency_levels_unplaced`]): no slot holds them (on
    /// each of the 45,805 instanced Visuals of Riders Republic whose selector has dependencies, their slots are none
    /// and the matrices are in the first referenced level's: the census). The census measures what each instance's stored
    /// bounds cover: on an Entity, its matrix (then the Entity's rotation) applied to the LODSelectorInstance's
    /// bounds, within 1e-3 on all 6,422,618 (2 bit-equal), those bounds being the union of every mesh level's stored
    /// bounds (bit-equal on 349,004 of 349,009 instanced Visuals, the finest level's alone on 45,716); on an
    /// EntityGroup (whose LODSelectorInstance bounds are unset), R I applied to that union is contained in them on
    /// 241,706 of 241,747 (within 1e-3) and equal to them within 1e-3 on 84,666, against 50,573 for the finest
    /// level's. So each instance's bounds cover every level; that the game draws them there isn't read.
    Selector,
    /// Every mesh level, as for a Visual without instances: as [`InstancedLevels::Selector`], and the levels a
    /// LODSelector's dependencies fill before the slot's too (its `_LOD0`, `_LOD1` meshes, which no slot holds), the
    /// finest of them the prototype's own mesh. Nothing in an instance names those levels; what supports placing them
    /// there is the instances' stored bounds. Where the dependencies' meshes widen the union of the selector's
    /// referenced levels' bounds (115,136 EntityGroup instances of the six Riders Republic forges), an instance's
    /// stored bounds follow the union with the dependencies on 94,159 of them and the union without on 3,800 (17,177
    /// as near both), and lie within 1e-3 of the former on 58,544 against 13,572 of the latter (the census,
    /// `instanced bounds where the dependencies widen the levels' union`, worlds/anvil/tests/census.rs). On Entities
    /// the dependencies never widen that union, so their bounds don't tell the two apart (the census prints no
    /// Entity row). That the game draws the dependencies' meshes at the instances isn't read: it is the reading that
    /// the stored bounds are the instance's whole selector's, the one the game's own picture agrees with (the thin
    /// poles of Area 52's towers, which the slot's level alone draws as flat triangles).
    Every,
}

/// What an instanced Entity's RigidBody places: the caller's stated choice, as how its one shape stands for the
/// instances isn't established (the Riders Republic notes: 5,076 of 5,087 capsule owners are instanced, one capsule
/// for their N instances).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstancedCollision {
    /// Nothing; counted in [`Placed::instanced_collision`].
    Skip,
    /// The shape at each instance, at T + R (I v) as the instance's meshes are, I's own scale with it (whether the
    /// game scales the shape so isn't read either). On the first 400 instanced, unscaled owners of a RigidBody in
    /// DataPC_MAD_USNAT, 17,948 of the 19,637 shapes so placed lie within their instance's stored bounds (of its
    /// meshes) to 1 cm, and all within 0.87 m.
    PerInstance,
}

/// What placing the Entities did, beyond the placements.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub placements: Vec<Entity>,
    /// Visuals whose object resolves to nothing the reader places (a SplashFX, an id in no forge).
    pub visuals_skipped: u64,
    /// LODSelector levels referring to an Impostor (not placed), once per LODSelector.
    pub impostor_levels: u64,
    /// Mesh levels of instanced Visuals not placed at their instances (see [`InstancedLevels`]), once per Visual.
    pub levels_unplaced: u64,
    /// Mesh levels a LODSelector's dependencies fill that an instanced Visual doesn't place at its instances (no slot
    /// holds them; see [`InstancedLevels::Selector`]; under [`InstancedLevels::Every`] they are placed), once per
    /// Visual.
    pub dependency_levels_unplaced: u64,
    /// The member references of the EntityGroups placed, in order. Members aren't placed through their group: each is
    /// an Entity or EntityGroup of its own, which the caller places where it lists it.
    pub members: Vec<u64>,
    /// The collision placements: each RigidBodyComponent's shape, at its owner's T R s.
    pub collisions: Vec<Entity>,
    /// Objects (meshes, LODSelectors, collision shapes) that couldn't be read, by ClassID, and why, once each: a mesh
    /// level left out of its object, an object's Visuals or a shape's RigidBodies left out.
    pub refused: Vec<(u64, String)>,
    /// RigidBodyComponents whose shape is of a type this reader doesn't read, by the type (or 0: an id in no forge).
    pub shapes_unread: BTreeMap<u32, u64>,
    /// RigidBodyComponents of instanced Entities not placed (under [`InstancedCollision::Skip`]).
    pub instanced_collision: u64,
    /// Placements of StaticDecalComponents: decal geometry made for its one placement, placed as a Visual is (its
    /// LODSelector at the owner's matrix: docs/entity-grammar.md §9.1), its Materials drawn as their prelude states.
    pub static_decals: u64,
    /// RigidBodyComponents of owners with a scale other than 1: whether it applies to the shape isn't established
    /// (the notes: of 298 scaled owners of ReferenceList or Sphere shapes, 154 fit only unscaled, 67 only scaled), so
    /// not placed.
    pub scaled_collision: u64,
    /// The lakes: an entity each, holding its WaterComponent's [`AnvilWater`].
    pub lakes: Vec<Entity>,
    /// WaterComponents whose body can't be read as a lake's (recorded in `refused` too, by the owner's ClassID).
    pub lakes_unread: u64,
    /// The deferred decals: an entity per DDecalStaticInstancers item, holding its [`AnvilDeferredDecals`].
    pub deferred_decals: Vec<Entity>,
    /// HoudiniDecalInstancerComponents whose body can't be read (recorded in `refused` too).
    pub deferred_decals_unread: u64,
    /// Entities and EntityGroups whose `IsHidden` or `IsPhantom` is set (UI, HUD and editor helpers, docs/placement.md
    /// §19.1): placed as the others are, each of their placements marked [`AnvilEntityState`]. A group's members
    /// have flags of their own.
    pub hidden_or_phantom: u64,
    /// Placements of fake MeshInstanceData, marked [`AnvilFake`].
    pub fakes: u64,
    /// RigidBodies that are water surfaces (`QueryFlags` `WaterLevel` set and `Static` clear: on Riders Republic the
    /// river surfaces, docs/placement.md §19.2), placed with their [`AnvilCollisionFilter`] as every collision is.
    pub water_bodies: u64,
    /// RigidBodies placed whose `QueryFlags` `Static` is clear (and `WaterLevel` too), each marked as every collision
    /// placement is ([`AnvilCollisionFilter`]).
    pub not_static: u64,
    /// Collision placements of owners that place no visual, each marked [`AnvilCollisionOnly`] (on Riders Republic the
    /// `MAD_COL_*` blockers); `select-anvil-entities collision-only=` keeps, drops or tests them.
    pub collision_only: u64,
    /// The components kept as stored bytes ([`ComponentBody::Unread`]), by type, once per owner placed: gameplay, UI,
    /// sound and trigger components nothing here reads. A Visual or RigidBodyComponent among them is not placed.
    pub unread_components: BTreeMap<u32, u64>,
}

/// The column-major 4x4 `m` as a transform, its linear part times `s`.
fn transform(m: &[f32; 16], s: f32) -> Transform {
    let col = |c: usize| [0, 1, 2].map(|r| m.get(c * 4 + r).copied().unwrap_or(0.0));
    let [x, y, z, t] = [col(0), col(1), col(2), col(3)];
    Transform {
        cols: [x.map(|v| v * s), y.map(|v| v * s), z.map(|v| v * s), t],
    }
}

/// `outer` after `inner` (`world = outer (inner v)`), computed in f64 and rounded once.
fn compose(outer: &Transform, inner: &Transform) -> Transform {
    let o = outer.cols.map(|c| c.map(f64::from));
    let i = inner.cols.map(|c| c.map(f64::from));
    let apply = |v: [f64; 3]| [0, 1, 2].map(|r| o[0][r] * v[0] + o[1][r] * v[1] + o[2][r] * v[2]);
    let lin = |c: usize| apply(i[c]).map(|x| x as f32);
    let t = apply(i[3]);
    Transform {
        cols: [lin(0), lin(1), lin(2), [0, 1, 2].map(|r| (t[r] + o[3][r]) as f32)],
    }
}

/// A generic `Texture` binding's key: the CRC-32 of its label.
const GENERIC_TEXTURE: u32 = 0x4DDB_34EE;

/// The name of the entry `id` names in `archive`, if any.
fn entry_name(archive: &Archive, id: u64) -> Option<String> {
    let at = archive.find(id)?;
    Some(archive.forge(at.forge)?.names.get(at.entry)?.name.clone())
}

/// A shape's parts: each a collision prototype and its transform within the shape's owner, if any.
type Parts = Vec<(Entity, Option<Transform>)>;

/// An object's mesh levels, each `(level, the entity holding its AnvilMesh)`, finest first (a Mesh: its level 0).
type Levels = Vec<(u8, Entity)>;

/// A Visual's slots as placing reads them.
struct Shows {
    /// The slot holding instance matrices, and them.
    instances: Option<(u8, Vec<[f32; 16]>)>,
    /// Each mesh slot's material-info entries as (source, replacement) pairs, where it holds any.
    overrides: Vec<(u8, Vec<(u64, u64)>)>,
    /// Its mesh slots.
    mesh_slots: u64,
    /// A fake MeshInstanceData's mark, where one of its slots (or its MeshInstanceData) is a fake.
    fake: Option<AnvilFake>,
}

/// A MeshInstanceData's material-info entries as (source, replacement) pairs, in stored order.
fn pairs(m: &MeshInstance) -> Vec<(u64, u64)> {
    m.material_infos
        .iter()
        .map(|&[_, source, replacement]| (source, replacement))
        .collect()
}

/// `v`'s slots: where its instance matrices are, and its overrides.
fn shows(v: &Visual, at: At) -> Result<Shows, Error> {
    let slots: Vec<(u8, &MeshInstance)> = match &v.instance {
        Shown::Lod { instances, slots, .. } => {
            if !instances.is_empty() {
                return Err(Error::Instances(at, "in the LODSelectorInstance"));
            }
            let mut mesh_slots = Vec::new();
            for (level, s) in (0u8..).zip(slots) {
                match s {
                    Slot::Mesh(m) => mesh_slots.push((level, m)),
                    Slot::Impostor(i) if !i.instances.is_empty() => {
                        return Err(Error::Instances(at, "in an ImpostorInstanceData"));
                    }
                    Slot::Impostor(_) | Slot::None => {}
                }
            }
            mesh_slots
        }
        Shown::Mesh(m) => vec![(0, m)],
        Shown::Raw { .. } | Shown::None => Vec::new(),
    };
    let mut holding = slots.iter().filter(|(_, m)| !m.instances.is_empty());
    let instances = match (holding.next(), holding.next()) {
        (None, _) => None,
        (Some(&(level, m)), None) => Some((level, m.instances.clone())),
        (Some(_), Some(_)) => return Err(Error::Instances(at, "in more than one slot")),
    };
    Ok(Shows {
        instances,
        overrides: slots
            .iter()
            .filter(|(_, m)| !m.material_infos.is_empty())
            .map(|&(level, m)| (level, pairs(m)))
            .collect(),
        mesh_slots: u64::try_from(slots.len()).unwrap_or(u64::MAX),
        fake: slots.iter().find(|(_, m)| m.is_fake()).map(|(_, m)| AnvilFake {
            mesh_index: m.fake_mesh_index(),
            near: m.is_near_fake(),
        }),
    })
}

/// Places Entities, keeping each object's meshes and prototypes for every Entity that shows it.
#[derive(Debug)]
pub struct Placer<'a> {
    archive: &'a mut Archive,
    /// The world placed into: the prototypes kept below are its entities.
    world: &'a mut World,
    frame: SourceFrame,
    instanced: InstancedLevels,
    instanced_collision: InstancedCollision,
    /// Whether meshes' Materials are read.
    read_materials: bool,
    /// Whether Materials' templates' shader permutations are read.
    read_shaders: bool,
    /// Per template id, its [`AnvilShaders`]' entity.
    shaders: HashMap<u64, Entity>,
    /// Per Material id, its entity (`None`: not a Material, or not read).
    materials: HashMap<u64, Option<Entity>>,
    /// Per TextureMap id, its texture's entity.
    textures: HashMap<u64, Option<Entity>>,
    /// Per DDecalModel id, its entity (`None`: not a DDecalModel, or not read).
    decal_models: HashMap<u64, Option<Entity>>,
    /// Per object id: its mesh levels (`None`: not a LODSelector or Mesh, none of its levels a Mesh, or in no forge).
    objects: HashMap<u64, Option<Levels>>,
    /// Per object id: how many of its leading levels its LODSelector's dependencies fill.
    dependency_levels: HashMap<u64, u8>,
    /// Per object id: its level distances, where it states any.
    distances: HashMap<u64, AnvilLodDistances>,
    /// Per (prototype, object id): the copy of a prototype that two objects with other distances share.
    distance_copies: HashMap<(Entity, u64), Entity>,
    /// Per object id and level: the prototype placing every mesh level from that level on.
    wholes: HashMap<(u64, u8), Entity>,
    /// The entities some prototype holds as a level-of-detail form (each form belongs to one prototype).
    forms: HashSet<Entity>,
    /// The entities placed as prototypes themselves, and the copy placed for one that is also a form.
    prototypes: HashSet<Entity>,
    prototype_copies: HashMap<Entity, Entity>,
    /// Per shape id: what it places (see [`Placer::shape`]).
    shapes: HashMap<u64, Result<Parts, u32>>,
    /// The (object, level) pairs whose mesh was refused.
    refused_levels: HashSet<(u64, u8)>,
}

impl<'a> Placer<'a> {
    /// The placer placing instanced Entities' collision as `choice` states ([`InstancedCollision::Skip`] by default).
    /// The placer reading each mesh's Materials too ([`AnvilMaterial`], their textures as core textures), linked
    /// from its ranges.
    pub fn with_materials(mut self) -> Placer<'a> {
        self.read_materials = true;
        self
    }

    /// The placer reading, with each Material, its template's shader permutations ([`AnvilShaders`], on an
    /// entity per template).
    pub fn with_shaders(mut self) -> Placer<'a> {
        self.read_shaders = true;
        self
    }

    pub fn with_instanced_collision(mut self, choice: InstancedCollision) -> Placer<'a> {
        self.instanced_collision = choice;
        self
    }

    /// A placer reading through `archive` into `world`.
    pub fn new(
        archive: &'a mut Archive,
        world: &'a mut World,
        frame: SourceFrame,
        instanced: InstancedLevels,
    ) -> Placer<'a> {
        Placer {
            archive,
            world,
            frame,
            instanced,
            instanced_collision: InstancedCollision::Skip,
            read_materials: false,
            read_shaders: false,
            shaders: HashMap::new(),
            materials: HashMap::new(),
            textures: HashMap::new(),
            decal_models: HashMap::new(),
            objects: HashMap::new(),
            dependency_levels: HashMap::new(),
            distances: HashMap::new(),
            distance_copies: HashMap::new(),
            wholes: HashMap::new(),
            forms: HashSet::new(),
            prototypes: HashSet::new(),
            prototype_copies: HashMap::new(),
            shapes: HashMap::new(),
            refused_levels: HashSet::new(),
        }
    }

    /// The world placed into.
    pub fn world(&self) -> &World {
        self.world
    }

    /// What the shape `id` names places, read once: each part a prototype holding collision in its owner's local frame
    /// (the Riders Republic notes find the owner's stored bounds holding mesh and shape under s R on 213 of 213 and 218
    /// of 218) and its transform within the owner, if any. A MeshShape's
    /// [`AnvilMeshShape`](crate::components::AnvilMeshShape); a SphereShape's or CapsuleShape's [`AnvilCollisionShape`];
    /// a BoxShape's, at its own transform; a ReferenceListShape's children, each at its transform in the list. `Err`
    /// holds the type of a shape not read (0: an id in no forge), counted in `placed` for every body naming it.
    fn shape(&mut self, id: u64, placed: &mut Placed) -> Result<Result<Parts, u32>, Error> {
        let parts = match self.shapes.get(&id) {
            Some(parts) => parts.clone(),
            None => {
                let parts = match self.read_shape(id, placed) {
                    Ok(parts) => parts,
                    Err(e) => {
                        placed.refused.push((id, e.to_string()));
                        Ok(Vec::new())
                    }
                };
                self.shapes.insert(id, parts.clone());
                parts
            }
        };
        if let Err(kind) = parts {
            *placed.shapes_unread.entry(kind).or_default() += 1;
        }
        Ok(parts)
    }

    /// The parts of the shape `id` names (see [`Placer::shape`]).
    fn read_shape(&mut self, id: u64, placed: &mut Placed) -> Result<Result<Parts, u32>, Error> {
        let Some(at) = self.archive.find(id) else {
            return Ok(Err(0));
        };
        let r = self.archive.resource(at)?;
        let frame = self.frame;
        let shape_err = |source| Error::ShapeBody { at, source };
        let scale = frame.metres_per_unit() as f32;
        let xyz = |v: [f32; 4]| [v[0], v[1], v[2]];
        let (geometry, material, local) = match r.type_id {
            MESH_SHAPE => {
                let shape = MeshShape::parse(&r.body).map_err(|source| Error::Shape { at, source })?;
                let archive = &*self.archive;
                let component = anvil_mesh_shape(&shape, at.entry, id, frame, &|m| entry_name(archive, m))?;
                let e = self.prototype(at);
                self.world.insert(e, component);
                return Ok(Ok(vec![(e, None)]));
            }
            SPHERE_SHAPE => {
                let s = StoredSphere::parse(&r.body).map_err(shape_err)?;
                let geometry = ShapeGeometry::Sphere {
                    centre: frame.point(xyz(s.centre)),
                    radius: s.radius * scale,
                };
                (geometry, s.material, None)
            }
            CAPSULE_SHAPE => {
                let s = StoredCapsule::parse(&r.body).map_err(shape_err)?;
                let geometry = ShapeGeometry::Capsule {
                    a: frame.point(xyz(s.a)),
                    b: frame.point(xyz(s.b)),
                    radius: s.radius * scale,
                };
                (geometry, s.material, None)
            }
            BOX_SHAPE => {
                let s = StoredBox::parse(&r.body).map_err(shape_err)?;
                let geometry = ShapeGeometry::Box {
                    half_extents: frame.direction(xyz(s.half_extents)).map(|v| v.abs() * scale),
                };
                (geometry, s.material, Some(transform(&s.matrix, 1.0)))
            }
            REFERENCE_LIST_SHAPE => {
                let list = StoredReferenceList::parse(&r.body).map_err(shape_err)?;
                let mut parts = Vec::new();
                for (child, m) in list.children.iter().zip(&list.matrices) {
                    let within = transform(m, 1.0);
                    match self.shape(child.id, placed)? {
                        Ok(child_parts) => parts.extend(
                            child_parts
                                .into_iter()
                                .map(|(e, local)| (e, Some(local.map_or(within, |l| compose(&within, &l))))),
                        ),
                        Err(kind) => return Ok(Err(kind)),
                    }
                }
                return Ok(Ok(parts));
            }
            kind => return Ok(Err(kind)),
        };
        let e = self.prototype(at);
        self.world.insert(
            e,
            AnvilCollisionShape {
                entry: u32::try_from(at.entry).unwrap_or(u32::MAX),
                index_id: id,
                geometry,
                material: material.map(|m| CollisionMaterialRef {
                    id: m.id,
                    name: entry_name(self.archive, m.id),
                }),
            },
        );
        Ok(Ok(vec![(e, local)]))
    }

    /// A new entity named for the entry at `at`.
    fn prototype(&mut self, at: At) -> Entity {
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        e
    }

    /// The name of the entry at `at`, if its forge names it.
    fn name(&self, at: At) -> Option<String> {
        self.archive
            .forge(at.forge)
            .and_then(|f| f.names.get(at.entry))
            .map(|n| n.name.clone())
    }

    /// An entity holding the AnvilMesh of the Mesh at `at`, whose body is `body` and ClassID `id`.
    fn mesh(&mut self, at: At, body: &[u8], id: u64, placed: &mut Placed) -> Result<Entity, Error> {
        let stored = StoredMesh::parse(body).map_err(|source| Error::Mesh { at, source })?;
        let mut mesh = anvil_mesh(&stored, at.entry, id, self.frame)?;
        if self.read_materials {
            let ids = mesh.materials.clone();
            mesh.material_entities = ids.into_iter().map(|m| self.material(m, placed)).collect();
        }
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        self.world.insert(e, mesh);
        Ok(e)
    }

    /// The entity holding the Material `id` names, read once: its [`AnvilMaterial`], its textures read too. `None`
    /// where it isn't a Material or can't be read (recorded in `placed`). Placing reads a mesh's Materials so, where the
    /// placer reads Materials ([`Placer::with_materials`]); a caller may read one by its id.
    pub fn material(&mut self, id: u64, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.materials.get(&id) {
            return e;
        }
        let e = match self.read_material(id, placed) {
            Ok(e) => e,
            Err(why) => {
                placed.refused.push((id, why.to_string()));
                None
            }
        };
        self.materials.insert(id, e);
        e
    }

    fn read_material(&mut self, id: u64, placed: &mut Placed) -> Result<Option<Entity>, Error> {
        let Some(at) = self.archive.find(id) else {
            return Ok(None);
        };
        let r = self.archive.resource(at)?;
        if r.type_id != MATERIAL {
            return Ok(None);
        }
        let m = StoredMaterial::parse(&r.body).map_err(|source| Error::Material { at, source })?;
        let template = m.references[0].map(|t| t.id);
        let mut texture_set = Vec::new();
        if let Some(set_at) = m.references[1].and_then(|s| self.archive.find(s.id)) {
            let body = self.archive.resource(set_at)?.body;
            let set =
                StoredTextureSet::parse(&body).map_err(|source| Error::TextureSet { at: set_at, source })?;
            texture_set = set
                .references
                .iter()
                .map(|r| r.and_then(|r| self.texture(r.id, placed)))
                .collect();
        }
        let mut bindings = Vec::new();
        let mut shaders = None;
        // each texture binding's key, shader slot and TextureParameter
        let mut texture_bindings: Vec<(u32, u32, Option<template::TextureParameter>)> = Vec::new();
        // each other binding's default, from its Operator
        let mut defaults = Vec::new();
        if let Some((id, t)) = template.and_then(|id| Some((id, self.archive.find(id)?))) {
            let body = self.archive.resource(t)?.body;
            if self.read_shaders {
                shaders = Some(self.template_shaders(id, t, &body)?);
            }
            let read = template::bindings(&body).map_err(|source| Error::Bindings { at: t, source })?;
            for b in &read {
                if let BindingKind::Texture { slot } = b.kind() {
                    let p = template::texture_parameter(&body, b)
                        .map_err(|source| Error::Bindings { at: t, source })?;
                    texture_bindings.push((b.key, slot, p));
                    continue;
                }
                let binding = match b.kind() {
                    BindingKind::Scalar { vec4 } => MaterialBinding::Scalar { vec4 },
                    BindingKind::Vector { vec4 } => MaterialBinding::Vector { vec4 },
                    BindingKind::UvTransform { vec4 } => MaterialBinding::UvTransform { vec4 },
                    BindingKind::Texture { .. } | BindingKind::Other => continue,
                };
                let value =
                    match template::operator(&body, b).map_err(|source| Error::Bindings { at: t, source })? {
                        Some(template::OperatorDefault::Float(x)) => MaterialValue::Float(x),
                        Some(template::OperatorDefault::Vector(v)) => MaterialValue::Float4(v),
                        Some(template::OperatorDefault::Integer(v)) => MaterialValue::Int(v),
                        Some(template::OperatorDefault::Uv { floats, .. }) => {
                            MaterialValue::Floats(floats.to_vec())
                        }
                        _ => continue,
                    };
                defaults.push((binding, value));
            }
            bindings = read
                .iter()
                .map(|b| {
                    let kind = match b.kind() {
                        BindingKind::Texture { slot } => MaterialBinding::Texture { slot },
                        BindingKind::Scalar { vec4 } => MaterialBinding::Scalar { vec4 },
                        BindingKind::Vector { vec4 } => MaterialBinding::Vector { vec4 },
                        BindingKind::UvTransform { vec4 } => MaterialBinding::UvTransform { vec4 },
                        BindingKind::Other => MaterialBinding::Other,
                    };
                    (b.key, kind)
                })
                .collect();
        }
        let mut values = Vec::with_capacity(m.properties.len());
        // the Material's texture records: (its value's index, key, slot, channel, whether its texture is null)
        let mut records: Vec<(usize, u32, u32, u32, bool)> = Vec::new();
        for p in &m.properties {
            if let Payload::Texture {
                words: [channel, slot, _],
                references: [_, map],
                ..
            } = &p.payload
            {
                records.push((values.len(), p.key, *slot, *channel, map.is_none()));
            }
            let value = match &p.payload {
                Payload::Int(v) => MaterialValue::Int(*v),
                Payload::Float(v) => MaterialValue::Float(*v),
                Payload::Float4(v) => MaterialValue::Float4(*v),
                Payload::Texture {
                    words: [channel, slot, _],
                    references: [_, map],
                    ..
                } => MaterialValue::Texture {
                    texture: map.and_then(|map| self.texture(map.id, placed)),
                    slot: *slot,
                    channel: *channel,
                },
                Payload::Floats8 { floats, .. } => MaterialValue::Floats(floats.to_vec()),
                Payload::Floats6 { floats, .. } => MaterialValue::Floats(floats.to_vec()),
                Payload::Raw45 { .. } => MaterialValue::Other,
            };
            values.push((p.key, value));
        }
        let mut textures = Vec::with_capacity(texture_bindings.len());
        for &(key, shader_slot, p) in &texture_bindings {
            // a generic `Texture` (its key the CRC-32 of the label, or shared by two of the template's bindings)
            // matches a record by its slot and channel too
            let generic =
                key == GENERIC_TEXTURE || texture_bindings.iter().filter(|(k, ..)| *k == key).count() > 1;
            let parameter = p.map(|p| (p.slot, p.channel));
            let record = records.iter().find(|&&(_, k, slot, channel, _)| {
                k == key && (!generic || parameter == Some((slot, channel)))
            });
            let fallback = |placer: &mut Self, placed: &mut Placed| -> (Option<Entity>, TextureFrom) {
                match p {
                    Some(p) if p.channel == 0 => match texture_set.get(p.slot as usize).copied().flatten() {
                        Some(t) => (Some(t), TextureFrom::Set),
                        None => (None, TextureFrom::Unset),
                    },
                    Some(p) => match p.texture.and_then(|r| placer.texture(r.id, placed)) {
                        Some(t) => (Some(t), TextureFrom::Default),
                        None => (None, TextureFrom::Unset),
                    },
                    None => (None, TextureFrom::Unset),
                }
            };
            let (texture, from) = match record {
                Some(&(.., true)) => {
                    let (fallback, _) = fallback(self, placed);
                    (None, TextureFrom::NullRecord { fallback })
                }
                Some(&(index, ..)) => match values.get(index).map(|(_, v)| v) {
                    Some(MaterialValue::Texture { texture, .. }) => (*texture, TextureFrom::Record),
                    _ => (None, TextureFrom::Unset),
                },
                None => fallback(self, placed),
            };
            textures.push(BoundTexture {
                key,
                shader_slot,
                parameter,
                texture,
                from,
            });
        }
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        let template_name = template.and_then(|t| entry_name(self.archive, t));
        self.world.insert(
            e,
            AnvilMaterial {
                index_id: id,
                template,
                template_name,
                bindings,
                values,
                texture_set,
                textures,
                defaults,
                frame: self.frame,
                flags: m.flags(),
                shaders,
            },
        );
        Ok(Some(e))
    }

    /// The entity holding template `id`'s [`AnvilShaders`] (at `at`, its body `body`), read once.
    fn template_shaders(&mut self, id: u64, at: At, body: &[u8]) -> Result<Entity, Error> {
        if let Some(&e) = self.shaders.get(&id) {
            return Ok(e);
        }
        let read = template::permutations(body).map_err(|source| Error::Permutations { at, source })?;
        let features = template::feature_node(body).map_err(|source| Error::Features { at, source })?;
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        self.world.insert(e, template::shader_tables(&read, features));
        self.shaders.insert(id, e);
        Ok(e)
    }

    /// The entity holding the DDecalModel `id` names, read once: its [`AnvilDecalModel`], its textures read too.
    /// `None` where it isn't a DDecalModel or can't be read (recorded in `placed`).
    fn decal_model(&mut self, id: u64, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.decal_models.get(&id) {
            return e;
        }
        let read = (|| -> Result<Option<Entity>, String> {
            let Some(at) = self.archive.find(id) else {
                return Ok(None);
            };
            let r = self.archive.resource(at).map_err(|e| e.to_string())?;
            if r.type_id != DDECAL_MODEL {
                return Ok(None);
            }
            let m = StoredDecalModel::parse(&r.body).map_err(|e| e.to_string())?;
            let textures = m.textures.map(|t| t.and_then(|t| self.texture(t.id, placed)));
            let e = self.world.spawn();
            if let Some(name) = self.name(at) {
                self.world.insert(e, Name(name));
            }
            self.world.insert(
                e,
                AnvilDecalModel {
                    textures,
                    opacity: m.opacity(),
                    byte: m.byte,
                    flags: m.flags,
                    parameters: m.parameters,
                },
            );
            Ok(Some(e))
        })();
        let e = match read {
            Ok(e) => e,
            Err(why) => {
                placed.refused.push((id, why));
                None
            }
        };
        self.decal_models.insert(id, e);
        e
    }

    /// The entity holding the TextureMap `id` names, read once: a core texture of its levels (its CompiledMips', then
    /// its own), colour space linear and role other until a lowering states them, and its [`AnvilTextureMap`]. `None`
    /// where it isn't a TextureMap, its format code's codec isn't established, or it can't be read (recorded).
    fn texture(&mut self, id: u64, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.textures.get(&id) {
            return e;
        }
        let e = match self.read_texture(id) {
            Ok(e) => e,
            Err(why) => {
                placed.refused.push((id, why.to_string()));
                None
            }
        };
        self.textures.insert(id, e);
        e
    }

    fn read_texture(&mut self, id: u64) -> Result<Option<Entity>, Error> {
        Ok(textures::read_texture_map(self.archive, self.world, id)?)
    }

    /// The mesh levels of the object `id` names, read once.
    fn levels(&mut self, id: u64, placed: &mut Placed) -> Result<Option<Levels>, Error> {
        if let Some(levels) = self.objects.get(&id) {
            return Ok(levels.clone());
        }
        let levels = match self.read_levels(id, placed) {
            Ok(levels) => levels,
            Err(e) => {
                placed.refused.push((id, e.to_string()));
                None
            }
        };
        self.objects.insert(id, levels.clone());
        Ok(levels)
    }

    /// The mesh levels of the object `id` names (see [`Placer::levels`]); a mesh level that can't be read is recorded
    /// in `placed` and left out.
    fn read_levels(&mut self, id: u64, placed: &mut Placed) -> Result<Option<Levels>, Error> {
        let levels = match self.archive.find(id) {
            None => None,
            Some(at) => {
                let r = self.archive.resource(at)?;
                match r.type_id {
                    MESH => {
                        let culling = max_culling_distance(&r.body);
                        if culling > 0.0 {
                            self.distances.insert(
                                id,
                                AnvilLodDistances {
                                    levels: Vec::new(),
                                    culling,
                                },
                            );
                        }
                        Some(vec![(0, self.mesh(at, &r.body, id, placed)?)])
                    }
                    LOD_SELECTOR => {
                        let s =
                            StoredLodSelector::parse(&r.body).map_err(|source| Error::Lod { at, source })?;
                        let ids = s.level_ids().map_err(|source| Error::Lod { at, source })?;
                        let filled = u8::try_from(s.dependencies.len()).unwrap_or(u8::MAX);
                        self.dependency_levels.insert(id, filled);
                        let mut levels = Vec::new();
                        let mut distances = AnvilLodDistances {
                            levels: Vec::new(),
                            culling: 0.0,
                        };
                        for (level, id_at) in (0u8..).zip(ids) {
                            let Some(reference) = id_at else {
                                continue;
                            };
                            let mesh = match self.archive.find(reference) {
                                Some(m) => Some((m, self.archive.resource(m)?)),
                                None => None,
                            };
                            match mesh {
                                Some((m, r)) if r.type_id == MESH => {
                                    match self.mesh(m, &r.body, reference, placed) {
                                        Ok(e) => {
                                            levels.push((level, e));
                                            let switch = s
                                                .levels
                                                .get(usize::from(level))
                                                .map_or(0.0, |l| l.switch_distance());
                                            distances.levels.push((level, switch));
                                            distances.culling = max_culling_distance(&r.body);
                                        }
                                        Err(e) => {
                                            placed.refused.push((reference, e.to_string()));
                                            self.refused_levels.insert((id, level));
                                        }
                                    }
                                }
                                _ => placed.impostor_levels = placed.impostor_levels.saturating_add(1),
                            }
                        }
                        if !levels.is_empty() {
                            self.distances.insert(id, distances);
                        }
                        (!levels.is_empty()).then_some(levels)
                    }
                    _ => None,
                }
            }
        };
        Ok(levels)
    }

    /// The prototype placing every one of `levels` (the object `id`'s, from one level on): the finest's mesh entity
    /// where it is the only one, else an entity holding a copy of its AnvilMesh and the others as [`AnvilLods`].
    fn whole(&mut self, id: u64, levels: &[(u8, Entity)]) -> Option<Entity> {
        let (&(detail, own), forms) = levels.split_first()?;
        if let Some(&p) = self.wholes.get(&(id, detail)) {
            return Some(p);
        }
        let p = if forms.is_empty() {
            self.as_prototype(own)
        } else {
            let p = self.world.spawn();
            if let Some(name) = self.archive.find(id).and_then(|at| self.name(at)) {
                self.world.insert(p, Name(name));
            }
            if let Some(mesh) = self.world.get::<AnvilMesh>(own).cloned() {
                self.world.insert(p, mesh);
            }
            let forms = forms
                .iter()
                .map(|&(level, form)| (level, self.as_form(form)))
                .collect();
            self.world.insert(p, AnvilLods { detail, forms });
            p
        };
        self.wholes.insert((id, detail), p);
        Some(p)
    }

    /// `p` (a prototype placing the object `object`) carrying the object's level distances: itself where it carries
    /// none yet or the same, else one copy of it per object (a mesh the only level of two objects with other
    /// distances).
    fn with_distances(&mut self, p: Entity, object: u64) -> Entity {
        let Some(d) = self.distances.get(&object).cloned() else {
            return p;
        };
        match self.world.get::<AnvilLodDistances>(p) {
            None => {
                self.world.insert(p, d);
                p
            }
            Some(have) if *have == d => p,
            Some(_) => {
                if let Some(&c) = self.distance_copies.get(&(p, object)) {
                    return c;
                }
                let c = self.world.duplicate(p).unwrap_or(p);
                self.world.insert(c, d);
                self.prototypes.insert(c);
                self.distance_copies.insert((p, object), c);
                c
            }
        }
    }

    /// `e` (a mesh level's entity) as a form of one prototype: itself the first time, else a copy (another
    /// prototype holds it as a form, or it is placed itself: the same mesh a level of several prototypes).
    fn as_form(&mut self, e: Entity) -> Entity {
        let e = if self.forms.contains(&e) || self.prototypes.contains(&e) {
            self.world.duplicate(e).unwrap_or(e)
        } else {
            e
        };
        self.forms.insert(e);
        e
    }

    /// `e`'s `BoundingVolume` as a box in core's frame: its two corners (relative to the owner's translation, in the
    /// source frame) at the translation, each carried through the frame, then the component-wise least and greatest
    /// (the frame only permutes and negates axes, so the box stays a box).
    fn owner_bounds(&self, e: &StoredEntity) -> AnvilBounds {
        let t = [e.matrix[12], e.matrix[13], e.matrix[14]];
        let corner = |c: [f32; 3]| self.frame.point(std::array::from_fn(|k| t[k] + c[k]));
        let (a, b) = (corner(e.bounds.min), corner(e.bounds.max));
        AnvilBounds {
            min: std::array::from_fn(|k| a[k].min(b[k])),
            max: std::array::from_fn(|k| a[k].max(b[k])),
        }
    }

    /// `e` (a mesh level's entity) placed as a prototype itself: itself, or, where it is a form, one copy of it.
    fn as_prototype(&mut self, e: Entity) -> Entity {
        if let Some(&c) = self.prototype_copies.get(&e) {
            return c;
        }
        if !self.forms.contains(&e) {
            self.prototypes.insert(e);
            return e;
        }
        let c = self.world.duplicate(e).unwrap_or(e);
        self.prototype_copies.insert(e, c);
        self.prototypes.insert(c);
        c
    }

    /// The Mesh or LODSelector at `at` placed once, at the origin: what a Visual of an Entity places, without an owner
    /// (no matrix, bounds, DataLayerActions or material overrides). A Mesh is its one level; a LODSelector every mesh
    /// level it names, as an Entity's Visual without instances places them (levels referring to an Impostor are
    /// counted in [`Placed::impostor_levels`]). Its Materials are read where the placer reads them
    /// ([`Placer::with_materials`]). `None` where no level of it could be read (recorded in `placed`).
    pub fn place_object(&mut self, at: At, placed: &mut Placed) -> Result<Option<Entity>, Error> {
        let (id, found) = {
            let forge = self
                .archive
                .forge(at.forge)
                .ok_or(crate::read::archive::Error::NoForge(at.forge))?;
            let named = forge.names.get(at.entry).map(|n| n.type_id);
            (forge.index.get(at.entry).map(|i| i.id), named)
        };
        let (Some(id), Some(found)) = (id, found) else {
            return Err(Error::Type {
                at,
                found: 0,
                what: "an entry of its forge",
            });
        };
        if found != MESH && found != LOD_SELECTOR {
            return Err(Error::Type {
                at,
                found,
                what: "a Mesh or LODSelector",
            });
        }
        let Some(levels) = self.levels(id, placed)? else {
            return Ok(None);
        };
        let Some(prototype) = self.whole(id, &levels) else {
            return Ok(None);
        };
        let prototype = self.with_distances(prototype, id);
        let p = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(p, Name(name));
        }
        self.world.insert(
            p,
            Instance {
                prototype,
                variant: 0,
            },
        );
        self.world.insert(p, self.frame.transform(&Transform::IDENTITY));
        placed.placements.push(p);
        Ok(Some(p))
    }

    /// The Entity or EntityGroup at `at` placed into the world: one placement per Visual with something to show, or per
    /// instance. A group's own Visuals place as an Entity's (it is an Entity's body and its member list).
    ///
    /// An error refuses the owner: its body, or how its Visuals' instances are stored, can't be read as placed. An
    /// object it shows or a shape its RigidBody names that can't be read is recorded in [`Placed::refused`] instead,
    /// and only what needs it is left out.
    pub fn place(&mut self, at: At, placed: &mut Placed) -> Result<(), Error> {
        let r = self.archive.resource(at)?;
        let (e, group) = match r.type_id {
            ENTITY => (
                StoredEntity::parse(&r.body).map_err(|source| Error::Entity { at, source })?,
                false,
            ),
            ENTITY_GROUP => {
                let g = StoredEntityGroup::parse(&r.body).map_err(|source| Error::Entity { at, source })?;
                placed.members.extend(g.members.iter().map(|m| m.id));
                (g.entity, true)
            }
            found => {
                return Err(Error::Type {
                    at,
                    found,
                    what: "an Entity or EntityGroup",
                });
            }
        };
        // the owner's IsHidden / IsPhantom, on each of its placements: a pass keeps or drops them
        let state = (e.is_hidden() || e.is_phantom()).then(|| AnvilEntityState {
            hidden: e.is_hidden(),
            phantom: e.is_phantom(),
        });
        if state.is_some() {
            placed.hidden_or_phantom = placed.hidden_or_phantom.saturating_add(1);
        }
        for c in &e.components {
            if matches!(c.body, ComponentBody::Unread(_)) {
                *placed.unread_components.entry(c.kind).or_default() += 1;
            }
        }
        let name = self.name(at).unwrap_or_default();
        let bounds = self.owner_bounds(&e);
        let mut visual_placements = 0u64;
        let layers = (!e.data_layers.is_empty()).then(|| AnvilDataLayers {
            actions: e
                .data_layers
                .iter()
                .map(|&(layer, word)| DataLayerAction {
                    layer,
                    name: entry_name(self.archive, layer),
                    word,
                })
                .collect(),
        });
        for (v, decal) in e
            .visuals()
            .map(|v| (v, false))
            .chain(e.decals().map(|v| (v, true)))
        {
            let shows = shows(v, at)?;
            let fake = shows.fake;
            let found = match v.object {
                Some(object) => self.levels(object.id, placed)?.map(|l| (object.id, l)),
                None => None,
            };
            let Some((object, levels)) = found else {
                placed.visuals_skipped = placed.visuals_skipped.saturating_add(1);
                continue;
            };
            let (prototype, transforms, overrides) = match shows.instances {
                None => {
                    let Some(p) = self.whole(object, &levels) else {
                        placed.visuals_skipped = placed.visuals_skipped.saturating_add(1);
                        continue;
                    };
                    (p, vec![transform(&e.matrix, e.scale)], shows.overrides)
                }
                Some((level, instances)) => {
                    if e.scale != 1.0 {
                        return Err(Error::InstancedScale(at, e.scale));
                    }
                    let Some(&(_, mesh)) = levels.iter().find(|(l, _)| *l == level) else {
                        if self.refused_levels.contains(&(object, level)) {
                            placed.visuals_skipped = placed.visuals_skipped.saturating_add(1);
                            continue;
                        }
                        return Err(Error::Slot(at, level));
                    };
                    let every = matches!(self.instanced, InstancedLevels::Selector | InstancedLevels::Every);
                    let with_filled = self.instanced == InstancedLevels::Every;
                    // the levels the selector's dependencies fill before the slot's: no slot holds them
                    let filled = self.dependency_levels.get(&object).copied().unwrap_or(0);
                    let before = levels.iter().filter(|(l, _)| *l < level).count();
                    // the slot is the first level after the filled ones, so every level before it is a filled one
                    let only_filled = filled > 0 && level == filled;
                    if only_filled && !with_filled {
                        placed.dependency_levels_unplaced =
                            placed.dependency_levels_unplaced.saturating_add(before as u64);
                    }
                    let (prototype, overrides) = match (every, levels.first()) {
                        (true, Some(&(finest, _))) if finest == level || only_filled => {
                            // Every: the filled levels too, the finest first; Selector: from the slot's level on
                            let from: Levels = levels
                                .iter()
                                .copied()
                                .filter(|(l, _)| with_filled || *l >= level)
                                .collect();
                            let p = match self.whole(object, &from) {
                                Some(p) => p,
                                None => self.as_prototype(mesh),
                            };
                            (p, shows.overrides)
                        }
                        (true, _) => {
                            return Err(Error::Instances(
                                at,
                                "in a slot other than its finest mesh level's",
                            ));
                        }
                        (false, _) => {
                            let others = shows.mesh_slots.saturating_sub(1);
                            placed.levels_unplaced = placed.levels_unplaced.saturating_add(others);
                            let own = shows.overrides.into_iter().filter(|(l, _)| *l == level).collect();
                            (self.as_prototype(mesh), own)
                        }
                    };
                    let entity = transform(&e.matrix, 1.0);
                    let transforms = instances
                        .iter()
                        .map(|i| compose(&entity, &transform(i, 1.0)))
                        .collect();
                    (prototype, transforms, overrides)
                }
            };
            let prototype = self.with_distances(prototype, object);
            for t in transforms {
                let p = self.world.spawn();
                self.world.insert(p, Name(name.clone()));
                self.world.insert(
                    p,
                    Instance {
                        prototype,
                        variant: 0,
                    },
                );
                self.world.insert(p, self.frame.transform(&t));
                if !overrides.is_empty() {
                    self.world.insert(
                        p,
                        AnvilMaterialOverrides {
                            levels: overrides.clone(),
                        },
                    );
                }
                if let Some(layers) = &layers {
                    self.world.insert(p, layers.clone());
                }
                if let Some(s) = state {
                    self.world.insert(p, s);
                }
                if let Some(fake) = fake {
                    self.world.insert(p, fake);
                    placed.fakes = placed.fakes.saturating_add(1);
                }
                self.world.insert(p, bounds);
                visual_placements = visual_placements.saturating_add(1);
                placed.placements.push(p);
                if decal {
                    placed.static_decals = placed.static_decals.saturating_add(1);
                }
            }
        }
        // lakes: each WaterComponent's patches and atlases, on an entity of its own
        for c in &e.components {
            let ComponentBody::Raw(body) = &c.body else {
                continue;
            };
            if c.kind != WATER_COMPONENT {
                continue;
            }
            let stored = match StoredWater::parse(body) {
                Ok(stored) => stored,
                Err(why) => {
                    placed.lakes_unread = placed.lakes_unread.saturating_add(1);
                    placed.refused.push((c.id, why.to_string()));
                    continue;
                }
            };
            let atlases = stored.atlases.map(|r| r.and_then(|r| self.texture(r.id, placed)));
            let lake = self.world.spawn();
            self.world.insert(lake, Name(name.clone()));
            self.world.insert(
                lake,
                AnvilWater {
                    frame: self.frame,
                    patches: stored.patches,
                    atlases,
                },
            );
            if let Some(layers) = &layers {
                self.world.insert(lake, layers.clone());
            }
            if let Some(s) = state {
                self.world.insert(lake, s);
            }
            placed.lakes.push(lake);
        }
        // deferred decals: each item of each StaticDDecalComponent or HoudiniDecalInstancerComponent, on an entity of its own
        for c in &e.components {
            let ComponentBody::Raw(body) = &c.body else {
                continue;
            };
            if !matches!(c.kind, HOUDINI_DECAL_INSTANCER | STATIC_DDECAL) {
                continue;
            }
            let stored = match StoredDeferredDecals::parse(body) {
                Ok(stored) => stored,
                Err(why) => {
                    placed.deferred_decals_unread = placed.deferred_decals_unread.saturating_add(1);
                    placed.refused.push((c.id, why.to_string()));
                    continue;
                }
            };
            for item in &stored.items {
                let model = stored
                    .models
                    .get(item.model())
                    .copied()
                    .flatten()
                    .and_then(|r| self.decal_model(r.id, placed));
                let decals = item
                    .decals
                    .iter()
                    .map(|d| DeferredDecal {
                        axis_x: d.axis_x(),
                        axis_y: d.axis_y(),
                        centre: d.centre(),
                        bounds: d.bounds(),
                        window: d.window(),
                        fade: d.fade(),
                        record: d.0,
                    })
                    .collect();
                let set = self.world.spawn();
                self.world.insert(set, Name(name.clone()));
                self.world.insert(
                    set,
                    AnvilDeferredDecals {
                        frame: self.frame,
                        model,
                        decals,
                    },
                );
                if let Some(layers) = &layers {
                    self.world.insert(set, layers.clone());
                }
                if let Some(s) = state {
                    self.world.insert(set, s);
                }
                placed.deferred_decals.push(set);
            }
        }
        // collision: each RigidBodyComponent's shape at its owner's T R (an EntityGroup's merged shape spans its
        // instances: the notes, docs/skate-plan.md), where the owner's scale is 1; an instanced Entity's at each of its
        // instances, where the caller states so
        let mut instance_sets = e
            .visuals()
            .filter_map(|v| shows(v, at).ok().and_then(|s| s.instances).map(|(_, i)| i));
        let instances = match (instance_sets.next(), group) {
            (Some(first), false) => {
                if instance_sets.any(|other| other != first) {
                    return Err(Error::Instances(at, "of two Visuals differ, for one RigidBody"));
                }
                Some(first)
            }
            _ => None,
        };
        for c in &e.components {
            let ComponentBody::RigidBody {
                shape: Some(shape),
                bytes,
            } = &c.body
            else {
                continue;
            };
            let filter = match CollisionFilter::parse(bytes) {
                Ok(f) => f,
                Err(why) => {
                    placed.refused.push((c.id, why.to_string()));
                    continue;
                }
            };
            if filter.queries(query::WATER_LEVEL) && !filter.queries(query::STATIC) {
                placed.water_bodies = placed.water_bodies.saturating_add(1);
            }
            let mark = AnvilCollisionFilter {
                query: filter.query,
                layers: filter.layers,
                ground: filter.ground,
            };
            if instances.is_some() && self.instanced_collision == InstancedCollision::Skip {
                placed.instanced_collision = placed.instanced_collision.saturating_add(1);
                continue;
            }
            if e.scale != 1.0 {
                placed.scaled_collision = placed.scaled_collision.saturating_add(1);
                continue;
            }
            let Ok(parts) = self.shape(shape.id, placed)? else {
                continue;
            };
            let owner = transform(&e.matrix, 1.0);
            let owners: Vec<Transform> = match &instances {
                Some(instances) => instances
                    .iter()
                    .map(|i| compose(&owner, &transform(i, 1.0)))
                    .collect(),
                None => vec![owner],
            };
            if !filter.queries(query::STATIC) {
                placed.not_static = placed.not_static.saturating_add(1);
            }
            for owner in owners {
                for &(prototype, local) in &parts {
                    let p = self.world.spawn();
                    self.world.insert(p, Name(name.clone()));
                    self.world.insert(
                        p,
                        Instance {
                            prototype,
                            variant: 0,
                        },
                    );
                    let t = local.map_or(owner, |l| compose(&owner, &l));
                    self.world.insert(p, self.frame.transform(&t));
                    if let Some(layers) = &layers {
                        self.world.insert(p, layers.clone());
                    }
                    if let Some(s) = state {
                        self.world.insert(p, s);
                    }
                    self.world.insert(p, mark);
                    if visual_placements == 0 {
                        self.world.insert(p, AnvilCollisionOnly);
                        self.world.insert(p, bounds);
                        placed.collision_only = placed.collision_only.saturating_add(1);
                    }
                    placed.collisions.push(p);
                }
            }
        }
        Ok(())
    }
}
