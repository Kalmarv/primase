//! Texture coordinates for writers whose format samples fewer ways than the source: `bake-texture-transforms` and
//! `first-texcoord-set`.
//!
//! A transform belongs to a material's slot; the coordinates belong to the primitives that use the material. Baking
//! moves it into a primitive's `TexCoord(n)` stream, which every material that primitive can be drawn with (its own
//! and its per-placement variants) then samples transformed. So it is exact only for a group closed under that sharing:
//! the materials and primitives connected through use, for one coordinate set. A group is baked when all its materials
//! sample set n with the same transform (one value per material, the same across the group), every primitive has the
//! stream and its geometry is loaded; otherwise it is left as it is, counted by reason in texture slots.
//!
//! `first-texcoord-set` makes the one set a group samples its first, for a writer that samples only the first (a source
//! may sample set 1 for a detail layer, or a lowering state coordinates it projects as a set of their own): when every
//! slot of every material in a group samples the same set n other than 0, sets 0 and n swap places in every primitive
//! of the group and every slot then samples set 0. A swap loses nothing (the old first set is kept as set n), so it is
//! exact. A group is left as it is, counted by reason in texture slots, when its materials sample several sets, a
//! material drawn in it is not a `pbr.material` (a blend of layers samples through its layers), no primitive uses it, a
//! primitive of it lacks set n, or its geometry is still in a file.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
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
/// A group of [`groups`]: its materials (those with `pbr.material`), its primitives with whether each one's geometry
/// is loaded, and whether a primitive of it is drawn with a material that is not a `pbr.material`.
struct Group {
    materials: Vec<Entity>,
    prims: Vec<(Prim, bool)>,
    unlowered: bool,
}

/// The materials joined through the primitives that use them (a primitive's own and its per-placement variants), each
/// group with its primitives; a prototype still in a file counts by its file's materials.
fn groups(world: &World) -> Vec<Group> {
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
    let new = || Group {
        materials: Vec::new(),
        prims: Vec::new(),
        unlowered: false,
    };
    let mut groups: BTreeMap<Entity, Group> = BTreeMap::new();
    for (m, _) in world.iter::<PbrMaterial>() {
        let root = find(&mut parent, m);
        groups.entry(root).or_insert_with(new).materials.push(m);
    }
    for (p, ms, loaded) in &uses {
        if let Some(&m) = ms.first() {
            let root = find(&mut parent, m);
            let g = groups.entry(root).or_insert_with(new);
            g.prims.push((*p, *loaded));
            g.unlowered |= ms.iter().any(|&m| !world.has::<PbrMaterial>(m));
        }
    }
    groups.into_values().collect()
}

/// The texture slots of `materials` that sample set `n` (every set: `None`).
fn slots_sampling(world: &World, materials: &[Entity], n: Option<u8>) -> u64 {
    materials
        .iter()
        .filter_map(|&m| world.get::<PbrMaterial>(m))
        .map(|m| m.textures().filter(|r| n.is_none_or(|n| r.texcoord == n)).count() as u64)
        .sum()
}

/// Why `prims` can't have set `n` changed: none uses the group, a geometry is still in a file, or a primitive lacks the
/// set.
fn unchangeable(world: &World, prims: &[(Prim, bool)], n: u8) -> Option<&'static str> {
    if prims.is_empty() {
        return Some("no primitive uses the material");
    }
    if prims.iter().any(|(_, loaded)| !loaded) {
        return Some("geometry still in a file");
    }
    let has_stream = |&((e, i), _): &(Prim, bool)| {
        matches!(
            world
                .get::<Mesh>(e)
                .map(|m| m.primitives()[i].streams.get(&Semantic::TexCoord(n))),
            Some(Some(Stream::F32x2(_)))
        )
    };
    (!prims.iter().all(has_stream)).then_some("a primitive lacks the coordinate set")
}

/// Bake every bakeable transform (see the module documentation), clearing it from the slots it came from.
pub fn bake_texture_transforms(world: &mut World) -> PassLog {
    let mut log = PassLog::default();
    let groups = groups(world);
    let sets: std::collections::BTreeSet<u8> = world
        .iter::<PbrMaterial>()
        .flat_map(|(_, m)| m.textures().map(|r| r.texcoord).collect::<Vec<_>>())
        .collect();
    for n in sets {
        for Group {
            materials: ms, prims, ..
        } in &groups
        {
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
            let refuse = |log: &mut PassLog, why: &str| {
                log.count(&format!("not baked: {why}"), slots_sampling(world, ms, Some(n)));
            };
            if mixed {
                refuse(&mut log, "materials sample a set with different transforms");
                continue;
            }
            let Some(t) = transform.filter(|t| *t != UvTransform::IDENTITY) else {
                continue;
            };
            if let Some(why) = unchangeable(world, prims, n) {
                refuse(&mut log, why);
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

/// `bake-texture-transforms`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const BAKE_TEXTURE_TRANSFORMS: PassSpec = PassSpec {
    name: "bake-texture-transforms",
    options: &[],
    make: |_| Ok(Box::new(BakeTextureTransforms)),
};

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

/// Make the one set each group samples its first (see the module documentation).
pub fn first_texcoord_set(world: &mut World) -> PassLog {
    let mut log = PassLog::default();
    for Group {
        materials: ms,
        prims,
        unlowered,
    } in groups(world)
    {
        let sets: Vec<u8> = ms
            .iter()
            .filter_map(|&m| world.get::<PbrMaterial>(m))
            .flat_map(|m| m.textures().map(|r| r.texcoord).collect::<Vec<_>>())
            .collect::<std::collections::BTreeSet<u8>>()
            .into_iter()
            .collect();
        if sets.iter().all(|&n| n == 0) {
            continue;
        }
        let refuse = |log: &mut PassLog, why: &str| {
            log.count(&format!("not moved: {why}"), slots_sampling(world, &ms, None));
        };
        let &[n] = &sets[..] else {
            refuse(&mut log, "materials sample several sets");
            continue;
        };
        if unlowered {
            refuse(&mut log, "a material is not a pbr.material");
            continue;
        }
        if let Some(why) = unchangeable(world, &prims, n) {
            refuse(&mut log, why);
            continue;
        }
        for &((e, i), _) in &prims {
            let mesh = world.get_mut::<Mesh>(e).expect("loaded");
            let mut primitive = mesh.primitive_mut(i).expect("a listed primitive");
            let streams = primitive.streams_mut();
            let moved = streams.remove(&Semantic::TexCoord(n)).expect("checked");
            if let Some(first) = streams.insert(Semantic::TexCoord(0), moved) {
                streams.insert(Semantic::TexCoord(n), first);
            }
            world.remove::<MeshFile>(e);
            log.count("primitives with a set moved first", 1);
        }
        for &m in &ms {
            let mat = world.get_mut::<PbrMaterial>(m).expect("material");
            for s in mat.textures_mut() {
                s.texcoord = 0;
                log.count("slots sampling the first set", 1);
            }
        }
    }
    log
}

/// [`first_texcoord_set`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct FirstTexcoordSet;

/// `first-texcoord-set`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const FIRST_TEXCOORD_SET: PassSpec = PassSpec {
    name: "first-texcoord-set",
    options: &[],
    make: |_| Ok(Box::new(FirstTexcoordSet)),
};

impl Pass for FirstTexcoordSet {
    fn name(&self) -> &'static str {
        "first-texcoord-set"
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "pbr.material"]
    }
    /// The file links it removes no longer match the meshes.
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        log.absorb(first_texcoord_set(world));
        Ok(())
    }
}
