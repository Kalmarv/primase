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
//! Levels referring to an Impostor are counted, not placed.

use std::collections::HashMap;

use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Name, Transform, World};

use crate::components::{AnvilDataLayers, AnvilLods, AnvilMaterialOverrides, AnvilMesh};
use crate::read::archive::{Archive, At};
use crate::read::entity::{
    ENTITY, ENTITY_GROUP, Instance as Shown, MeshInstance, Slot, StoredEntity, StoredEntityGroup, Visual,
};
use crate::read::lod::{LOD_SELECTOR, StoredLodSelector};
use crate::read::mesh::{MESH, StoredMesh};
use crate::read::meshes::anvil_mesh;

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
}

/// What an instanced Visual places at each instance: the caller's stated choice, as whether the game draws an
/// object's other levels at its instances isn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstancedLevels {
    /// The level whose slot holds the matrices; the object's other mesh levels aren't placed
    /// ([`Placed::levels_unplaced`]).
    Slot,
    /// For an Entity, every mesh level, as for a Visual without instances. The census finds each Entity
    /// instance's stored bounds equal to its matrix (then the Entity's rotation) applied to the
    /// LODSelectorInstance's bounds within 1e-3 on all 6,422,618 (2 bit-equal), and applied to the slot's
    /// MeshInstanceData bounds (its finest level's, by the notes) off by more than 0.1, up to 10, on 2,842,279;
    /// the LODSelectorInstance's bounds are the union of every level's mesh bounds (the notes: 400 of 400). An
    /// EntityGroup's instance bounds don't fit its LODSelectorInstance's bounds (off by more than 0.1, up to 1,000,
    /// on all 241,747), so a group's instanced Visual places as for [`InstancedLevels::Slot`].
    Selector,
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
    /// The member references of the EntityGroups placed, in order. Members aren't placed through their group: each is
    /// an Entity or EntityGroup of its own, which the caller places where it lists it.
    pub members: Vec<u64>,
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
        Shown::Raw { .. } => Vec::new(),
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
    })
}

/// Places Entities, keeping each object's meshes and prototypes for every Entity that shows it.
#[derive(Debug)]
pub struct Placer<'a> {
    archive: &'a mut Archive,
    frame: SourceFrame,
    instanced: InstancedLevels,
    /// Per object id: its mesh levels (`None`: not a LODSelector or Mesh, none of its levels a Mesh, or in no forge).
    objects: HashMap<u64, Option<Levels>>,
    /// Per object id: the prototype placing every mesh level.
    wholes: HashMap<u64, Entity>,
}

impl<'a> Placer<'a> {
    pub fn new(archive: &'a mut Archive, frame: SourceFrame, instanced: InstancedLevels) -> Placer<'a> {
        Placer {
            archive,
            frame,
            instanced,
            objects: HashMap::new(),
            wholes: HashMap::new(),
        }
    }

    /// The name of the entry at `at`, if its forge names it.
    fn name(&self, at: At) -> Option<String> {
        self.archive
            .forge(at.forge)
            .and_then(|f| f.names.get(at.entry))
            .map(|n| n.name.clone())
    }

    /// An entity holding the AnvilMesh of the Mesh at `at`, whose body is `body` and ClassID `id`.
    fn mesh(&mut self, w: &mut World, at: At, body: &[u8], id: u64) -> Result<Entity, Error> {
        let stored = StoredMesh::parse(body).map_err(|source| Error::Mesh { at, source })?;
        let e = w.spawn();
        if let Some(name) = self.name(at) {
            w.insert(e, Name(name));
        }
        w.insert(e, anvil_mesh(&stored, at.entry, id, self.frame)?);
        Ok(e)
    }

    /// The mesh levels of the object `id` names, read once.
    fn levels(&mut self, w: &mut World, id: u64, placed: &mut Placed) -> Result<Option<Levels>, Error> {
        if let Some(levels) = self.objects.get(&id) {
            return Ok(levels.clone());
        }
        let levels = match self.archive.find(id) {
            None => None,
            Some(at) => {
                let r = self.archive.resource(at)?;
                match r.type_id {
                    MESH => Some(vec![(0, self.mesh(w, at, &r.body, id)?)]),
                    LOD_SELECTOR => {
                        let s =
                            StoredLodSelector::parse(&r.body).map_err(|source| Error::Lod { at, source })?;
                        let mut levels = Vec::new();
                        for (level, l) in (0u8..).zip(&s.levels) {
                            let Some(reference) = l.reference else {
                                continue;
                            };
                            let mesh = match self.archive.find(reference.id) {
                                Some(m) => Some((m, self.archive.resource(m)?)),
                                None => None,
                            };
                            match mesh {
                                Some((m, r)) if r.type_id == MESH => {
                                    levels.push((level, self.mesh(w, m, &r.body, reference.id)?));
                                }
                                _ => placed.impostor_levels = placed.impostor_levels.saturating_add(1),
                            }
                        }
                        (!levels.is_empty()).then_some(levels)
                    }
                    _ => None,
                }
            }
        };
        self.objects.insert(id, levels.clone());
        Ok(levels)
    }

    /// The prototype placing every one of `levels` (the object `id`'s): the finest's mesh entity where it is the only
    /// one, else an entity holding a copy of its AnvilMesh and the others as [`AnvilLods`].
    fn whole(&mut self, w: &mut World, id: u64, levels: &[(u8, Entity)]) -> Option<Entity> {
        if let Some(&p) = self.wholes.get(&id) {
            return Some(p);
        }
        let (&(detail, own), forms) = levels.split_first()?;
        let p = if forms.is_empty() {
            own
        } else {
            let p = w.spawn();
            if let Some(name) = self.archive.find(id).and_then(|at| self.name(at)) {
                w.insert(p, Name(name));
            }
            if let Some(mesh) = w.get::<AnvilMesh>(own).cloned() {
                w.insert(p, mesh);
            }
            w.insert(
                p,
                AnvilLods {
                    detail,
                    forms: forms.to_vec(),
                },
            );
            p
        };
        self.wholes.insert(id, p);
        Some(p)
    }

    /// The Entity or EntityGroup at `at` placed into `w`: one placement per Visual with something to show, or per
    /// instance. A group's own Visuals place as an Entity's (it is an Entity's body and its member list).
    pub fn place(&mut self, w: &mut World, at: At, placed: &mut Placed) -> Result<(), Error> {
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
        let name = self.name(at).unwrap_or_default();
        let layers = (!e.data_layers.is_empty()).then(|| AnvilDataLayers {
            actions: e.data_layers.clone(),
        });
        for v in e.visuals() {
            let shows = shows(v, at)?;
            let found = match v.object {
                Some(object) => self.levels(w, object.id, placed)?.map(|l| (object.id, l)),
                None => None,
            };
            let Some((object, levels)) = found else {
                placed.visuals_skipped = placed.visuals_skipped.saturating_add(1);
                continue;
            };
            let (prototype, transforms, overrides) = match shows.instances {
                None => {
                    let Some(p) = self.whole(w, object, &levels) else {
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
                        return Err(Error::Slot(at, level));
                    };
                    let every = self.instanced == InstancedLevels::Selector && !group;
                    let (prototype, overrides) = match (every, levels.first()) {
                        (true, Some(&(finest, _))) if finest == level => {
                            (self.whole(w, object, &levels).unwrap_or(mesh), shows.overrides)
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
                            (mesh, own)
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
            for t in transforms {
                let p = w.spawn();
                w.insert(p, Name(name.clone()));
                w.insert(
                    p,
                    Instance {
                        prototype,
                        variant: 0,
                    },
                );
                w.insert(p, self.frame.transform(&t));
                if !overrides.is_empty() {
                    w.insert(
                        p,
                        AnvilMaterialOverrides {
                            levels: overrides.clone(),
                        },
                    );
                }
                if let Some(layers) = &layers {
                    w.insert(p, layers.clone());
                }
                placed.placements.push(p);
            }
        }
        Ok(())
    }
}
