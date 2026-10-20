//! Baking texture-coordinate transforms into geometry, for writers whose format has no transform per texture slot.
//!
//! A transform belongs to a material's slot; the coordinates belong to the primitives that use the material. Baking
//! moves it into a primitive's `TexCoord(n)` stream, which every material that primitive can be drawn with (its own
//! and its per-placement variants) then samples transformed. So it is exact only for a group closed under that sharing:
//! the materials and primitives connected through use, for one coordinate set. A group is baked when all its materials
//! sample set n with the same transform (one value per material, the same across the group), every primitive has the
//! stream and its geometry is loaded; otherwise it is left as it is, counted by reason in texture slots.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{Entity, MaterialVariants, Mesh, MeshFile, Semantic, Stream, World};

/// What material `m` does to coordinate set `n`: `Ok(None)` when no slot samples it, `Ok(Some(t))` when every slot
/// that does has transform `t` (identity for none), `Err(())` when they differ.
fn transform_of(m: &PbrMaterial, n: u8) -> Result<Option<UvTransform>, ()> {
    let mut found: Option<UvTransform> = None;
    for r in m.textures().filter(|r| r.texcoord == n) {
        let t = r.transform.unwrap_or(UvTransform::IDENTITY);
        match found {
            Some(f) if f != t => return Err(()),
            _ => found = Some(t),
        }
    }
    Ok(found)
}

fn find(parent: &mut BTreeMap<Entity, Entity>, e: Entity) -> Entity {
    let p = *parent.entry(e).or_insert(e);
    if p == e {
        return e;
    }
    let root = find(parent, p);
    parent.insert(e, root);
    root
}

/// A primitive: its prototype and index.
type Prim = (Entity, usize);
/// Materials joined through use, and their primitives with whether each one's geometry is loaded.
type Group = (Vec<Entity>, Vec<(Prim, bool)>);

/// Bake every bakeable transform (see the module documentation), clearing it from the slots it came from.
pub fn bake_texture_transforms(world: &mut World) -> PassLog {
    let mut log = PassLog::default();
    // the materials each primitive can be drawn with; prototypes still in a file count by their file's materials
    let mut uses: Vec<(Prim, Vec<Entity>, bool)> = Vec::new();
    for (e, mesh) in world.iter::<Mesh>() {
        let variants = world.get::<MaterialVariants>(e);
        for (i, p) in mesh.primitives().iter().enumerate() {
            let mut ms: Vec<Entity> = p.material.into_iter().collect();
            if let Some(v) = variants.and_then(|v| v.0.get(i)) {
                ms.extend(v.values().copied());
            }
            uses.push(((e, i), ms, true));
        }
    }
    for (e, f) in world.iter::<MeshFile>() {
        if world.has::<Mesh>(e) {
            continue;
        }
        let variants = world.get::<MaterialVariants>(e);
        for (i, m) in f.materials.iter().enumerate() {
            let mut ms: Vec<Entity> = m.iter().copied().collect();
            if let Some(v) = variants.and_then(|v| v.0.get(i)) {
                ms.extend(v.values().copied());
            }
            uses.push(((e, i), ms, false));
        }
    }
    let sets: std::collections::BTreeSet<u8> = world
        .iter::<PbrMaterial>()
        .flat_map(|(_, m)| m.textures().map(|r| r.texcoord).collect::<Vec<_>>())
        .collect();
    for n in sets {
        // groups: materials joined through the primitives that use them
        let mut parent: BTreeMap<Entity, Entity> = BTreeMap::new();
        for (_, ms, _) in &uses {
            for w in ms.windows(2) {
                let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                parent.insert(a, b);
            }
            for &m in ms {
                find(&mut parent, m);
            }
        }
        let materials: Vec<Entity> = world.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        let mut groups: BTreeMap<Entity, Group> = BTreeMap::new();
        for &m in &materials {
            let root = find(&mut parent, m);
            groups.entry(root).or_default().0.push(m);
        }
        for (p, ms, loaded) in &uses {
            if let Some(&m) = ms.first() {
                let root = find(&mut parent, m);
                groups.entry(root).or_default().1.push((*p, *loaded));
            }
        }
        for (ms, prims) in groups.values() {
            let mut transform: Option<UvTransform> = None;
            let mut mixed = false;
            for &m in ms {
                let Some(mat) = world.get::<PbrMaterial>(m) else {
                    continue;
                };
                match transform_of(mat, n) {
                    Err(()) => mixed = true,
                    Ok(None) => {}
                    Ok(Some(t)) => match transform {
                        Some(f) if f != t => mixed = true,
                        _ => transform = Some(t),
                    },
                }
            }
            let slots_with = || -> u64 {
                ms.iter()
                    .filter_map(|&m| world.get::<PbrMaterial>(m))
                    .map(|m| m.textures().filter(|r| r.texcoord == n).count() as u64)
                    .sum()
            };
            let refuse = |log: &mut PassLog, why: &str| log.count(&format!("not baked: {why}"), slots_with());
            if mixed {
                refuse(&mut log, "materials sample a set with different transforms");
                continue;
            }
            let Some(t) = transform.filter(|t| *t != UvTransform::IDENTITY) else {
                continue;
            };
            if prims.is_empty() {
                refuse(&mut log, "no primitive uses the material");
                continue;
            }
            if prims.iter().any(|(_, loaded)| !loaded) {
                refuse(&mut log, "geometry still in a file");
                continue;
            }
            let has_stream = |&((e, i), _): &(Prim, bool)| {
                matches!(
                    world
                        .get::<Mesh>(e)
                        .map(|m| m.primitives()[i].streams.get(&Semantic::TexCoord(n))),
                    Some(Some(Stream::F32x2(_)))
                )
            };
            if !prims.iter().all(has_stream) {
                refuse(&mut log, "a primitive lacks the coordinate set");
                continue;
            }
            for &((e, i), _) in prims {
                let mesh = world.get_mut::<Mesh>(e).expect("loaded");
                if let Some(Stream::F32x2(uv)) = mesh
                    .primitive_mut(i)
                    .expect("a listed primitive")
                    .streams_mut()
                    .get_mut(&Semantic::TexCoord(n))
                {
                    for x in uv.iter_mut() {
                        *x = t.apply(*x);
                    }
                }
                world.remove::<MeshFile>(e);
                log.count("coordinate sets baked", 1);
            }
            for &m in ms {
                let mat = world.get_mut::<PbrMaterial>(m).expect("material");
                for s in mat.textures_mut() {
                    if s.texcoord == n && s.transform.is_some() {
                        s.transform = None;
                        log.count("slot transforms cleared", 1);
                    }
                }
            }
        }
    }
    log
}

/// [`bake_texture_transforms`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct BakeTextureTransforms;

impl Pass for BakeTextureTransforms {
    fn name(&self) -> &'static str {
        "bake-texture-transforms"
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "pbr.material"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    /// It lowers texture transforms; the file links it removes no longer match the meshes.
    fn legalizes(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "texture coordinates transformed once in f32"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let r = bake_texture_transforms(world);
        log.absorb(r);
        log.approximate(
            "coordinate sets transformed in f32",
            log.counts.get("coordinate sets baked").copied().unwrap_or(0),
        );
        Ok(())
    }
}
