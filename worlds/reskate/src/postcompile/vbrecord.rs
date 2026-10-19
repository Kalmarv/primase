//! The map writer's vertex-blend record (`vertex_blend.json`) resolved against Studio's compile: which material keys
//! of the Patch are the recorded `vertex_blend` materials, and the per-vertex values their sections get.
//!
//! The record decides which materials are vertex blends: `{"format": 1, "materials": {name: [[mesh, primitive],
//! ...]}}`, each (GLB, primitive) of the map drawing the material. Studio's material keys are not a function of the
//! material's name (they appear to be derived from its contents) and its output names no material, so each recorded
//! primitive is found in the compile by its weights:
//! - Studio compiles a `vertex_blend` material's surfaces with `TexCoord0:Half4`, the weight in `z`
//!   ([`crate::postcompile::vblend`]); it names a MeshSet after the first map object drawing the GLB (`" / "` and
//!   spaces become `___` and `_`) followed by `_<counter>_<n>`, and the writer names objects `<GLB stem> / section
//!   N`, or `<GLB stem>` when the GLB has one primitive, so the MeshSets of a GLB are the ones named
//!   `<stem>___section_..._<counter>_<n>` or `<stem>_<counter>_<n>` (a map not written by the writer may not follow
//!   this);
//! - Studio tessellates and re-wraps those surfaces (about five times the triangles), so section vertices are not the
//!   primitive's vertices; but tessellation keeps the source vertices' weights, so every distinct weight of the
//!   vertices the primitive draws (Studio drops the others: a lowered overlay shares its source primitive's
//!   vertices), as a half float (a weight exactly halfway between two halves: either, as Studio rounds such a tie
//!   away from the even half: [`half_choices`]), occurs in its key's sections;
//! - a primitive's candidates are the vertex-blend keys of its GLB's MeshSets whose sections contain all of its weights
//!   other than 0 and 1 (those are not distinctive; a primitive with no other weight, a small piece of one layer or of
//!   a decal's fade, allows every key of its GLB); a material's key is one all its primitives allow, and a key belongs
//!   to one material: a material left with a single key takes it, and that key leaves the others' choices, until
//!   nothing changes (a decal faded by one alpha value, contained in many keys, is told apart this way).
//!
//! Validated on ReSkate Studio 20261012 ([`STUDIO_BUILD`]): on a real layered scene every recorded primitive's weights
//! were all contained in exactly one key, the next-best key containing at most 46% of them, and the sections followed
//! the GLB's primitive order. The rule assumes that tessellation keeps the source weights and that `TexCoord0.z` is
//! stored as a half float; a different Studio build should be re-checked against both ([`Resolution`] carries the
//! margins). Materials still left with several keys are interchangeable (their primitives' weights fit the same keys,
//! as decals faded by one alpha value do): the stage treats every key alike, so they take distinct keys by a matching
//! ([`Resolution::interchangeable`] counts them). Refused rather than guessed: a primitive no key contains, a material
//! left with no key, two materials with one key, and a vertex-blend key no record claims (the discovery
//! cross-check).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use primase_compress::oodle::Oodle;

use crate::postcompile::fb::MESHSET;
use crate::postcompile::fb::folder::{Folder, Source};
use crate::postcompile::map_folder::{BlendMap, MapObject, Record};
use crate::postcompile::meshset;
use crate::postcompile::vblend::{
    self, Donor, Report, Request, SectionInput, SectionValues, StockPrograms, Target, decal_opacity,
    lod_geometry, patch_bundles, surface,
};
use crate::postcompile::{Result, bad};

const W: &str = "vertex-blend record";

/// The Studio build the weight match was established on.
pub const STUDIO_BUILD: &str = "ReSkate Studio 20261012";

/// One vertex-blend section of the compile: its MeshSet, where it is, its key and its weights as half-float bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlendSection {
    pub mesh: String,
    pub lod: usize,
    pub section: usize,
    pub key: u64,
    pub weights: Vec<u16>,
}

/// Every section of the Patch with Studio's vertex-blend layout, in TOC, bundle, MeshSet, LOD and section order.
pub fn blend_sections(src: &Source) -> Result<Vec<BlendSection>> {
    let mut out = Vec::new();
    for (_, _, bundles) in patch_bundles(src)? {
        for b in bundles.iter().flatten() {
            for (i, a) in b
                .manifest
                .res
                .iter()
                .enumerate()
                .filter(|(_, a)| a.rtype == MESHSET)
            {
                let d = src.payload(b.files[b.manifest.res_file(i)], a.size as usize)?;
                for (li, lod) in meshset::read(&d)?.lods.iter().enumerate() {
                    let mut geo = None;
                    for (si, sec) in lod.sections.iter().enumerate() {
                        let half4 = sec.decl.element(meshset::usage::TEXCOORD0).map(|e| e.format)
                            == Some(meshset::format::HALF4);
                        if !half4 {
                            continue;
                        }
                        if geo.is_none() {
                            geo = Some(lod_geometry(src, b, &a.name, li, lod)?.1);
                        }
                        let Some((_, _, Some(w))) = geo.as_deref().and_then(|g| surface(sec, g)) else {
                            return Err(bad(
                                W,
                                sec.at,
                                format!(
                                    "{} LOD {li} section {si}: TexCoord0:Half4 in layout {:?}, not Studio's \
                                     vertex-blend layout",
                                    a.name, sec.decl.elements
                                ),
                            ));
                        };
                        out.push(BlendSection {
                            mesh: a.name.clone(),
                            lod: li,
                            section: si,
                            key: sec.key,
                            weights: w.iter().map(|&x| half::f16::from_f32(x).to_bits()).collect(),
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// A recorded primitive as resolved: its key, its distinctive weights and the best containment among the other
/// vertex-blend keys of its GLB (the margin; the resolved key contains all of them).
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub material: String,
    pub mesh: String,
    pub primitive: usize,
    pub key: u64,
    pub distinct_weights: usize,
    pub runner_up: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    /// Material name -> its Studio key.
    pub keys: BTreeMap<String, u64>,
    pub primitives: Vec<Resolved>,
    /// Per GLB stem: whether the vertex-blend sections appear in the order of the GLB's primitives (observed with
    /// Studio 20261012; reported, not required).
    pub in_primitive_order: BTreeMap<String, bool>,
    /// Materials whose primitives' weights fit the same keys (a decal faded by one alpha value), given distinct keys by
    /// a matching: the stage treats every key alike, so the choice does not change its output.
    pub interchangeable: usize,
}

/// The GLB stem in a Studio MeshSet's name, lowercase: `.../<stem>___section_<i>_<counter>_<n>` (the writer's
/// `<stem> / section <i>`) or `.../<stem>_<counter>_<n>` (its `<stem>`, a GLB of one primitive).
fn meshset_stem(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    if let Some((stem, _)) = base.split_once("___") {
        return stem.to_ascii_lowercase();
    }
    // Studio's `_<counter>_<n>` suffix
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    let stem = match base.rsplitn(3, '_').collect::<Vec<_>>()[..] {
        [n, counter, stem] if numeric(n) && numeric(counter) => stem,
        _ => base,
    };
    stem.to_ascii_lowercase()
}

fn glb_stem(mesh: &str) -> String {
    Path::new(mesh)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Half-float bits of 0, -0 and 1: weights that do not identify a section.
fn distinctive(bits: u16) -> bool {
    !matches!(bits, 0x0000 | 0x8000 | 0x3c00)
}

/// The half floats Studio may store for a weight: the nearest, and where the weight lies exactly halfway between two
/// halves, both of them. Studio rounds such a tie away from the even half (found on the GTA downtown-east compile:
/// both of its tie weights, 0.968994140625, were stored as 0.969238 where round-half-to-even gives 0.96875), where
/// `half` rounds to even; so a tie allows either.
fn half_choices(w: f32) -> [u16; 2] {
    let h = half::f16::from_f32(w);
    let (lo, hi) = (
        half::f16::from_bits(h.to_bits().wrapping_sub(1)),
        half::f16::from_bits(h.to_bits().wrapping_add(1)),
    );
    let d = (f64::from(w) - f64::from(h.to_f32())).abs();
    // the neighbour on the weight's side, if it is exactly as near
    let other = if f64::from(w) > f64::from(h.to_f32()) {
        hi
    } else {
        lo
    };
    if d > 0.0 && (f64::from(other.to_f32()) - f64::from(w)).abs() == d {
        [h.to_bits(), other.to_bits()]
    } else {
        [h.to_bits(); 2]
    }
}

/// Record that `material` is `key`: refused when the material already resolved to another key or the key to another
/// material.
fn claim(
    keys: &mut BTreeMap<String, u64>,
    owner: &mut BTreeMap<u64, String>,
    material: &str,
    key: u64,
) -> Result<()> {
    if let Some(&k) = keys.get(material)
        && k != key
    {
        return Err(bad(
            W,
            0,
            format!("material {material} resolves to two keys, {k:016x} and {key:016x}"),
        ));
    }
    keys.insert(material.to_owned(), key);
    if let Some(other) = owner.insert(key, material.to_owned())
        && other != material
    {
        return Err(bad(
            W,
            0,
            format!("materials {other} and {material} resolve to one key, {key:016x}"),
        ));
    }
    Ok(())
}

/// Resolves every recorded primitive of a map's input (from [`crate::postcompile::map_folder::read`]) among `sections`
/// (from [`blend_sections`]).
pub fn resolve(input: &BlendMap, sections: &[BlendSection]) -> Result<Resolution> {
    let (names, peers) = object_glbs(&input.objects)?;
    resolve_by(&input.record, sections, &names, &peers, |mesh, primitive| {
        input
            .weights
            .get(&(mesh.to_owned(), primitive))
            .cloned()
            .ok_or_else(|| {
                bad(
                    W,
                    0,
                    format!("{mesh} primitive {primitive}: no weights in the map's input"),
                )
            })
    })
}

/// Per GLB stem, the stems of the other GLBs drawn by objects at the same placement (the same 12 transform numbers):
/// Studio merges the pieces of objects at one placement into one MeshSet named after one of them, so a GLB's sections
/// can be in a peer's MeshSet.
type Peers = BTreeMap<String, BTreeSet<String>>;

/// The map's object names as Studio writes them into a MeshSet's name (`" / "` and spaces as `___` and `_`, lower case,
/// before any `___section`), each with the stem of the GLB it draws (from `map.json`'s `objects`): an instance's objects
/// are named after their tile and prototype, not after its GLB. An object name drawing two GLBs is refused. With them,
/// each GLB's placement [`Peers`].
fn object_glbs(objects: &[MapObject]) -> Result<(BTreeMap<String, String>, Peers)> {
    let mut out = BTreeMap::new();
    let mut placed: BTreeMap<Vec<u64>, BTreeSet<String>> = BTreeMap::new();
    for MapObject {
        name,
        mesh,
        transform,
    } in objects
    {
        let at: Vec<u64> = transform.iter().map(|x| x.to_bits()).collect();
        placed.entry(at).or_default().insert(glb_stem(mesh));
        let base = name
            .split(" / ")
            .next()
            .unwrap_or(name)
            .replace(' ', "_")
            .to_ascii_lowercase();
        let glb = glb_stem(mesh);
        if let Some(other) = out.insert(base.clone(), glb.clone())
            && other != glb
        {
            return Err(bad(
                W,
                0,
                format!("map.json: objects named {base} draw {other} and {glb}"),
            ));
        }
    }
    let mut peers = Peers::new();
    for glbs in placed.values().filter(|g| g.len() > 1) {
        for g in glbs {
            peers
                .entry(g.clone())
                .or_default()
                .extend(glbs.iter().filter(|o| *o != g).cloned());
        }
    }
    Ok((out, peers))
}

/// [`resolve`] with the recorded primitives' weights from `weights_of(mesh, primitive)`.
fn resolve_by(
    record: &Record,
    sections: &[BlendSection],
    names: &BTreeMap<String, String>,
    peers: &Peers,
    weights_of: impl Fn(&str, usize) -> Result<Vec<f32>>,
) -> Result<Resolution> {
    // stem -> key -> the weights of its sections; and each stem's keys in section order
    let mut sets: BTreeMap<String, BTreeMap<u64, HashSet<u16>>> = BTreeMap::new();
    let mut order: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for s in sections {
        // the GLB the MeshSet's object draws (an instance's object is not named after its GLB), else its own stem
        let stem = meshset_stem(&s.mesh);
        let stem = names.get(&stem).cloned().unwrap_or(stem);
        sets.entry(stem.clone())
            .or_default()
            .entry(s.key)
            .or_default()
            .extend(s.weights.iter().copied());
        let o = order.entry(stem).or_default();
        if !o.contains(&s.key) {
            o.push(s.key);
        }
    }
    // every key of the Patch with the weights of all its sections, wherever they are: the last resort for a primitive
    // whose pieces Studio merged into prototypes named after no object (its cell prototypes, d2_x<i>_z<j>_<n>)
    let mut all: BTreeMap<u64, HashSet<u16>> = BTreeMap::new();
    for set in sets.values() {
        for (k, w) in set {
            all.entry(*k).or_default().extend(w.iter().copied());
        }
    }
    // MeshSets are matched to GLBs by lowercase file stem: two recorded GLBs with one stem would share a candidate set
    let mut stems: BTreeMap<String, &str> = BTreeMap::new();
    for (mesh, _) in record.materials.values().flatten() {
        if let Some(other) = stems.insert(glb_stem(mesh), mesh)
            && other != mesh
        {
            return Err(bad(
                W,
                0,
                format!("{other} and {mesh} share a file stem: their compiled MeshSets cannot be told apart"),
            ));
        }
    }
    // ---- each primitive's candidate keys: those of its GLB whose sections contain all its distinctive weights (every
    // key of the GLB for a primitive with none: a small piece with only weights 0 and 1)
    struct Candidate {
        material: String,
        mesh: String,
        primitive: usize,
        /// key -> how many of the primitive's weights its sections contain
        contained: Vec<(u64, usize)>,
        full: BTreeSet<u64>,
        distinct: usize,
        /// The primitive's distinctive weights (each with its half choices).
        weights: BTreeSet<[u16; 2]>,
    }
    let mut found = Vec::new();
    for (material, pairs) in &record.materials {
        for (mesh, primitive) in pairs {
            let at = format!("material {material} in {mesh} primitive {primitive}");
            let stem = glb_stem(mesh);
            // its own MeshSets, else (Studio merged them into a MeshSet named after another object at the same
            // placement) its placement peers': the weights below pick the keys
            let merged: BTreeMap<u64, HashSet<u16>>;
            let candidates = match sets.get(&stem) {
                Some(c) => c,
                None => {
                    let mut m: BTreeMap<u64, HashSet<u16>> = BTreeMap::new();
                    for p in peers.get(&stem).into_iter().flatten() {
                        for (k, w) in sets.get(p).into_iter().flatten() {
                            m.entry(*k).or_default().extend(w.iter().copied());
                        }
                    }
                    merged = m;
                    &merged
                }
            };
            let weights: BTreeSet<[u16; 2]> = weights_of(mesh, *primitive)?
                .iter()
                .map(|&w| half_choices(w))
                .filter(|b| distinctive(b[0]))
                .collect();
            let contain = |candidates: &BTreeMap<u64, HashSet<u16>>| -> (Vec<(u64, usize)>, BTreeSet<u64>) {
                let contained: Vec<(u64, usize)> = candidates
                    .iter()
                    .map(|(&k, set)| {
                        (
                            k,
                            weights
                                .iter()
                                .filter(|w| w.iter().any(|b| set.contains(b)))
                                .count(),
                        )
                    })
                    .collect();
                let full = contained
                    .iter()
                    .filter(|&&(_, n)| n == weights.len())
                    .map(|&(k, _)| k)
                    .collect();
                (contained, full)
            };
            // its GLB's (or placement peers') keys; else every key of the Patch: the stage treats every key alike, so
            // only which keys a material claims matters, and each key still belongs to one material
            let (mut contained, mut full) = contain(candidates);
            if full.is_empty() {
                (contained, full) = contain(&all);
            }
            if full.is_empty() {
                return Err(bad(
                    W,
                    0,
                    format!(
                        "{at}: 0 vertex-blend keys contain all {} of its weights, in its MeshSets, its placement \
                         peers' or the whole Patch (the compile did not carry the weights? vertex_blend needs \
                         {STUDIO_BUILD} or a build checked against it)",
                        weights.len()
                    ),
                ));
            }
            found.push(Candidate {
                material: material.clone(),
                mesh: mesh.clone(),
                primitive: *primitive,
                contained,
                full,
                distinct: weights.len(),
                weights: weights.clone(),
            });
        }
    }
    // ---- a material's key is one every primitive of it allows, and a key belongs to one material: materials with a
    // single possible key take it and it leaves the others' choices, until nothing changes
    let mut choices: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for c in &found {
        choices
            .entry(c.material.clone())
            .and_modify(|k| *k = k.intersection(&c.full).copied().collect())
            .or_insert_with(|| c.full.clone());
    }
    // a material whose primitives' own keys agree on none (one primitive's partial match among its own MeshSets can
    // leave out the key the others hold) takes the keys holding every one of its primitives in the whole Patch
    for (m, ks) in choices.iter_mut().filter(|(_, ks)| ks.is_empty()) {
        let mut common: Option<BTreeSet<u64>> = None;
        for c in found.iter().filter(|c| &c.material == m) {
            let f: BTreeSet<u64> = all
                .iter()
                .filter(|(_, set)| c.weights.iter().all(|w| w.iter().any(|b| set.contains(b))))
                .map(|(&k, _)| k)
                .collect();
            common = Some(match common {
                None => f,
                Some(i) => i.intersection(&f).copied().collect(),
            });
        }
        *ks = common.unwrap_or_default();
    }
    let mut keys: BTreeMap<String, u64> = BTreeMap::new();
    let mut owner: BTreeMap<u64, String> = BTreeMap::new();
    loop {
        let single: Vec<(String, u64)> = choices
            .iter()
            .filter(|(m, k)| k.len() == 1 && !keys.contains_key(*m))
            .map(|(m, k)| (m.clone(), *k.first().expect("one")))
            .collect();
        if single.is_empty() {
            break;
        }
        for (m, k) in single {
            claim(&mut keys, &mut owner, &m, k)?;
            for (other, ks) in choices.iter_mut() {
                if *other != m {
                    ks.remove(&k);
                }
            }
        }
    }
    // ---- materials still left with several keys are interchangeable: their primitives' weights fit the same keys.
    // The stage treats every recorded key alike (onto the stock decal row, values from Studio's own weights), so which
    // takes which does not change the output: they get distinct keys by a matching (in name and key order), refused
    // when none exists
    let open: Vec<(String, Vec<u64>)> = choices
        .iter()
        .filter(|(m, _)| !keys.contains_key(*m))
        .map(|(m, ks)| {
            (
                m.clone(),
                ks.iter().copied().filter(|k| !owner.contains_key(k)).collect(),
            )
        })
        .collect();
    let mut taken: BTreeMap<u64, usize> = BTreeMap::new();
    for i in 0..open.len() {
        let mut seen = BTreeSet::new();
        if !augment(i, &open, &mut taken, &mut seen) {
            let (m, ks) = &open[i];
            return Err(bad(
                W,
                0,
                format!(
                    "material {m}: no vertex-blend key left for it ({} keys contain the weights of all its primitives, \
                     all needed by other materials: {})",
                    ks.len(),
                    ks.iter()
                        .map(|k| format!("{k:016x}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    let interchangeable = taken.len();
    for (k, i) in taken {
        claim(&mut keys, &mut owner, &open[i].0, k)?;
    }
    let primitives: Vec<Resolved> = found
        .into_iter()
        .map(|c| {
            let key = keys[&c.material];
            let runner_up = if c.distinct == 0 {
                0.0
            } else {
                c.contained
                    .iter()
                    .filter(|&&(k, _)| k != key)
                    .map(|&(_, n)| n as f64 / c.distinct as f64)
                    .fold(0.0, f64::max)
            };
            Resolved {
                material: c.material,
                mesh: c.mesh,
                primitive: c.primitive,
                key,
                distinct_weights: c.distinct,
                runner_up,
            }
        })
        .collect();
    let unclaimed: BTreeSet<u64> = sections
        .iter()
        .map(|s| s.key)
        .filter(|k| !owner.contains_key(k))
        .collect();
    if !unclaimed.is_empty() {
        return Err(bad(
            W,
            0,
            format!(
                "vertex-blend keys no recorded material resolves to: {}",
                unclaimed
                    .iter()
                    .map(|k| format!("{k:016x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let mut in_primitive_order = BTreeMap::new();
    for (stem, seen) in &order {
        let mut by_prim: Vec<(usize, u64)> = primitives
            .iter()
            .filter(|r| glb_stem(&r.mesh) == *stem)
            .map(|r| (r.primitive, r.key))
            .collect();
        by_prim.sort_unstable();
        let mut recorded: Vec<u64> = Vec::new();
        for (_, k) in by_prim {
            if !recorded.contains(&k) {
                recorded.push(k);
            }
        }
        in_primitive_order.insert(stem.clone(), recorded == *seen);
    }
    Ok(Resolution {
        keys,
        primitives,
        in_primitive_order,
        interchangeable,
    })
}

/// Kuhn's augmenting path: give open material `i` a key of its choices, moving earlier materials to other keys of theirs
/// where that frees one. `taken`: key -> the open material holding it.
fn augment(
    i: usize,
    open: &[(String, Vec<u64>)],
    taken: &mut BTreeMap<u64, usize>,
    seen: &mut BTreeSet<u64>,
) -> bool {
    for &k in &open[i].1 {
        if !seen.insert(k) {
            continue;
        }
        match taken.get(&k).copied() {
            None => {
                taken.insert(k, i);
                return true;
            }
            Some(j) => {
                if augment(j, open, taken, seen) {
                    taken.insert(k, i);
                    return true;
                }
            }
        }
    }
    false
}

/// The values of a recorded material's section: `Color0` = [`decal_opacity`] of Studio's weight, sub-material 0.
/// Refuses a section without the weight.
pub fn blend_values(input: &SectionInput) -> Result<SectionValues> {
    let w = input.blend.ok_or_else(|| {
        bad(
            W,
            0,
            format!(
                "{} LOD {} section {} (key {:016x}): no vertex-blend weight; the compile did not carry it \
                 (vertex_blend needs {STUDIO_BUILD} or a build checked against it)",
                input.mesh, input.lod, input.section, input.key
            ),
        )
    })?;
    Ok(SectionValues {
        color0: w.iter().map(|&x| decal_opacity(x)).collect(),
        submaterial: vec![[0; 4]; w.len()],
    })
}

/// Applies a map's vertex-blend input (from [`crate::postcompile::map_folder::read`]) to Patch `patch` into `out`
/// ([`vblend::apply`]): every recorded material's key onto the stock decal row of `decal`, its opacities from Studio's
/// weights, and the game's program records `stock` in place of Studio's.
pub fn apply(
    patch: &Path,
    out: &Path,
    input: &BlendMap,
    base: Option<&Folder>,
    oodle: Option<&Oodle>,
    decal: &Donor,
    stock: &StockPrograms,
) -> Result<(Report, Resolution)> {
    let folder = Folder::open(patch)?;
    let src = Source {
        folder: &folder,
        base,
        oodle,
    };
    let resolution = resolve(input, &blend_sections(&src)?)?;
    let requests: Vec<Request> = resolution
        .keys
        .values()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|&key| Request {
            key,
            target: Target::Decal,
            donor: decal.clone(),
        })
        .collect();
    let report = vblend::apply(patch, out, base, oodle, stock, &requests, &mut |s| {
        blend_values(s)
    })?;
    Ok((report, resolution))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(
            meshset_stem("world/custom/level/meshes/t_1_-5___section_1_103_0"),
            "t_1_-5"
        );
        assert_eq!(meshset_stem("world/custom/level/meshes/Plain"), "plain");
        // a GLB of one primitive: the object is named after the GLB alone
        assert_eq!(
            meshset_stem("world/custom/dingolevel_reskate_legion/meshes/t_0_-5_v23_70_0"),
            "t_0_-5_v23"
        );
        assert_eq!(meshset_stem("world/custom/level/meshes/t_1_2_7_0"), "t_1_2");
        assert_eq!(glb_stem("meshes/T_0_-6.glb"), "t_0_-6");
    }

    /// Studio merges the pieces of objects at one placement into one MeshSet named after one of them: a GLB with no
    /// MeshSet of its own is resolved in its placement peers' MeshSets, and failing those (Studio's cell prototypes,
    /// named after no object) among every key of the Patch; weights no section holds are refused.
    #[test]
    fn a_glb_merged_into_a_peers_meshset_is_found_there() {
        let sections = [
            section("t_a", 7, &[0.0, 0.25, 1.0]),
            section("t_a", 9, &[0.0, 0.5, 1.0]),
        ];
        let rec = record(&[("ma", "t_a", 0), ("mb", "t_b", 0)]);
        let weights = |mesh: &str, _: usize| -> Result<Vec<f32>> {
            Ok(match mesh {
                "meshes/t_a.glb" => vec![0.0, 0.25, 1.0],
                _ => vec![0.0, 0.5, 1.0],
            })
        };
        let peers = Peers::from([
            ("t_a".to_owned(), BTreeSet::from(["t_b".to_owned()])),
            ("t_b".to_owned(), BTreeSet::from(["t_a".to_owned()])),
        ]);
        let r = resolve_by(&rec, &sections, &BTreeMap::new(), &peers, weights).unwrap();
        assert_eq!((r.keys["ma"], r.keys["mb"]), (7, 9));
        // without peers: found among every key of the Patch
        let r = resolve_by(&rec, &sections, &BTreeMap::new(), &Peers::new(), weights).unwrap();
        assert_eq!((r.keys["ma"], r.keys["mb"]), (7, 9));
        // a weight no section holds: refused
        let none = |_: &str, _: usize| -> Result<Vec<f32>> { Ok(vec![0.0, 0.375, 1.0]) };
        let e = resolve_by(&rec, &sections, &BTreeMap::new(), &peers, none).unwrap_err();
        assert!(e.to_string().contains("or the whole Patch"), "{e}");
    }

    /// A material's primitives may not agree on a key among their own MeshSets (one primitive's own MeshSet holds a
    /// key of another material that fits it; the material's key is in a cell prototype named after no object): the
    /// material then takes the key every one of its primitives fits in the whole Patch.
    #[test]
    fn a_material_whose_own_keys_disagree_takes_its_key_from_the_whole_patch() {
        let sections = [
            section("t_a", 5, &[0.0, 0.25, 1.0]),
            section("d2_x1_z1", 8, &[0.0, 0.25, 0.5, 1.0]),
        ];
        let rec = record(&[("m", "t_a", 0), ("m", "t_b", 0), ("n", "t_a", 1)]);
        let weights = |mesh: &str, p: usize| -> Result<Vec<f32>> {
            Ok(match (mesh, p) {
                ("meshes/t_b.glb", _) => vec![0.0, 0.5, 1.0],
                _ => vec![0.0, 0.25, 1.0],
            })
        };
        let r = resolve_by_files(&rec, &sections, weights).unwrap();
        assert_eq!((r.keys["m"], r.keys["n"]), (8, 5));
    }

    /// [`resolve_by`] for a map whose objects are named after their GLBs (no map.json names).
    fn resolve_by_files(
        record: &Record,
        sections: &[BlendSection],
        weights_of: impl Fn(&str, usize) -> Result<Vec<f32>>,
    ) -> Result<Resolution> {
        resolve_by(record, sections, &BTreeMap::new(), &Peers::new(), weights_of)
    }

    fn h(w: &[f32]) -> Vec<u16> {
        w.iter().map(|&x| half::f16::from_f32(x).to_bits()).collect()
    }

    fn section(mesh: &str, key: u64, w: &[f32]) -> BlendSection {
        BlendSection {
            mesh: format!("world/custom/level/meshes/{mesh}___section_1_0_0"),
            lod: 0,
            section: 0,
            key,
            weights: h(w),
        }
    }

    fn record(entries: &[(&str, &str, usize)]) -> Record {
        let mut materials: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
        for &(m, mesh, p) in entries {
            materials
                .entry(m.into())
                .or_default()
                .push((format!("meshes/{mesh}.glb"), p));
        }
        Record { materials }
    }

    /// Primitive weights: `a` (0..2) of `t_0`, `b` (3) of `t_0` and `b` (0) of `t_1`.
    fn weights(mesh: &str, p: usize) -> Result<Vec<f32>> {
        Ok(match (mesh, p) {
            ("meshes/t_0.glb", 2) => vec![0.0, 0.25, 0.5, 1.0],
            ("meshes/t_0.glb", 3) => vec![0.0, 0.75, 1.0],
            ("meshes/t_1.glb", 0) => vec![0.75, 0.125],
            ("meshes/t_1.glb", 7) => vec![0.6],
            ("meshes/t_1.glb", 9) | ("meshes/t_0.glb", 9) => vec![0.0, 1.0, 1.0],
            _ => vec![0.25],
        })
    }

    /// Studio-like sections: tessellation adds weights between the source ones, the keys in primitive order.
    fn sections() -> Vec<BlendSection> {
        vec![
            section("t_0", 0xa, &[0.0, 0.25, 0.375, 0.5, 1.0]),
            section("t_0", 0xb, &[0.0, 0.75, 0.875, 1.0]),
            section("t_1", 0xb, &[0.75, 0.125, 0.5]),
        ]
    }

    fn message(r: Result<Resolution>) -> String {
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn primitives_resolve_by_contained_weights() {
        let rec = record(&[("a", "t_0", 2), ("b", "t_0", 3), ("b", "t_1", 0)]);
        let r = resolve_by_files(&rec, &sections(), weights).unwrap();
        assert_eq!(r.keys, BTreeMap::from([("a".into(), 0xa), ("b".into(), 0xb)]));
        // a's weights 0.25 and 0.5: key b contains neither; b's 0.75 in t_0: key a does not contain it
        assert_eq!(r.primitives[0].runner_up, 0.0);
        assert_eq!(r.primitives[0].distinct_weights, 2);
        assert!(r.in_primitive_order["t_0"]);
        // the t_1 primitive: 0.75 and 0.125 both in b's t_1 section only (a has no t_1 section)
        assert_eq!(r.primitives[2].key, 0xb);
    }

    #[test]
    fn section_order_is_reported_not_required() {
        let rec = record(&[("a", "t_0", 2), ("b", "t_0", 3)]);
        let mut s = sections();
        s.truncate(2);
        // b's section ahead of a's: still resolved, reported as out of primitive order
        s.reverse();
        let r = resolve_by_files(&rec, &s, weights).unwrap();
        assert_eq!(r.keys, BTreeMap::from([("a".into(), 0xa), ("b".into(), 0xb)]));
        assert!(!r.in_primitive_order["t_0"]);
    }

    #[test]
    fn only_zero_and_one_resolve_to_the_glbs_only_key() {
        // t_1's MeshSets have one vertex-blend key: a primitive of only 0 and 1 is that key
        let r = resolve_by_files(&record(&[("b", "t_1", 9)]), &sections()[2..], weights).unwrap();
        assert_eq!(r.keys, BTreeMap::from([("b".into(), 0xb)]));
        assert_eq!(r.primitives[0].distinct_weights, 0);
        // and is still one material per key
        let e = message(resolve_by_files(
            &record(&[("a", "t_1", 0), ("b", "t_1", 9)]),
            &sections()[2..],
            weights,
        ));
        assert!(e.contains("resolve to one key"), "{e}");
    }

    /// A key belongs to one material: a material whose weights two keys contain takes the one no other material has.
    #[test]
    fn a_key_claimed_by_another_material_is_ruled_out() {
        let mut s = sections();
        // c's sections contain a's weights too: a could be 0xa or 0xc, but 0xc is d's (its only possible key)
        s.push(section("t_0", 0xc, &[0.0, 0.25, 0.5, 0.625]));
        let r = resolve_by_files(
            &record(&[("a", "t_0", 2), ("b", "t_0", 3), ("b", "t_1", 0), ("d", "t_0", 8)]),
            &s,
            |mesh, p| match (mesh, p) {
                ("meshes/t_0.glb", 8) => Ok(vec![0.625]),
                _ => weights(mesh, p),
            },
        )
        .unwrap();
        assert_eq!(
            r.keys,
            BTreeMap::from([("a".into(), 0xa), ("b".into(), 0xb), ("d".into(), 0xc)])
        );
    }

    /// A weight exactly halfway between two halves (0.968994140625: halves 0.96875 and 0.96923828125, worked by
    /// hand) is found whichever of them Studio stored; a weight off the tie allows only its nearest half.
    #[test]
    fn a_tie_weight_matches_either_half() {
        let tie = 0.968_994_14_f32;
        assert_eq!(f64::from(tie), 0.968_994_140_625);
        let [even, away] = half_choices(tie);
        assert_eq!(
            (
                half::f16::from_bits(even).to_f32(),
                half::f16::from_bits(away).to_f32()
            ),
            (0.968_75, 0.968_75 + 1.0 / 2048.0)
        );
        assert_eq!(half_choices(0.968_8), [half::f16::from_f32(0.968_8).to_bits(); 2]);
        let away = half::f16::from_bits(away).to_f32();
        let s = vec![section("t_0", 0xa, &[0.0, away, 0.25])];
        let w = |_: &str, _: usize| Ok(vec![tie, 0.25]);
        let r = resolve_by_files(&record(&[("x", "t_0", 0)]), &s, w).unwrap();
        assert_eq!(r.keys["x"], 0xa);
        let off = |_: &str, _: usize| Ok(vec![0.968_8, 0.25]);
        let e = message(resolve_by_files(&record(&[("x", "t_0", 0)]), &s, off));
        assert!(e.contains("contain all 2 of its weights"), "{e}");
    }

    /// Two materials whose weights fit the same two keys (decals faded by one alpha value) take one key each; which
    /// takes which does not change the stage's output.
    #[test]
    fn interchangeable_materials_take_distinct_keys() {
        let s = vec![
            section("t_0", 0xa, &[0.0, 0.5, 1.0]),
            section("t_0", 0xb, &[0.0, 0.5, 1.0]),
        ];
        let half = |_: &str, _: usize| Ok(vec![0.5, 1.0]);
        let r = resolve_by_files(&record(&[("x", "t_0", 0), ("y", "t_0", 1)]), &s, half).unwrap();
        assert_eq!(r.interchangeable, 2);
        let keys: BTreeSet<u64> = r.keys.values().copied().collect();
        assert_eq!(keys, BTreeSet::from([0xa, 0xb]));
        // three such materials for two keys: one is left without a key
        let e = message(resolve_by_files(
            &record(&[("x", "t_0", 0), ("y", "t_0", 1), ("z", "t_0", 2)]),
            &s,
            half,
        ));
        assert!(e.contains("no vertex-blend key left"), "{e}");
    }

    #[test]
    fn every_ambiguity_is_refused() {
        let s = sections();
        // only 0 and 1 in a GLB of two vertex-blend keys, and no second material for the other key: a key is left
        // unclaimed
        let e = message(resolve_by_files(&record(&[("a", "t_0", 9)]), &s, weights));
        assert!(
            e.contains("keys no recorded material resolves to: 000000000000000b"),
            "{e}"
        );
        // no containing key in the GLB's MeshSets nor anywhere in the Patch
        let e = message(resolve_by_files(&record(&[("a", "t_1", 7)]), &s, weights));
        assert!(
            e.contains("0 vertex-blend keys contain all 1 of its weights"),
            "{e}"
        );
        // two keys contain a's weights, one material for them: the other key is left unclaimed
        let mut two = s.clone();
        two.push(section("t_0", 0xc, &[0.25, 0.5]));
        let e = message(resolve_by_files(
            &record(&[("a", "t_0", 2), ("b", "t_0", 3)]),
            &two,
            weights,
        ));
        assert!(
            e.contains("keys no recorded material resolves to: 000000000000000c"),
            "{e}"
        );
        // one material whose primitives allow no common key
        let e = message(resolve_by_files(
            &record(&[("a", "t_0", 2), ("a", "t_0", 3)]),
            &s,
            weights,
        ));
        assert!(e.contains("material a: no vertex-blend key left"), "{e}");
        // two materials, one key
        let e = message(resolve_by_files(
            &record(&[("a", "t_0", 2), ("b", "t_0", 3), ("c", "t_1", 0)]),
            &s,
            weights,
        ));
        assert!(e.contains("materials b and c resolve to one key"), "{e}");
        // a vertex-blend key no record claims (discovery cross-check)
        let e = message(resolve_by_files(&record(&[("a", "t_0", 2)]), &s, weights));
        assert!(
            e.contains("keys no recorded material resolves to: 000000000000000b"),
            "{e}"
        );
        // a Patch without vertex-blend sections names the Studio build
        let e = message(resolve_by_files(&record(&[("a", "t_9", 0)]), &[], weights));
        assert!(e.contains(STUDIO_BUILD), "{e}");
    }

    #[test]
    fn glbs_sharing_a_stem_are_refused() {
        let rec = Record {
            materials: BTreeMap::from([
                ("a".into(), vec![("a/t_0.glb".into(), 2)]),
                ("b".into(), vec![("b/T_0.glb".into(), 3)]),
            ]),
        };
        let e = message(resolve_by_files(&rec, &sections(), weights));
        assert!(e.contains("share a file stem"), "{e}");
    }

    #[test]
    fn values_need_the_weight_and_never_write_the_sentinel() {
        let input = |blend| SectionInput {
            mesh: "m",
            lod: 0,
            section: 0,
            key: 1,
            positions: &[],
            uv0: &[],
            blend,
        };
        assert!(
            blend_values(&input(None))
                .unwrap_err()
                .to_string()
                .contains(STUDIO_BUILD)
        );
        let v = blend_values(&input(Some(&[0.0, 0.5, 1.0]))).unwrap();
        assert_eq!(v.color0, vec![[0, 0, 0, 255], [128, 0, 0, 255], [255, 0, 0, 255]]);
        assert!(vblend::check_values(&Target::Decal, 3, &v).is_ok());
    }

    #[test]
    fn zero_and_one_are_not_distinctive() {
        for w in [0.0f32, -0.0, 1.0] {
            assert!(!distinctive(half::f16::from_f32(w).to_bits()), "{w}");
        }
        assert!(distinctive(half::f16::from_f32(0.5).to_bits()));
    }
}
