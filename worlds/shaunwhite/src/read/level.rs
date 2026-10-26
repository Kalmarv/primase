//! A level read into a World: every Entity of the level forge's cells placed with the meshes its Visuals show.
//!
//! The level forge's entries are read to their objects ([`primase_anvil::read::objects`]); further forges given
//! (the shared props other levels' forges hold) are indexed too, so a Visual naming a Mesh outside the level's own
//! cells finds it. Each Entity of a GridCellDataBlock entry becomes one placement per drawn Visual: a Mesh shown
//! directly, or a LODSelector's nearest level (its first descriptor: every level is kept by the source, and the
//! reader places the nearest one, which a selector draws up close). Visuals showing anything else (lights, effects)
//! are counted in the report, not placed. A Mesh is read once, as a prototype, in the source's frame converted to
//! core's (`SourceFrame::ENU`: x east, y north, z up, metres); its primitives take one Material entity per Material id
//! (`shaunwhite.material`: the id, the object's name and its texture ids), which [`crate::read::level`]'s caller
//! lowers (the textures it names are read by [`crate::read::texture`]).
//! Inert and MultiInert components also place collision-only prototypes (`shaunwhite.collision`): ordinary shapes
//! at their entity, multi shapes at their contained entities. The caller runs `lower-shaunwhite-collision` to index
//! source surface names and obtain core collision. Unknown bindings/shapes are named in the report.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the forge files the caller names"
)]

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use primase_anvil::read::data::read_entry;
use primase_anvil::read::forge::Forge;
use primase_anvil::read::objects::{StoredObject, read_objects};
use primase_core::convert::Report;
use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Marker, Mesh, Name, Transform, World};

use crate::components::{MaterialRef, MaterialState, MeshFormat, PlacedBy, Sampler, SlotRef};
use crate::read::markers;
use crate::read::material::{self, MATERIAL, Material};
use crate::read::mesh::{CompiledMesh, MESH};
use crate::read::scene::{self, GRID_CELL_DATA_BLOCK, LOD_SELECTOR};
use crate::read::texture::{TEXTURE_MAP_SPEC, TextureMap};

/// The most bytes a block may declare decoded (the largest entry seen decodes to about 38 MB).
const MAX_DECODED: usize = 1 << 30;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One decoded entry: its name, its objects and its resource block.
struct Decoded {
    name: String,
    objects: Vec<StoredObject>,
    resource: Vec<u8>,
}

/// Every object of the forges read, by id: which entry, which object.
#[derive(Default)]
pub(super) struct Index {
    entries: Vec<Decoded>,
    by_id: BTreeMap<u32, (usize, usize)>,
}

impl Index {
    fn add(&mut self, path: &Path) -> Result<()> {
        let f = Forge::open(path)?;
        let mut r = File::open(path)?;
        for (k, n) in f.names.iter().enumerate() {
            if n.name == "GlobalMetaFile" {
                continue;
            }
            let e = read_entry(&f.payload(&mut r, k)?, MAX_DECODED)?;
            let objects = read_objects(&e.metadata.data, &e.resource.data)?;
            let at = self.entries.len();
            for (i, o) in objects.iter().enumerate() {
                self.by_id.entry(o.id).or_insert((at, i));
            }
            self.entries.push(Decoded {
                name: n.name.clone(),
                objects,
                resource: e.resource.data,
            });
        }
        Ok(())
    }

    pub(super) fn get(&self, id: u32) -> Option<(&StoredObject, &[u8])> {
        let (e, i) = *self.by_id.get(&id)?;
        let d = self.entries.get(e)?;
        let o = d.objects.get(i)?;
        Some((o, d.resource.get(o.body.clone())?))
    }
}

/// The level whose cells `level` holds, with `shared` forges indexed for the objects it names elsewhere.
pub fn read_level(level: &Path, shared: &[PathBuf]) -> Result<(World, Report)> {
    let mut index = Index::default();
    index.add(level)?;
    let level_entries = index.entries.len();
    for s in shared {
        index.add(s)?;
    }
    let frame = SourceFrame::ENU;
    let mut world = World::new();
    let mut report = Report::new();
    let mut prototypes: BTreeMap<u32, Option<Entity>> = BTreeMap::new();
    let mut materials: BTreeMap<u32, Entity> = BTreeMap::new();
    let mut textures: BTreeMap<u32, Option<Entity>> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut bump = |k: String| *counts.entry(k).or_default() += 1;
    let mut collision = super::level_collision::CollisionReader::new(&index);
    for d in index.entries.get(..level_entries).unwrap_or_default() {
        if d.objects.first().map(|o| o.class) != Some(GRID_CELL_DATA_BLOCK) {
            continue;
        }
        for o in d.objects.iter().filter(|o| o.class == scene::ENTITY) {
            let body = d.resource.get(o.body.clone()).unwrap_or_default();
            let entity = match scene::Entity::parse(body) {
                Ok(e) => e,
                Err(e) => {
                    bump(format!("entities not read: {e}"));
                    continue;
                }
            };
            bump("entities".into());
            let m = entity.matrix;
            let t = Transform {
                cols: [
                    [m[0], m[1], m[2]],
                    [m[4], m[5], m[6]],
                    [m[8], m[9], m[10]],
                    [m[12], m[13], m[14]],
                ],
            };
            // an Entity that marks a place (a start position, a spawn node) is a marker at its matrix
            let kind = markers::kind(&o.name, &entity.components);
            if let Some(kind) = kind {
                let e = world.spawn();
                world.insert(e, frame.transform(&t));
                world.insert(
                    e,
                    Marker {
                        kind: kind.into(),
                        ground: None,
                        yaw: None,
                    },
                );
                world.insert(e, Name(o.name.clone()));
                bump(format!("markers: {kind}"));
            } else if entity.visuals.is_empty() {
                bump("entities with neither a Visual nor a marker (no visual placement)".into());
            }
            match scene::collision_bindings(body) {
                Ok(bindings) => {
                    for (owner, shape) in bindings {
                        collision.place(&mut world, &d.name, owner, shape, &mut bump);
                    }
                }
                Err(e) => bump(format!("collision bindings not read: {e}")),
            }
            for v in &entity.visuals {
                let Some(mesh_id) = resolve(&index, v.target, &mut bump) else {
                    continue;
                };
                let proto = *prototypes.entry(mesh_id).or_insert_with(|| {
                    prototype(
                        &index,
                        mesh_id,
                        frame,
                        &mut world,
                        &mut materials,
                        &mut textures,
                        &mut bump,
                    )
                });
                let Some(proto) = proto else {
                    continue;
                };
                let p = world.spawn();
                world.insert(p, frame.transform(&t));
                world.insert(
                    p,
                    Instance {
                        prototype: proto,
                        variant: 0,
                    },
                );
                world.insert(p, Name(o.name.clone()));
                world.insert(
                    p,
                    PlacedBy {
                        cell: d.name.clone(),
                        entity: entity.id,
                        kind: v.kind,
                    },
                );
                bump("placements".into());
            }
        }
    }
    collision.finish(&mut world);
    report.push((
        "shaunwhite".into(),
        format!(
            "{} prototypes, {} materials, {} textures",
            prototypes.values().flatten().count(),
            materials.len(),
            textures.values().flatten().count()
        ),
    ));
    for (k, n) in counts {
        report.push(("shaunwhite".into(), format!("{k}: {n}")));
    }
    Ok((world, report))
}

/// The Mesh a Visual's target draws: a Mesh, or a LODSelector's nearest level.
fn resolve(index: &Index, target: u32, bump: &mut impl FnMut(String)) -> Option<u32> {
    let Some((o, body)) = index.get(target) else {
        bump("visuals whose object is in no forge read".into());
        return None;
    };
    match o.class {
        MESH => Some(target),
        LOD_SELECTOR => {
            let levels = scene::lod_levels(body).ok()?;
            let first = levels
                .first()
                .map(|l| l.0)
                .filter(|&id| index.get(id).is_some_and(|(o, _)| o.class == MESH));
            if first.is_none() {
                bump("LODSelectors whose nearest level is no Mesh read".into());
            }
            first
        }
        c => {
            bump(format!("visuals showing class {c:#010x} (not placed)"));
            None
        }
    }
}

/// The prototype of Mesh `id`: its standard primitives in core's frame, each with its Material entity.
fn prototype(
    index: &Index,
    id: u32,
    frame: SourceFrame,
    world: &mut World,
    materials: &mut BTreeMap<u32, Entity>,
    textures: &mut BTreeMap<u32, Option<Entity>>,
    bump: &mut impl FnMut(String),
) -> Option<Entity> {
    let (o, body) = index.get(id)?;
    let cm = match CompiledMesh::in_mesh(body) {
        Ok(m) => m,
        Err(e) => {
            bump(format!("meshes not read: {e}"));
            return None;
        }
    };
    let mats: Vec<Entity> = cm
        .materials
        .iter()
        .map(|&m| material(index, m, world, materials, textures, bump))
        .collect();
    let mut prims = match cm.primitives(|k| mats.get(k).copied()) {
        Ok(p) => p,
        Err(e) => {
            bump(format!("meshes not read: {e}"));
            return None;
        }
    };
    for p in &mut prims {
        for (sem, st) in &mut p.streams {
            match (sem, st) {
                (primase_core::Semantic::Position, primase_core::Stream::F32x3(v)) => {
                    v.iter_mut().for_each(|x| *x = frame.point(*x));
                }
                (primase_core::Semantic::Normal, primase_core::Stream::F32x3(v)) => {
                    v.iter_mut().for_each(|x| *x = frame.direction(*x));
                }
                _ => {}
            }
        }
        if frame.flips_winding()
            && let Some(ix) = &mut p.indices
        {
            ix.as_chunks_mut::<3>().0.iter_mut().for_each(|t| t.swap(1, 2));
        }
    }
    let e = world.spawn();
    world.insert(e, Mesh::new(prims));
    world.insert(e, Name(o.name.clone()));
    if let Ok(stride) = u32::try_from(cm.stride) {
        world.insert(
            e,
            MeshFormat {
                stride,
                flags: cm.flags,
            },
        );
    }
    bump("meshes".into());
    Some(e)
}

/// The texture entity of TextureMap `id`, read once (its first face: a cube map's others are not read).
fn texture(
    index: &Index,
    id: u32,
    world: &mut World,
    textures: &mut BTreeMap<u32, Option<Entity>>,
    bump: &mut impl FnMut(String),
) -> Option<Entity> {
    if let Some(&t) = textures.get(&id) {
        return t;
    }
    let read = index.get(id).and_then(|(o, body)| {
        let map = TextureMap::parse(body);
        let sampler = map.as_ref().ok().map(|t| Sampler {
            address: t.info.address,
            filter: t.info.filter,
        });
        let read = map.and_then(|t| t.textures());
        match read {
            Ok(mut faces) if !faces.is_empty() => {
                if faces.len() > 1 {
                    bump("cube maps: first face only".into());
                }
                let e = world.spawn();
                world.insert(e, faces.swap_remove(0));
                world.insert(e, Name(o.name.clone()));
                if let Some(s) = sampler {
                    world.insert(e, s);
                }
                Some(e)
            }
            Ok(_) => None,
            Err(err) => {
                bump(format!("textures not read: {err}"));
                None
            }
        }
    });
    textures.insert(id, read);
    read
}

/// The Material entity of Material id `id`, made once: its name and its texture slots' textures.
fn material(
    index: &Index,
    id: u32,
    world: &mut World,
    materials: &mut BTreeMap<u32, Entity>,
    textures: &mut BTreeMap<u32, Option<Entity>>,
    bump: &mut impl FnMut(String),
) -> Entity {
    if let Some(&e) = materials.get(&id) {
        return e;
    }
    let e = world.spawn();
    let mut r = MaterialRef {
        id,
        name: String::new(),
        state: None,
        slots: BTreeMap::new(),
        unread: Vec::new(),
    };
    if let Some((o, body)) = index.get(id)
        && o.class == MATERIAL
    {
        r.name = o.name.clone();
        match Material::parse(body) {
            Ok(m) => {
                r.state = Some(MaterialState {
                    blend: m.blend,
                    blended: m.blended,
                    alpha_test: m.alpha_test,
                    alpha_ref: m.alpha_ref,
                    water: m.water,
                    color: m.color,
                    illumination: m.illumination,
                    specular_level: m.specular_level,
                    specular_power: m.specular_power,
                    specular_color: m.specular_color,
                });
                for slot in &m.slots {
                    // the slots whose maps no lowering reads are named, not read
                    if !material::READ_SLOTS.contains(&slot.name) {
                        r.unread.push(slot.name.to_string());
                        continue;
                    }
                    // a slot holds a TextureMapSpec's id when one exists; its last u32 is the TextureMap's id
                    if let Some((so, sb)) = index.get(slot.spec)
                        && so.class == TEXTURE_MAP_SPEC
                        && let Some(tm) = sb
                            .get(28..32)
                            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        && let Some(t) = texture(index, tm, world, textures, bump)
                    {
                        r.slots.insert(
                            slot.name.to_string(),
                            SlotRef {
                                texture: t,
                                scale: slot.scale,
                                offset: slot.offset,
                            },
                        );
                    }
                }
            }
            Err(err) => bump(format!("materials not read: {err}")),
        }
        world.insert(e, Name(o.name.clone()));
    }
    world.insert(e, r);
    materials.insert(id, e);
    e
}
