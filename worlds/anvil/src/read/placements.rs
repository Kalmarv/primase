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
//! Impostor levels retain their authored cards and owned atlases in `AnvilImpostor`, including forms reached through
//! a selector; lowering chooses their static representation. An Entity or EntityGroup whose `IsHidden` or `IsPhantom`
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
    AnvilBounds, AnvilClip, AnvilClipBinding, AnvilClipCandidates, AnvilCollisionFilter, AnvilCollisionOnly,
    AnvilCollisionShape, AnvilDataLayers, AnvilDecalModel, AnvilDeferredDecals, AnvilEntityState, AnvilFake,
    AnvilGrass, AnvilGrassModel, AnvilLodDistances, AnvilLods, AnvilMaterial, AnvilMaterialOverrides,
    AnvilMesh, AnvilRig, AnvilRigOccurrence, AnvilSkeleton, AnvilSkinned, AnvilWater, BoundTexture,
    ClipCandidate, CollisionMaterialRef, DataLayerAction, DeferredDecal, GrassSystemSetup, LibraryClip,
    MaterialBinding, MaterialValue, ShapeGeometry, StoredBuildTable, TextureFrom,
};
use crate::read::animation::{ANIMATION, StoredAnimation};
use crate::read::archive::{Archive, At};
use crate::read::build_table::{BUILD_TABLE, FEMALE, MALE};
use crate::read::ddecal::{
    DDECAL_MODEL, HOUDINI_DECAL_INSTANCER, STATIC_DDECAL, StoredDecalModel, StoredDeferredDecals,
};
use crate::read::entity::{
    CollisionFilter, ComponentBody, ENTITY, ENTITY_GROUP, Instance as Shown, MeshInstance, Refusal,
    SkeletonBinding, Slot, StoredEntity, StoredEntityGroup, Visual, query,
};
use crate::read::grass::{GRASS_PATTERN_LIBRARY, StoredGrassLibrary};
use crate::components::AnvilImpostor;
use crate::read::impostor::{IMPOSTOR, StoredImpostor};
use crate::read::lod::{LOD_SELECTOR, StoredLodSelector};
use crate::read::material::{Payload, StoredMaterial, StoredTextureSet};
use crate::read::mesh::{MATERIAL, MESH, StoredMesh, max_culling_distance};
use crate::read::mesh_shape::{MESH_SHAPE, MeshShape};
use crate::read::meshes::anvil_mesh;
pub use crate::read::rider::VariantRow;
use crate::read::rider::{part_variants, stated};
use crate::read::rig::StoredRig;
use crate::read::shape::{
    BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE, StoredBox, StoredCapsule,
    StoredReferenceList, StoredSphere,
};
use crate::read::shapes::anvil_mesh_shape;
use crate::read::skeleton::{SKELETON, StoredSkeleton};
use crate::read::template::{self, BindingKind};
use crate::read::textures;
use crate::read::water::{StoredWater, WATER_COMPONENT};

/// Why an Entity can't be placed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{at:?}: Impostor: {why}")]
    Impostor { at: At, why: String },
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
    #[error("shape {id:#x}: {why}")]
    ShapeNesting { id: u64, why: String },
}

/// The deepest reference lists inside reference lists a shape is read through. The lists seen hold shapes of one kind;
/// a list may name itself or a list it is inside, and one named twice at each level doubles the parts.
const MAX_SHAPE_NESTING: usize = 16;

/// The most parts one shape may place: a reference list places the parts of each of its children, and the children
/// may be lists that share their own.
const MAX_SHAPE_PARTS: usize = 1 << 16;

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
    /// The grass setups: an entity each, holding its [`AnvilGrass`].
    pub grass: Vec<Entity>,
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
    /// The components of types read here whose rule refused their bytes ([`ComponentBody::Refused`]), kept as stored,
    /// once per owner placed: by type and reason ([`Refusal::reason`], why without the positions).
    pub refused_components: BTreeMap<(u32, String), Refusals>,
}

/// The components refused for one reason: how many, and the first in full.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusals {
    pub count: u64,
    pub first: Refusal,
}

/// The column-major 4x4 `m` as a transform, its linear part times `s`.
fn transform(m: &[f32; 16], s: f32) -> Transform {
    let col = |c: usize| [0, 1, 2].map(|r| m.get(c * 4 + r).copied().unwrap_or(0.0));
    let [x, y, z, t] = [col(0), col(1), col(2), col(3)];
    Transform {
        cols: [x.map(|v| v * s), y.map(|v| v * s), z.map(|v| v * s), t],
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
    /// Per Skeleton id, its entity (`None`: not a Skeleton, or not read).
    skeletons: HashMap<u64, Option<Entity>>,
    /// Per (Animation id, rig entity), the entity holding the clip (`None`: not an Animation, or can't be read).
    clips: HashMap<(u64, Entity), Option<Entity>>,
    /// Whether each rig gets the library's clips that fit it ([`Placer::with_clip_candidates`]).
    clip_candidates: bool,
    /// The Animations of the forges, read once for the first rig that asks.
    library: Option<Vec<std::sync::Arc<LibraryClip>>>,
    /// Per SkeletonComponent's Skeletons (the main one's id, then its addons'), the entity holding their rig (`None`:
    /// one of them isn't a Skeleton or can't be read).
    rigs: HashMap<Vec<u64>, Option<Entity>>,
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
    /// The shapes being read, outermost first: a reference list naming one of them is a loop.
    shapes_open: Vec<u64>,
    /// The (object, level) pairs whose mesh was refused.
    refused_levels: HashSet<(u64, u8)>,
}

/// Which row of a rider part table is taken: the table's rows are conditioned on a tag hash, and the male and female
/// hashes are the two the rider's tables have ([`MALE`], [`FEMALE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

impl Gender {
    /// The condition of the rows this gender takes.
    pub fn condition(self) -> u32 {
        match self {
            Gender::Male => MALE,
            Gender::Female => FEMALE,
        }
    }
}

/// Which materials a rider part is drawn with ([`RiderPartChoice::variant`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The meshes' own Materials (the generic ones the meshes name), nothing replaced.
    Own,
    /// The variant table the caller names (a `T###_V###` BuildTable): its row for the gender replaces each generic
    /// Material its columns' path nodes name with the Material its properties name.
    Table(u64),
    /// The variant the part's materials table states: the one row whose weight is exactly 1.0 (the other rows of the
    /// tables read so far state 0.0). Refused where the part has no single materials table or the table states no
    /// single such row: the weights of other tables are population odds, not a default.
    Stated,
    /// One row of a table the caller names, the row picked by its own condition (a colour's tag) instead of the gender:
    /// the hair and beard colour tables ([`crate::read::rider::colours`]) list one row per colour, each linking the
    /// Materials that replace the generic ones its columns' path nodes name. Refused where no single row has that
    /// condition.
    Row { table: u64, condition: u32 },
}

/// One part of a rider: its table and the materials it is drawn with, both the caller's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiderPartChoice {
    pub table: u64,
    pub variant: Variant,
}

/// A rider to put together ([`Placer::place_rider`]); every field is the caller's choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiderSpec {
    /// The occurrence's name.
    pub name: String,
    /// The rig's base Skeletons' ClassIDs, the main one first (a part's addon Skeleton is added to them).
    pub skeletons: Vec<u64>,
    /// The row each part table takes.
    pub gender: Gender,
    /// The parts: BuildTables whose rows name meshes, an addon Skeleton and the tables of their materials and addons.
    pub parts: Vec<RiderPartChoice>,
}

/// The Material replacements a variant table states for a gender: pairs of the generic Material (named by a column's
/// path node) and the Material the row's property of that column names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedVariant {
    pub table: u64,
    pub pairs: Vec<(u64, u64)>,
}

/// A part of a rider as its table states it, and what became of it.
#[derive(Clone, Debug, PartialEq)]
pub struct RiderPart {
    /// The part table's ClassID and its entry's name.
    pub table: u64,
    pub name: String,
    /// What the table's row for the gender links, by the type of what it names: every Mesh (the part's highest-detail
    /// form, `_LODM`; a head's row names the head's and the eyelashes'), its Skeletons (the part's addons), its
    /// LODSelectors (not placed: the lower levels of the same meshes), its Materials (not applied: no column of the
    /// part row says which generic Material one replaces) and its BuildTables (the materials and addons tables).
    pub meshes: Vec<u64>,
    pub skeletons: Vec<u64>,
    pub selectors: Vec<u64>,
    pub materials: Vec<u64>,
    pub tables: Vec<u64>,
    /// The placements of the meshes, following the rider's rig occurrence.
    pub placements: Vec<Entity>,
    /// The variants the part's materials table lists (empty where it has none): the caller's choices for
    /// [`Variant::Table`].
    pub variants: Vec<VariantRow>,
    /// The Material replacements applied to the placements, where the variant gave any.
    pub variant: Option<AppliedVariant>,
    /// Why the variant isn't applied (the meshes keep their own Materials).
    pub variant_refused: Option<String>,
    /// Why a mesh, or the whole part, isn't placed (one reason per refusal).
    pub refused: Vec<String>,
}

/// A rider placed: its rig, the occurrence of it, and each part.
#[derive(Clone, Debug, PartialEq)]
pub struct RiderPlaced {
    pub rig: Option<Entity>,
    pub occurrence: Option<Entity>,
    pub parts: Vec<RiderPart>,
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

    /// The placer giving each rig it reads every Animation of the forges that fits it ([`AnvilClipCandidates`], on the
    /// rig's entity): the Animations are read once for the first rig, which takes the time and memory of the whole
    /// library. Which of them play is the program's to choose (`select-anvil-clips`).
    pub fn with_clip_candidates(mut self) -> Placer<'a> {
        self.clip_candidates = true;
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
            skeletons: HashMap::new(),
            rigs: HashMap::new(),
            clips: HashMap::new(),
            clip_candidates: false,
            library: None,
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
            shapes_open: Vec::new(),
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
                // a list inside itself, or nested past the limit, is refused and not kept: the shape it is inside is
                // still being read, and answers for itself
                let loops = if self.shapes_open.contains(&id) {
                    Some("a reference list that names itself, or a list it is inside")
                } else if self.shapes_open.len() >= MAX_SHAPE_NESTING {
                    Some("reference lists nested too deep")
                } else {
                    None
                };
                if let Some(why) = loops {
                    let e = Error::ShapeNesting {
                        id,
                        why: why.to_owned(),
                    };
                    placed.refused.push((id, e.to_string()));
                    return Ok(Ok(Vec::new()));
                }
                self.shapes_open.push(id);
                let read = self.read_shape(id, placed);
                self.shapes_open.pop();
                let parts = match read {
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
                        Ok(child_parts) => {
                            if parts.len().saturating_add(child_parts.len()) > MAX_SHAPE_PARTS {
                                return Err(Error::ShapeNesting {
                                    id,
                                    why: format!("places more than {MAX_SHAPE_PARTS} parts"),
                                });
                            }
                            parts.extend(
                                child_parts.into_iter().map(|(e, local)| {
                                    (e, Some(local.map_or(within, |l| within.compose(&l))))
                                }),
                            );
                        }
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
        if mesh.materials.iter().any(|&id| entry_name(self.archive, id)
            .is_some_and(|name| matches!(name.as_str(), "MAD_MAT_FakeForest" | "MAD_MAT_FakeForest_triangle")))
        {
            self.world.insert(e, crate::components::AnvilFarClass::Canopy);
        }
        self.world.insert(e, mesh);
        Ok(e)
    }

    /// The entity holding the Skeleton `id` names, read once: its [`AnvilSkeleton`], named for the entry. `None` where
    /// it isn't a Skeleton or can't be read (recorded in `placed`).
    pub fn skeleton(&mut self, id: u64, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.skeletons.get(&id) {
            return e;
        }
        let e = match self.read_skeleton(id) {
            Ok(e) => e,
            Err(why) => {
                placed.refused.push((id, format!("skeleton: {why}")));
                None
            }
        };
        self.skeletons.insert(id, e);
        e
    }

    /// The entity holding the rig of the SkeletonComponent `sc`, read once ([`AnvilRig`], named for its main Skeleton):
    /// `None` where it names no main Skeleton, or one of its Skeletons isn't one or can't be read (recorded in
    /// `placed`).
    fn rig(&mut self, sc: &SkeletonBinding, placed: &mut Placed) -> Option<Entity> {
        let main = sc.main?;
        let key: Vec<u64> = std::iter::once(main.id)
            .chain(sc.addons.iter().map(|a| a.id))
            .collect();
        self.rig_of(key, placed)
    }

    /// The entity holding the rig of the Skeletons `key` names (the main one first), read once.
    fn rig_of(&mut self, key: Vec<u64>, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.rigs.get(&key) {
            return e;
        }
        let main = key.first().copied().unwrap_or(0);
        let e = match self.read_rig(&key) {
            Ok(e) => {
                if self.clip_candidates {
                    self.fit_clips(e, placed);
                }
                Some(e)
            }
            Err(why) => {
                placed.refused.push((main, format!("rig: {why}")));
                None
            }
        };
        self.rigs.insert(key, e);
        e
    }

    /// The entity holding the Animation `id` names read for the rig `rig` (an [`AnvilClip`], named for its entry), read
    /// once: `None` where it isn't an Animation or can't be read (recorded in `placed`).
    fn clip(&mut self, id: u64, rig: Entity, placed: &mut Placed) -> Option<Entity> {
        if let Some(&e) = self.clips.get(&(id, rig)) {
            return e;
        }
        let e = match self.read_clip(id, rig) {
            Ok(e) => e,
            Err(why) => {
                placed.refused.push((id, format!("clip: {why}")));
                None
            }
        };
        self.clips.insert((id, rig), e);
        e
    }

    fn read_clip(&mut self, id: u64, rig: Entity) -> Result<Option<Entity>, String> {
        let Some(at) = self.archive.find(id) else {
            return Ok(None);
        };
        let r = self.archive.resource(at).map_err(|e| e.to_string())?;
        if r.type_id != ANIMATION {
            return Ok(None);
        }
        let animation = StoredAnimation::parse(&r.body).map_err(|e| e.to_string())?;
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        self.world.insert(
            e,
            AnvilClip {
                frame: self.frame,
                id,
                animation,
                rig,
            },
        );
        Ok(Some(e))
    }

    /// Every Animation of the forges, in the order of the forges' entries, the entry that an id resolves to read once
    /// each (`placed` records one that can't be read).
    fn clip_library(&mut self, placed: &mut Placed) -> Vec<std::sync::Arc<LibraryClip>> {
        if let Some(l) = &self.library {
            return l.clone();
        }
        let mut found: Vec<(At, u64, String)> = Vec::new();
        let mut forge = 0;
        while let Some(f) = self.archive.forge(forge) {
            for (entry, (index, name)) in f.entries().enumerate() {
                let at = At { forge, entry };
                if name.type_id == ANIMATION && self.archive.find(index.id) == Some(at) {
                    found.push((at, index.id, name.name.clone()));
                }
            }
            forge += 1;
        }
        let mut library = Vec::with_capacity(found.len());
        for (at, id, name) in found {
            let animation = self
                .archive
                .resource(at)
                .map_err(|e| e.to_string())
                .and_then(|r| StoredAnimation::parse(&r.body).map_err(|e| e.to_string()));
            match animation {
                Ok(animation) => {
                    let joint_tracks = animation.joint_tracks();
                    library.push(std::sync::Arc::new(LibraryClip {
                        id,
                        name,
                        animation,
                        joint_tracks,
                    }));
                }
                Err(why) => placed.refused.push((id, format!("clip: {why}"))),
            }
        }
        self.library = Some(library.clone());
        library
    }

    /// The rig entity `rig` given the library's clips that fit it: those with a joint-driving track on one of its
    /// joints.
    fn fit_clips(&mut self, rig: Entity, placed: &mut Placed) {
        let library = self.clip_library(placed);
        let Some(stored) = self.world.get::<AnvilRig>(rig) else {
            return;
        };
        let joints: HashSet<u32> = stored.rig.bones.iter().map(|b| b.name_hash).collect();
        let candidates: Vec<ClipCandidate> = library
            .iter()
            .filter_map(|clip| {
                let covered = clip.joint_tracks.iter().filter(|h| joints.contains(h)).count();
                (covered > 0).then(|| ClipCandidate {
                    clip: clip.clone(),
                    covered: u32::try_from(covered).unwrap_or(u32::MAX),
                })
            })
            .collect();
        self.world.insert(
            rig,
            AnvilClipCandidates {
                frame: self.frame,
                candidates,
            },
        );
    }

    fn read_rig(&mut self, key: &[u64]) -> Result<Entity, String> {
        let mut skeletons = Vec::with_capacity(key.len());
        let mut name = None;
        for &id in key {
            let at = self
                .archive
                .find(id)
                .ok_or_else(|| format!("no resource {id:#x} in the forges"))?;
            let r = self.archive.resource(at).map_err(|e| e.to_string())?;
            if r.type_id != SKELETON {
                return Err(format!("{id:#x} is type {:#010x}, not a Skeleton", r.type_id));
            }
            skeletons.push(StoredSkeleton::parse(&r.body).map_err(|e| format!("{id:#x}: {e}"))?);
            name = name.or_else(|| self.name(at));
        }
        let refs: Vec<&StoredSkeleton> = skeletons.iter().collect();
        let rig = StoredRig::assemble(&refs).map_err(|e| e.to_string())?;
        let main = skeletons.first().ok_or("no main Skeleton")?;
        let (position, rotation) = (main.position, main.rotation);
        let e = self.world.spawn();
        if let Some(name) = name {
            self.world.insert(e, Name(name));
        }
        self.world.insert(
            e,
            AnvilRig {
                frame: self.frame,
                position,
                rotation,
                rig,
            },
        );
        Ok(e)
    }

    fn read_skeleton(&mut self, id: u64) -> Result<Option<Entity>, String> {
        let Some(at) = self.archive.find(id) else {
            return Ok(None);
        };
        let r = self.archive.resource(at).map_err(|e| e.to_string())?;
        if r.type_id != SKELETON {
            return Ok(None);
        }
        let skeleton = StoredSkeleton::parse(&r.body).map_err(|e| e.to_string())?;
        let e = self.world.spawn();
        if let Some(name) = self.name(at) {
            self.world.insert(e, Name(name));
        }
        self.world.insert(
            e,
            AnvilSkeleton {
                frame: self.frame,
                skeleton,
            },
        );
        Ok(Some(e))
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

    /// Read an Impostor's own atlas references as well as its card table. They are independent of the Mesh proxy.
    fn impostor(
        &mut self, at: At, id: u64, body: &[u8], placed: &mut Placed,
    ) -> Result<Entity, Error> {
        let stored = StoredImpostor::parse(body).map_err(|why| Error::Impostor { at, why })?;
        let textures = stored.maps.map(|map| map.and_then(|map| self.texture(map.id, placed)));
        let entity = self.prototype(at);
        self.world.insert(entity, AnvilImpostor { index_id: id, frame: self.frame, stored, textures });
        Ok(entity)
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
                    IMPOSTOR => Some(vec![(0, self.impostor(at, id, &r.body, placed)?)]),
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
                                Some((m, r)) if r.type_id == IMPOSTOR => {
                                    match self.impostor(m, reference, &r.body, placed) {
                                        Ok(entity) => {
                                            levels.push((level, entity));
                                            distances.levels.push((level, s.levels.get(usize::from(level))
                                                .map_or(0.0, |l| l.switch_distance())));
                                        }
                                        Err(error) => {
                                            placed.refused.push((reference, error.to_string()));
                                            self.refused_levels.insert((id, level));
                                        }
                                    }
                                }
                                _ => {
                                    placed.refused.push((reference,
                                        format!("LOD {level}: resource is missing or is neither Mesh nor Impostor")));
                                }
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
            if let Some(impostor) = self.world.get::<AnvilImpostor>(own).cloned() {
                self.world.insert(p, impostor);
            }
            if let Some(class) = self.world.get::<crate::components::AnvilFarClass>(own).copied() {
                self.world.insert(p, class);
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
    /// level it names, including authored Impostor levels, as an Entity's Visual without instances places them.
    /// Its Materials are read where the placer reads them
    /// ([`Placer::with_materials`]). `None` where no level of it could be read (recorded in `placed`).
    pub fn place_object(&mut self, at: At, placed: &mut Placed) -> Result<Option<Entity>, Error> {
        self.place_object_following(at, None, placed)
    }

    /// [`Placer::place_object`], the placement following the rig occurrence `occurrence` where one is given
    /// ([`AnvilSkinned`]).
    fn place_object_following(
        &mut self,
        at: At,
        occurrence: Option<Entity>,
        placed: &mut Placed,
    ) -> Result<Option<Entity>, Error> {
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
        if found != MESH && found != LOD_SELECTOR && found != IMPOSTOR {
            return Err(Error::Type {
                at,
                found,
                what: "a Mesh, LODSelector or Impostor",
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
        if let Some(occurrence) = occurrence {
            self.world.insert(p, AnvilSkinned { occurrence });
        }
        placed.placements.push(p);
        Ok(Some(p))
    }

    /// A rider put together from the parts `spec` chooses (see [`RiderSpec`]): each part table's row for the gender
    /// names the part's meshes, its addon Skeleton and the tables of its materials and addons; the rig is the
    /// Skeletons `spec` gives and the parts' addons; each part's meshes are placed following one occurrence of that
    /// rig, with the Materials of the variant the caller chose. A mesh that can't be placed is refused with its
    /// reason in [`RiderPart::refused`] (and in `placed`); nothing is chosen for the caller.
    pub fn place_rider(&mut self, spec: &RiderSpec, placed: &mut Placed) -> RiderPlaced {
        let mut parts: Vec<RiderPart> = spec
            .parts
            .iter()
            .map(|c| self.describe_part(*c, spec.gender))
            .collect();
        let mut key = spec.skeletons.clone();
        for part in &parts {
            for &s in &part.skeletons {
                if !key.contains(&s) {
                    key.push(s);
                }
            }
        }
        let Some(rig) = self.rig_of(key, placed) else {
            for part in &mut parts {
                part.refused.push("the rig can't be read".to_owned());
            }
            return RiderPlaced {
                rig: None,
                occurrence: None,
                parts,
            };
        };
        let joints: HashSet<u32> = self
            .world
            .get::<AnvilRig>(rig)
            .map(|r| r.rig.bones.iter().map(|b| b.name_hash).collect())
            .unwrap_or_default();
        let occurrence = self.world.spawn();
        self.world.insert(occurrence, Name(spec.name.clone()));
        self.world.insert(occurrence, AnvilRigOccurrence { rig });
        self.world
            .insert(occurrence, self.frame.transform(&Transform::IDENTITY));
        for part in &mut parts {
            if !part.refused.is_empty() {
                continue;
            }
            let pairs = part.variant.as_ref().map(|v| v.pairs.clone()).unwrap_or_default();
            for mesh in part.meshes.clone() {
                match self.place_part_mesh(mesh, occurrence, &joints, &pairs, placed) {
                    Ok(Some(p)) => part.placements.push(p),
                    Ok(None) => {}
                    Err(why) => part.refused.push(why),
                }
            }
            for why in &part.refused {
                placed.refused.push((part.table, format!("rider part: {why}")));
            }
            if let Some(why) = &part.variant_refused {
                placed
                    .refused
                    .push((part.table, format!("rider part variant: {why}")));
            }
        }
        RiderPlaced {
            rig: Some(rig),
            occurrence: Some(occurrence),
            parts,
        }
    }

    /// The type of the entry `id` names (`None`: in no forge).
    fn type_of(&self, id: u64) -> Option<u32> {
        let at = self.archive.find(id)?;
        self.archive
            .forge(at.forge)
            .and_then(|f| f.names.get(at.entry))
            .map(|n| n.type_id)
    }

    /// The BuildTable `id` names, and its entry's name.
    fn build_table(&mut self, id: u64) -> Result<(StoredBuildTable, String), String> {
        let at = self.archive.find(id).ok_or("the table is in no forge")?;
        let name = self.name(at).unwrap_or_default();
        let r = self.archive.resource(at).map_err(|e| e.to_string())?;
        if r.type_id != BUILD_TABLE {
            return Err(format!("{id:#x} is type {:#010x}, not a BuildTable", r.type_id));
        }
        Ok((StoredBuildTable::parse(&r.body).map_err(|e| e.to_string())?, name))
    }

    /// What the part `choice` names states for `gender`, without placing anything: its row's meshes, Skeletons,
    /// selectors, Materials and tables, the variants its materials table lists and the replacements the chosen
    /// variant gives (or why it gives none).
    pub fn describe_part(&mut self, choice: RiderPartChoice, gender: Gender) -> RiderPart {
        let mut part = self.resolve_part(choice.table, gender);
        if part.refused.is_empty() {
            part.variants = self.variants_of(&part).unwrap_or_default();
            match self.resolve_variant(&part, choice.variant, gender) {
                Ok(v) => part.variant = v,
                Err(why) => part.variant_refused = Some(why),
            }
        }
        part
    }

    /// What the part table `table` states for `gender`: its row's meshes, Skeletons, selectors, Materials and tables.
    fn resolve_part(&mut self, table: u64, gender: Gender) -> RiderPart {
        let mut part = RiderPart {
            table,
            name: String::new(),
            meshes: Vec::new(),
            skeletons: Vec::new(),
            selectors: Vec::new(),
            materials: Vec::new(),
            tables: Vec::new(),
            placements: Vec::new(),
            variants: Vec::new(),
            variant: None,
            variant_refused: None,
            refused: Vec::new(),
        };
        let found = (|| -> Result<(), String> {
            let (t, name) = self.build_table(table)?;
            part.name = name;
            let mut rows = t.rows_with(gender.condition());
            let (Some(row), None) = (rows.next(), rows.next()) else {
                return Err(format!(
                    "no single row for the condition {:#010x}",
                    gender.condition()
                ));
            };
            for (_, id) in row.links().map_err(|e| e.to_string())? {
                match self.type_of(id) {
                    Some(MESH) => part.meshes.push(id),
                    Some(LOD_SELECTOR) => part.selectors.push(id),
                    Some(SKELETON) => part.skeletons.push(id),
                    Some(MATERIAL) => part.materials.push(id),
                    Some(BUILD_TABLE) => part.tables.push(id),
                    _ => {}
                }
            }
            if part.meshes.is_empty() {
                return Err("its row names no Mesh".to_owned());
            }
            Ok(())
        })();
        if let Err(why) = found {
            part.refused.push(why);
        }
        part
    }

    /// The replacements `variant` gives for `part`: `Ok(None)` for [`Variant::Own`].
    fn resolve_variant(
        &mut self,
        part: &RiderPart,
        variant: Variant,
        gender: Gender,
    ) -> Result<Option<AppliedVariant>, String> {
        let table = match variant {
            Variant::Own => return Ok(None),
            Variant::Table(t) => t,
            Variant::Stated => self.stated_variant(part)?,
            Variant::Row { table, .. } => table,
        };
        let (t, _) = self.build_table(table)?;
        let condition = match variant {
            Variant::Row { condition, .. } => condition,
            _ => gender.condition(),
        };
        let mut rows = t.rows_with(condition);
        let (Some(row), None) = (rows.next(), rows.next()) else {
            return Err(format!(
                "the variant table has no single row for the condition {condition:#010x}"
            ));
        };
        let links = row.links().map_err(|e| e.to_string())?;
        let mut pairs = Vec::new();
        for c in &t.columns {
            let Some(generic) = c.generic_material().map_err(|e| e.to_string())? else {
                continue;
            };
            let number = c.fields[0];
            if let Some((_, effective)) = links.iter().find(|(column, _)| *column == number) {
                pairs.push((generic, *effective));
            }
        }
        if pairs.is_empty() {
            return Err("the variant table's row replaces no Material".to_owned());
        }
        Ok(Some(AppliedVariant { table, pairs }))
    }

    /// The variants the part's materials table lists ([`part_variants`] of the tables its row links).
    fn variants_of(&mut self, part: &RiderPart) -> Result<Vec<VariantRow>, String> {
        part_variants(self.archive, &part.tables).map_err(|e| e.to_string())
    }

    /// The variant table the part's materials table states: the one row of weight exactly 1.0.
    fn stated_variant(&mut self, part: &RiderPart) -> Result<u64, String> {
        if part.variants.is_empty() {
            return Err("the part has no single materials table listing variant tables".to_owned());
        }
        stated(&part.variants)
            .map(|v| v.table)
            .ok_or_else(|| "its materials table states no single variant of weight 1.0".to_owned())
    }

    /// The placement of the mesh `mesh` following `occurrence`, once every bone a vertex weights is a joint of the
    /// rig (`joints`), its Materials replaced by `pairs` (generic, effective).
    fn place_part_mesh(
        &mut self,
        mesh: u64,
        occurrence: Entity,
        joints: &HashSet<u32>,
        pairs: &[(u64, u64)],
        placed: &mut Placed,
    ) -> Result<Option<Entity>, String> {
        let at = self.archive.find(mesh).ok_or("its Mesh is in no forge")?;
        let r = self.archive.resource(at).map_err(|e| e.to_string())?;
        let stored = StoredMesh::parse(&r.body).map_err(|e| e.to_string())?;
        let bones = stored.bones().map_err(|e| e.to_string())?;
        if let Some(skin) = stored.skin().map_err(|e| e.to_string())? {
            let weighted = skin.weighted_slots(bones.len());
            let missing: Vec<u32> = bones
                .iter()
                .zip(&weighted)
                .filter(|(b, w)| **w && !joints.contains(&b.bone_id))
                .map(|(b, _)| b.bone_id)
                .collect();
            if let Some(first) = missing.first() {
                return Err(format!(
                    "its Mesh weights {} bones the rig has no joint for (the first {first:08x})",
                    missing.len()
                ));
            }
        }
        let Some(p) = self
            .place_object_following(at, Some(occurrence), placed)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        if !pairs.is_empty() {
            self.replace_materials(p, pairs, placed);
        }
        Ok(Some(p))
    }

    /// The placement `p` drawn with the Materials `pairs` (generic, effective) replace: its prototype copied, each
    /// range whose Material is a generic one of a pair taking the effective one (read where the placer reads
    /// Materials).
    fn replace_materials(&mut self, p: Entity, pairs: &[(u64, u64)], placed: &mut Placed) {
        let Some(prototype) = self.world.get::<Instance>(p).map(|i| i.prototype) else {
            return;
        };
        let Some(copy) = self.world.duplicate(prototype) else {
            return;
        };
        let Some(mut mesh) = self.world.get::<AnvilMesh>(copy).cloned() else {
            return;
        };
        for (k, id) in mesh.materials.clone().into_iter().enumerate() {
            let Some(&(_, effective)) = pairs.iter().find(|(generic, _)| *generic == id) else {
                continue;
            };
            if let Some(slot) = mesh.materials.get_mut(k) {
                *slot = effective;
            }
            if self.read_materials {
                let e = self.material(effective, placed);
                if let Some(slot) = mesh.material_entities.get_mut(k) {
                    *slot = e;
                }
            }
        }
        self.world.insert(copy, mesh);
        self.prototypes.insert(copy);
        self.world.insert(
            p,
            Instance {
                prototype: copy,
                variant: 0,
            },
        );
        self.release_prototype(prototype);
    }

    /// The prototype `prototype` removed when no placement uses it any more (a placement drawn with a copy of it left
    /// it with none, and a prototype nothing places is a mesh that is drawn nowhere): the entries of the caches that
    /// name it go too, so that a later placement of the same object reads it again.
    fn release_prototype(&mut self, prototype: Entity) {
        if self
            .world
            .iter::<Instance>()
            .any(|(_, i)| i.prototype == prototype)
            || self.forms.contains(&prototype)
        {
            return;
        }
        self.wholes.retain(|_, e| *e != prototype);
        self.objects
            .retain(|_, levels| !levels.iter().flatten().any(|(_, e)| *e == prototype));
        self.prototypes.remove(&prototype);
        self.prototype_copies
            .retain(|k, v| *k != prototype && *v != prototype);
        self.distance_copies
            .retain(|(e, _), v| *e != prototype && *v != prototype);
        self.world.despawn(prototype);
    }

    /// The Entity or EntityGroup at `at` placed into the world: one placement per Visual with something to show, or per
    /// instance. A group's own Visuals place as an Entity's (it is an Entity's body and its member list).
    ///
    /// An error refuses the owner: its body, or how its Visuals' instances are stored, can't be read as placed. An
    /// object it shows or a shape its RigidBody names that can't be read is recorded in [`Placed::refused`] instead,
    /// and only what needs it is left out.
    pub fn place(&mut self, at: At, placed: &mut Placed) -> Result<(), Error> {
        let starts = [placed.placements.len(), placed.collisions.len(), placed.lakes.len(),
            placed.deferred_decals.len(), placed.grass.len()];
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
            match &c.body {
                ComponentBody::Unread(_) => *placed.unread_components.entry(c.kind).or_default() += 1,
                ComponentBody::Refused { why, .. } => {
                    placed
                        .refused_components
                        .entry((c.kind, why.reason()))
                        .or_insert_with(|| Refusals {
                            count: 0,
                            first: why.clone(),
                        })
                        .count += 1;
                }
                _ => {}
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
        // a GrassVisual shows its model's Mesh once, at the owner's matrix: a Visual of that Mesh, marked below
        let grass: Vec<(Visual, &crate::read::entity::GrassVisual)> = e
            .components
            .iter()
            .filter_map(|c| match &c.body {
                ComponentBody::GrassVisual(g) => Some((
                    Visual {
                        head: g.head,
                        object: g.object,
                        instance: Shown::Mesh(g.instance.clone()),
                        tail: g.tail.clone(),
                    },
                    g,
                )),
                _ => None,
            })
            .collect();
        // a SkeletonComponent's rig, placed once at the owner's transform: its Visuals follow it, and the Animations of
        // its AnimComponent play on it
        let rig = e
            .components
            .iter()
            .find_map(|c| match &c.body {
                ComponentBody::Skeleton(sc) => Some(sc),
                _ => None,
            })
            .and_then(|sc| self.rig(sc, placed));
        let occurrence = rig.map(|rig| {
            let o = self.world.spawn();
            self.world.insert(o, Name(name.clone()));
            self.world.insert(o, AnvilRigOccurrence { rig });
            self.world
                .insert(o, self.frame.transform(&transform(&e.matrix, e.scale)));
            o
        });
        if let (Some(rig), Some(occurrence)) = (rig, occurrence) {
            let animations = e.components.iter().find_map(|c| match &c.body {
                ComponentBody::Anim(a) => Some(&a.animations),
                _ => None,
            });
            for animation in animations.into_iter().flatten().flatten() {
                if let Some(clip) = self.clip(animation.id, rig, placed) {
                    let b = self.world.spawn();
                    self.world.insert(b, Name(name.clone()));
                    self.world.insert(b, AnvilClipBinding { clip, occurrence });
                }
            }
        }
        for (v, decal, grass_model) in e
            .visuals()
            .map(|v| (v, false, None))
            .chain(e.decals().map(|v| (v, true, None)))
            .chain(grass.iter().map(|(v, g)| (v, false, Some(*g))))
        {
            if let Shown::Raw { kind: crate::read::entity::LIGHT_INSTANCE, bytes, id } = &v.instance {
                let read = (|| -> Result<(u64, crate::read::light::StoredSpotLight), String> {
                    let id = crate::read::light::instance_reference(bytes)?;
                    let at = self.archive.find(id).ok_or("light reference is absent from the archive")?;
                    let resource = self.archive.resource(at).map_err(|e| e.to_string())?;
                    if resource.type_id != crate::read::light::SPOT_LIGHT {
                        return Err(format!("light {id:#x} is type {:#x}, not SpotLight", resource.type_id));
                    }
                    Ok((id, crate::read::light::StoredSpotLight::parse(&resource.body)?))
                })();
                match read {
                    Ok((index_id, stored)) => {
                        let light = self.world.spawn();
                        self.world.insert(light, Name(name.clone()));
                        self.world.insert(light, self.frame.transform(&transform(&e.matrix, e.scale)));
                        self.world.insert(light, crate::components::AnvilSpotLight { index_id, stored });
                        if let Some(layers) = &layers { self.world.insert(light, layers.clone()); }
                        if let Some(state) = state { self.world.insert(light, state); }
                        self.world.insert(light, bounds);
                        placed.placements.push(light);
                    }
                    Err(why) => placed.refused.push((*id, why)),
                }
                continue;
            }
            if let Shown::Raw { kind, id, .. } = &v.instance {
                let marker = self.world.spawn();
                self.world.insert(marker, Name(name.clone()));
                self.world.insert(marker, self.frame.transform(&transform(&e.matrix, e.scale)));
                let target = v.object.and_then(|r| entry_name(self.archive, r.id));
                self.world.insert(marker, crate::components::AnvilRenderStatus {
                    source: format!("Visual instance {id:#x}"), template: None, unsupported: true,
                    reason: format!("Visual instance class {kind:#010x}, target {target:?}: runtime-generated visual; no static geometry decoded"),
                });
                if let Some(layers) = &layers { self.world.insert(marker, layers.clone()); }
                if let Some(state) = state { self.world.insert(marker, state); }
                self.world.insert(marker, bounds);
                placed.placements.push(marker);
                continue;
            }
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
                        .map(|i| entity.compose(&transform(i, 1.0)))
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
                if let Some(occurrence) = occurrence {
                    self.world.insert(p, AnvilSkinned { occurrence });
                }
                if let Some(g) = grass_model {
                    self.world.insert(
                        p,
                        AnvilGrassModel {
                            model: g.model,
                            ranges: g.ranges.clone(),
                        },
                    );
                }
                visual_placements = visual_placements.saturating_add(1);
                placed.placements.push(p);
                if decal {
                    placed.static_decals = placed.static_decals.saturating_add(1);
                }
            }
        }
        // Runtime effects retain a spatial, named explanation rather than becoming a silently empty catalog row.
        // Their emission timing and property programs are not static geometry, so no particle picture is invented.
        for component in &e.components {
            if !matches!(component.kind, 0xD2BF_93A6 | 0x713F_0C7D) { continue; }
            let marker = self.world.spawn();
            let class = crate::read::component_types::component_name(component.kind).unwrap_or("effect");
            self.world.insert(marker, Name(format!("{name}: {class}")));
            self.world.insert(marker, self.frame.transform(&transform(&e.matrix, e.scale)));
            self.world.insert(marker, primase_core::Marker { kind: "effect-emitter".into(), ground: None, yaw: None });
            self.world.insert(marker, crate::components::AnvilRenderStatus {
                source: format!("component {:#x}", component.id), template: None, unsupported: true,
                reason: format!("{class}: runtime emitter/property program; marked at its authored transform; particle simulation and FX property evaluation unavailable"),
            });
            if let Some(layers) = &layers { self.world.insert(marker, layers.clone()); }
            if let Some(state) = state { self.world.insert(marker, state); }
            self.world.insert(marker, bounds);
            placed.placements.push(marker);
        }
        // the grass setup: the GrassComponent as stored and the library it names, on an entity of its own
        for c in &e.components {
            let ComponentBody::GrassSetup(setup) = &c.body else {
                continue;
            };
            let Some(library) = setup.library else {
                placed
                    .refused
                    .push((c.id, "GrassComponent names no library".to_owned()));
                continue;
            };
            let parsed = self
                .archive
                .find(library.id)
                .ok_or_else(|| "the library is in no forge".to_owned())
                .and_then(|lat| self.archive.resource(lat).map_err(|e| e.to_string()))
                .and_then(|r| {
                    if r.type_id == GRASS_PATTERN_LIBRARY {
                        StoredGrassLibrary::parse(&r.body).map_err(|e| e.to_string())
                    } else {
                        Err(format!("type {:#010x}, not a GrassPatternLibrary", r.type_id))
                    }
                });
            match parsed {
                Ok(library) => {
                    let g = self.world.spawn();
                    self.world.insert(g, Name(name.clone()));
                    self.world.insert(
                        g,
                        AnvilGrass {
                            setup: GrassSystemSetup {
                                id: setup.id,
                                library: setup.library.map(|r| r.id),
                                settings_id: setup.settings_id,
                                visibility_distance: setup.visibility_distance(),
                                settings: setup.settings.clone(),
                                entities: setup.entities.clone(),
                            },
                            library,
                        },
                    );
                    if let Some(layers) = &layers {
                        self.world.insert(g, layers.clone());
                    }
                    if let Some(s) = state {
                        self.world.insert(g, s);
                    }
                    placed.grass.push(g);
                }
                Err(why) => placed.refused.push((library.id, format!("grass library: {why}"))),
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
        let mut sets = Vec::new();
        for v in e.visuals() {
            if let Some((_, i)) = shows(v, at)?.instances {
                sets.push(i);
            }
        }
        let mut instance_sets = sets.into_iter();
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
                    .map(|i| owner.compose(&transform(i, 1.0)))
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
                    let t = local.map_or(owner, |l| owner.compose(&l));
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
        // Preserve bounded opaque component bodies once per source owner, not once per mesh instance.
        // Water/decal Raw bodies already have typed readers above; other Raw bodies only had length rules.
        let components: Vec<_> = e.components.iter().filter_map(|c| {
            let (bytes, status) = match &c.body {
                ComponentBody::Raw(bytes) if !matches!(c.kind, WATER_COMPONENT | HOUDINI_DECAL_INSTANCER | STATIC_DDECAL) =>
                    (bytes, "length-rule-only; semantic fields not decoded".to_owned()),
                ComponentBody::Unread(bytes) => (bytes, "unread; extent delimited by component headers".to_owned()),
                ComponentBody::Refused { bytes, why } => (bytes, format!("refused: {}", why.reason())),
                _ => return None,
            };
            Some(crate::components::SourceComponent { type_id: c.kind, id: c.id, status, bytes: bytes.clone() })
        }).collect();
        if !components.is_empty() {
            let lists = [&placed.placements, &placed.collisions, &placed.lakes, &placed.deferred_decals, &placed.grass];
            let occurrences: Vec<_> = lists.into_iter().zip(starts)
                .flat_map(|(list, start)| list.iter().skip(start).copied()).collect();
            let target = self.world.spawn();
            self.world.insert(target, Name(format!("{name} source components")));
            self.world.insert(target, self.frame.transform(&transform(&e.matrix, e.scale)));
            self.world.insert(target, bounds);
            if let Some(layers) = &layers { self.world.insert(target, layers.clone()); }
            if let Some(state) = state { self.world.insert(target, state); }
            if occurrences.is_empty() {
                self.world.insert(target, crate::components::AnvilRenderStatus {
                    source: format!("Entity {:#x}", r.class_id), template: None, unsupported: true,
                    reason: "source components retained in anvil.source_components; no static visual decoded for this owner".into(),
                });
            }
            // Include the source record in listed/state selection, and retain it through each occurrence's closure.
            placed.placements.push(target);
            for occurrence in occurrences {
                self.world.insert(occurrence, crate::components::AnvilSourceOwner { source: target });
            }
            self.world.insert(target, crate::components::AnvilSourceComponents { owner: r.class_id, components });
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "fixtures"
)]
mod shape_tests {
    use super::*;
    use crate::read::shape::{REFERENCE_LIST_SHAPE, SPHERE_SHAPE};

    fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A sphere shape's body: its centre and radius, and a material reference.
    fn sphere() -> Vec<u8> {
        let mut b = vec![1, 0, 0, 0, 0];
        b.extend(floats(&[1.0, 2.0, 3.0, 0.0, 0.5]));
        b.extend([1, 1, 0x1D, 0x31, 0xF7, 0x74, 0, 0, 0, 0]);
        b
    }

    /// A reference list's body: one child at the identity for each id.
    fn list(children: &[u64]) -> Vec<u8> {
        let n = u32::try_from(children.len()).unwrap();
        let mut b = vec![1u8];
        b.extend(0u32.to_le_bytes());
        b.extend(n.to_le_bytes());
        for c in children {
            b.extend([1, 1]);
            b.extend(c.to_le_bytes());
        }
        b.extend(n.to_le_bytes());
        for _ in children {
            for k in 0..16 {
                b.extend((if k % 5 == 0 { 1.0f32 } else { 0.0 }).to_le_bytes());
            }
        }
        b.extend(n.to_le_bytes());
        for _ in children {
            b.extend(floats(&[0.0; 4]));
        }
        b.extend(floats(&[0.0; 8]));
        b
    }

    /// A data block of `data` as a forge stores it, the chunk left as it is.
    fn block(data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(0x1004_FA99_57FB_AA33u64.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.push(5);
        b.extend([0u8; 4]);
        b.extend(1u32.to_le_bytes());
        b.extend((data.len() as u32).to_le_bytes());
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(primase_hash::adler32(0, data).to_le_bytes());
        b.extend(data);
        b
    }

    /// An entry's payload: an empty metadata block and the resource of type `ty` with `body`.
    fn payload(ty: u32, body: &[u8]) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend(ty.to_le_bytes());
        r.extend(u32::try_from(12 + body.len()).unwrap().to_le_bytes());
        r.extend(0u32.to_le_bytes());
        r.push(0);
        r.extend(0u64.to_le_bytes());
        r.extend(0u32.to_le_bytes());
        r.extend(body);
        [block(&[]), block(&r)].concat()
    }

    /// A version 27 forge holding `entries` (index id, type, payload) as `crate::read::forge`'s reader lays it.
    fn forge(entries: &[(u64, u32, Vec<u8>)]) -> Vec<u8> {
        const INDEX_TABLE: usize = 1142;
        let n = entries.len();
        let names = INDEX_TABLE + 20 * (n + 1);
        let end = names + 192 * (n + 1);
        let mut b = vec![0u8; end];
        let put = |b: &mut Vec<u8>, at: usize, bytes: &[u8]| b[at..at + bytes.len()].copy_from_slice(bytes);
        put(&mut b, 0, b"scimitar\0");
        put(&mut b, 0x09, &27i32.to_le_bytes());
        put(&mut b, 0x0D, &1050u64.to_le_bytes());
        put(&mut b, 0x15, &16i64.to_le_bytes());
        put(&mut b, 0x1D, &1i32.to_le_bytes());
        put(&mut b, 1050, &(n as i32).to_le_bytes());
        put(&mut b, 1054, &2i32.to_le_bytes());
        put(&mut b, 1070, &(-1i64).to_le_bytes());
        put(&mut b, 1078, &(n as i32 + 1).to_le_bytes());
        put(&mut b, 1082, &1i32.to_le_bytes());
        put(&mut b, 1086, &1094i64.to_le_bytes());
        put(&mut b, 1094, &(n as i32).to_le_bytes());
        put(&mut b, 1098, &2i32.to_le_bytes());
        put(&mut b, 1102, &(INDEX_TABLE as i64).to_le_bytes());
        put(&mut b, 1110, &(-1i64).to_le_bytes());
        put(&mut b, 1118, &0i32.to_le_bytes());
        put(&mut b, 1122, &(n as i32).to_le_bytes());
        put(&mut b, 1126, &(names as i64).to_le_bytes());
        put(&mut b, 1134, &(end as i64).to_le_bytes());
        let mut at = end;
        for (k, (id, ty, data)) in entries.iter().enumerate() {
            let i = INDEX_TABLE + 20 * k;
            put(&mut b, i, &(at as i64).to_le_bytes());
            put(&mut b, i + 8, &id.to_le_bytes());
            put(&mut b, i + 16, &(data.len() as i32).to_le_bytes());
            let m = names + 192 * k;
            put(&mut b, m, &(data.len() as i32).to_le_bytes());
            put(
                &mut b,
                m + 4,
                &(0x5E5E_0000_0000_0000u64 + k as u64 + 1).to_le_bytes(),
            );
            put(&mut b, m + 16, &ty.to_le_bytes());
            put(&mut b, m + 28, &(k as i32 + 1).to_le_bytes());
            put(&mut b, m + 40, &1_789_743_211i32.to_le_bytes());
            put(&mut b, m + 44, format!("shape{k}").as_bytes());
            put(&mut b, m + 172, &4i32.to_le_bytes());
            at += data.len();
        }
        for (_, _, data) in entries {
            b.extend(data);
        }
        b
    }

    /// What `shape(id)` places and what was refused, over a forge of `shapes` (id, type, body).
    #[allow(
        clippy::disallowed_methods,
        reason = "test I/O: the archive opens a forge from a path"
    )]
    fn read(test: &str, shapes: &[(u64, u32, Vec<u8>)], id: u64) -> (Result<Parts, u32>, Vec<String>) {
        let entries: Vec<(u64, u32, Vec<u8>)> = shapes
            .iter()
            .map(|(id, ty, body)| (*id, *ty, payload(*ty, body)))
            .collect();
        let dir = std::env::temp_dir().join(format!("primase-anvil-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shapes.forge");
        std::fs::write(&path, forge(&entries)).unwrap();
        let mut archive = crate::read::archive::Archive::open(&[&path]).unwrap();
        let mut world = World::new();
        let mut placer = Placer::new(
            &mut archive,
            &mut world,
            primase_core::frame::SourceFrame::WORLD,
            InstancedLevels::Every,
        );
        let mut placed = Placed::default();
        let parts = placer.shape(id, &mut placed).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        (parts, placed.refused.into_iter().map(|(_, why)| why).collect())
    }

    /// A list of lists of spheres places the spheres once each, the lists' transforms composed.
    #[test]
    fn nested_lists_place_their_spheres() {
        let shapes = [
            (10, REFERENCE_LIST_SHAPE, list(&[20, 20])),
            (20, REFERENCE_LIST_SHAPE, list(&[30, 30])),
            (30, SPHERE_SHAPE, sphere()),
        ];
        let (parts, refused) = read("nested", &shapes, 10);
        assert_eq!(parts.unwrap().len(), 4);
        assert!(refused.is_empty(), "{refused:?}");
    }

    /// A list that names itself, or two that name each other, is refused where the loop closes: the shapes are read, not
    /// recursed through until the stack ends.
    #[test]
    fn a_list_naming_itself_or_a_list_it_is_inside_is_refused() {
        let own = [(10, REFERENCE_LIST_SHAPE, list(&[10]))];
        let (parts, refused) = read("self", &own, 10);
        assert!(parts.unwrap().is_empty());
        assert!(refused.iter().any(|r| r.contains("names itself")), "{refused:?}");
        let pair = [
            (10, REFERENCE_LIST_SHAPE, list(&[20, 30])),
            (20, REFERENCE_LIST_SHAPE, list(&[10])),
            (30, SPHERE_SHAPE, sphere()),
        ];
        let (parts, refused) = read("pair", &pair, 10);
        // the loop adds nothing; the sphere beside it is placed
        assert_eq!(parts.unwrap().len(), 1);
        assert!(refused.iter().any(|r| r.contains("names itself")), "{refused:?}");
    }

    /// Lists that each name the next twice make 2^depth parts: refused at the nesting limit. And a flat pair of levels
    /// of a few hundred each, which is past the limit of parts, is refused by that.
    #[test]
    fn lists_that_share_their_children_are_bounded() {
        let mut deep: Vec<(u64, u32, Vec<u8>)> = (0..20u64)
            .map(|k| (100 + k, REFERENCE_LIST_SHAPE, list(&[101 + k, 101 + k])))
            .collect();
        deep.push((120, SPHERE_SHAPE, sphere()));
        let (parts, refused) = read("deep", &deep, 100);
        assert!(
            refused.iter().any(|r| r.contains("nested too deep")),
            "{refused:?}"
        );
        // the nesting refusal empties the list it is in, so the answer is small whatever was nested
        assert!(parts.unwrap().len() < 1 << 16);
        let wide = [
            (10, REFERENCE_LIST_SHAPE, list(&[20; 300])),
            (20, REFERENCE_LIST_SHAPE, list(&[30; 300])),
            (30, SPHERE_SHAPE, sphere()),
        ];
        let (parts, refused) = read("wide", &wide, 10);
        assert!(parts.unwrap().is_empty());
        assert!(
            refused.iter().any(|r| r.contains("places more than 65536 parts")),
            "{refused:?}"
        );
    }
}
