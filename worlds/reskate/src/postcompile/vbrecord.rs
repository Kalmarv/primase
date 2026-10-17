//! The map writer's vertex-blend record (`vertex_blend.json`) resolved against Studio's compile: which material keys
//! of the Patch are the recorded `vertex_blend` materials, and the per-vertex values their sections get.
//!
//! The record decides which materials are vertex blends: `{"format": 1, "materials": {name: [[mesh, primitive],
//! ...]}}`, each (GLB, primitive) of the map drawing the material. Studio's material keys are not a function of the
//! material's name (they appear to be derived from its contents) and its output names no material, so each recorded
//! primitive is found in the compile by its weights:
//! - Studio compiles a `vertex_blend` material's surfaces with `TexCoord0:Half4`, the weight in `z`
//!   ([`crate::postcompile::vblend`]); it names a MeshSet after the first map object drawing the GLB (`" / "` and
//!   spaces become `___` and `_`), and the writer names objects `<GLB stem> / section N`, so the MeshSets of a GLB
//!   are the ones named `<stem>___section_...` (a map not written by the writer may not follow this);
//! - Studio tessellates and re-wraps those surfaces (about five times the triangles), so section vertices are not the
//!   primitive's vertices; but tessellation keeps the source vertices' weights, so every distinct weight of the
//!   vertices the primitive draws (Studio drops the others: a lowered overlay shares its source primitive's
//!   vertices), as a half float, occurs in its key's sections;
//! - a primitive resolves to the one key, among the vertex-blend keys of its GLB's MeshSets, whose sections contain
//!   all of its weights other than 0 and 1 (those are not distinctive).
//!
//! Validated on ReSkate Studio 20261012 ([`STUDIO_BUILD`]): on a real layered scene every recorded primitive's weights
//! were all contained in exactly one key, the next-best key containing at most 46% of them, and the sections followed
//! the GLB's primitive order. The rule assumes that tessellation keeps the source weights and that `TexCoord0.z` is
//! stored as a half float; a different Studio build should be re-checked against both ([`Resolution`] carries the
//! margins). Every ambiguity is refused rather than guessed: a primitive with no distinctive weight, none or several
//! containing keys, a material resolving to two keys, two materials to one key, and a vertex-blend key no record
//! claims (the discovery cross-check).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use primase_compress::oodle::Oodle;

use crate::postcompile::fb::MESHSET;
use crate::postcompile::fb::folder::{Folder, Source};
use crate::postcompile::meshset;
use crate::postcompile::vblend::{
    self, Donor, Report, Request, SectionInput, SectionValues, Target, decal_opacity, lod_geometry,
    patch_bundles, surface,
};
use crate::postcompile::{Error, Result, bad};

const W: &str = "vertex-blend record";

/// The Studio build the weight match was established on.
pub const STUDIO_BUILD: &str = "ReSkate Studio 20261012";

/// The record's file name in the map folder.
pub const RECORD: &str = "vertex_blend.json";

/// `vertex_blend.json`: material name -> the (GLB relative to the map folder, primitive) pairs drawing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub materials: BTreeMap<String, Vec<(String, usize)>>,
}

impl Record {
    /// The record of map folder `map`; `None` when the map has no vertex-blend material (no record).
    pub fn read(map: &Path) -> Result<Option<Record>> {
        let path = map.join(RECORD);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::Io { path, source }),
        };
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| bad(W, 0, format!("{RECORD}: {e}")))?;
        if v["format"] != 1 {
            return Err(bad(W, 0, format!("{RECORD}: format {} is not 1", v["format"])));
        }
        let materials = v["materials"]
            .as_object()
            .ok_or_else(|| bad(W, 0, format!("{RECORD}: no materials object")))?
            .iter()
            .map(|(name, pairs)| {
                let pairs = pairs
                    .as_array()
                    .ok_or_else(|| bad(W, 0, format!("{RECORD}: {name} is not a list")))?
                    .iter()
                    .map(|p| match (p[0].as_str(), p[1].as_u64()) {
                        (Some(mesh), Some(prim)) => Ok((mesh.to_owned(), prim as usize)),
                        _ => Err(bad(
                            W,
                            0,
                            format!("{RECORD}: {name}: {p} is not [mesh, primitive]"),
                        )),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((name.clone(), pairs))
            })
            .collect::<Result<_>>()?;
        Ok(Some(Record { materials }))
    }
}

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
}

/// The GLB stem in a Studio MeshSet's name (`.../<stem>___section_...`, from the writer's object name), lowercase.
fn meshset_stem(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.split("___").next().unwrap_or(base).to_ascii_lowercase()
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

/// The `_RESKATE_BLEND` weights of the vertices `primitive` of the map GLB at `path` draws (all of them without
/// indices), in vertex order.
fn glb_weights(path: &Path, primitive: usize) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let fail = |d: String| bad(W, 0, format!("{}: {d}", path.display()));
    let g = primase_gltf::Gltf::from_glb(&bytes).map_err(|e| fail(e.to_string()))?;
    let j = g.json();
    // the writer writes one mesh per GLB, and the record's primitive indices are that mesh's
    let meshes = j["meshes"].as_array().map_or(0, Vec::len);
    if meshes != 1 {
        return Err(fail(format!("{meshes} meshes, not the writer's one")));
    }
    let attr = &j["meshes"][0]["primitives"][primitive]["attributes"]["_RESKATE_BLEND"];
    let a = &j["accessors"][attr
        .as_u64()
        .ok_or_else(|| fail(format!("primitive {primitive} has no _RESKATE_BLEND")))?
        as usize];
    if a["componentType"] != 5126 || a["type"] != "SCALAR" {
        return Err(fail(format!(
            "primitive {primitive}: _RESKATE_BLEND is not a float scalar"
        )));
    }
    let view = a["bufferView"]
        .as_u64()
        .ok_or_else(|| fail("_RESKATE_BLEND has no buffer view".into()))? as usize;
    if j["bufferViews"][view].get("byteStride").is_some_and(|s| s != 4) {
        return Err(fail("_RESKATE_BLEND is interleaved".into()));
    }
    let data = g.buffer_view(view).map_err(|e| fail(e.to_string()))?;
    let past = |e: String| fail(format!("_RESKATE_BLEND runs past its buffer view: {e}"));
    let int = |k: &str| usize::try_from(a[k].as_u64().unwrap_or(0)).map_err(|e| past(e.to_string()));
    let span = primase_bytes::checked_span(int("byteOffset")?, int("count")?, 4, data.len())
        .map_err(|e| past(e.to_string()))?;
    let weights: Vec<f32> = data[span]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    // Studio compiles only the vertices the primitive's triangles use; a lowered overlay shares its source
    // primitive's vertices, and the unused ones may carry weights of other triangles
    let Some(ix) = j["meshes"][0]["primitives"][primitive]["indices"].as_u64() else {
        return Ok(weights);
    };
    let ia = &j["accessors"][ix as usize];
    let size = match ia["componentType"].as_u64() {
        Some(5121) => 1,
        Some(5123) => 2,
        Some(5125) => 4,
        _ => {
            return Err(fail(format!(
                "primitive {primitive}: indices are not unsigned integers"
            )));
        }
    };
    let iview = ia["bufferView"]
        .as_u64()
        .ok_or_else(|| fail("indices have no buffer view".into()))? as usize;
    if j["bufferViews"][iview].get("byteStride").is_some() {
        return Err(fail("indices are interleaved".into()));
    }
    let idata = g.buffer_view(iview).map_err(|e| fail(e.to_string()))?;
    let iint = |k: &str| usize::try_from(ia[k].as_u64().unwrap_or(0)).map_err(|e| past(e.to_string()));
    let ispan = primase_bytes::checked_span(iint("byteOffset")?, iint("count")?, size, idata.len())
        .map_err(|e| fail(format!("indices run past their buffer view: {e}")))?;
    let mut used = vec![false; weights.len()];
    for c in idata[ispan].chunks_exact(size) {
        let i = match size {
            1 => usize::from(c[0]),
            2 => usize::from(u16::from_le_bytes([c[0], c[1]])),
            _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize,
        };
        *used.get_mut(i).ok_or_else(|| {
            fail(format!(
                "primitive {primitive}: index {i} past its {} vertices",
                weights.len()
            ))
        })? = true;
    }
    Ok(weights
        .into_iter()
        .zip(used)
        .filter(|(_, u)| *u)
        .map(|(w, _)| w)
        .collect())
}

/// Resolves every recorded primitive of map folder `map` among `sections` (from [`blend_sections`]).
pub fn resolve(record: &Record, map: &Path, sections: &[BlendSection]) -> Result<Resolution> {
    resolve_by(record, sections, |mesh, primitive| {
        glb_weights(&map.join(mesh), primitive)
    })
}

/// [`resolve`] with the recorded primitives' weights from `weights_of(mesh, primitive)`.
fn resolve_by(
    record: &Record,
    sections: &[BlendSection],
    weights_of: impl Fn(&str, usize) -> Result<Vec<f32>>,
) -> Result<Resolution> {
    // stem -> key -> the weights of its sections; and each stem's keys in section order
    let mut sets: BTreeMap<String, BTreeMap<u64, HashSet<u16>>> = BTreeMap::new();
    let mut order: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for s in sections {
        let stem = meshset_stem(&s.mesh);
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
    let mut primitives = Vec::new();
    let mut keys: BTreeMap<String, u64> = BTreeMap::new();
    let mut owner: BTreeMap<u64, String> = BTreeMap::new();
    for (material, pairs) in &record.materials {
        for (mesh, primitive) in pairs {
            let at = format!("material {material} in {mesh} primitive {primitive}");
            let stem = glb_stem(mesh);
            let candidates = sets.get(&stem).ok_or_else(|| {
                bad(
                    W,
                    0,
                    format!(
                        "{at}: no MeshSet of {mesh} has a vertex-blend section (TexCoord0:Half4); the compile did \
                         not carry the weights (vertex_blend needs {STUDIO_BUILD} or a build checked against it)"
                    ),
                )
            })?;
            let weights: BTreeSet<u16> = weights_of(mesh, *primitive)?
                .iter()
                .map(|&w| half::f16::from_f32(w).to_bits())
                .filter(|&b| distinctive(b))
                .collect();
            if weights.is_empty() {
                return Err(bad(
                    W,
                    0,
                    format!("{at}: only weights 0 and 1, nothing identifies its section"),
                ));
            }
            let contained: Vec<(u64, usize)> = candidates
                .iter()
                .map(|(&k, set)| (k, weights.iter().filter(|w| set.contains(w)).count()))
                .collect();
            let full: Vec<u64> = contained
                .iter()
                .filter(|&&(_, n)| n == weights.len())
                .map(|&(k, _)| k)
                .collect();
            let [key] = full[..] else {
                return Err(bad(
                    W,
                    0,
                    format!(
                        "{at}: {} vertex-blend keys of {mesh} contain all {} of its weights ({})",
                        full.len(),
                        weights.len(),
                        full.iter()
                            .map(|k| format!("{k:016x}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            };
            let runner_up = contained
                .iter()
                .filter(|&&(k, _)| k != key)
                .map(|&(_, n)| n as f64 / weights.len() as f64)
                .fold(0.0, f64::max);
            match keys.get(material) {
                Some(&k) if k != key => {
                    return Err(bad(
                        W,
                        0,
                        format!("material {material} resolves to two keys, {k:016x} and {key:016x}"),
                    ));
                }
                _ => {
                    keys.insert(material.clone(), key);
                }
            }
            if let Some(other) = owner.insert(key, material.clone())
                && other != *material
            {
                return Err(bad(
                    W,
                    0,
                    format!("materials {other} and {material} resolve to one key, {key:016x}"),
                ));
            }
            primitives.push(Resolved {
                material: material.clone(),
                mesh: mesh.clone(),
                primitive: *primitive,
                key,
                distinct_weights: weights.len(),
                runner_up,
            });
        }
    }
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
    })
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

/// Applies map folder `map`'s record to Patch `patch` into `out` ([`vblend::apply`]): every recorded material's key
/// onto the stock decal row of `decal`, its opacities from Studio's weights. `None` (and nothing written) when the
/// map has no record.
pub fn apply(
    patch: &Path,
    out: &Path,
    map: &Path,
    base: Option<&Folder>,
    oodle: Option<&Oodle>,
    decal: &Donor,
) -> Result<Option<(Report, Resolution)>> {
    let Some(record) = Record::read(map)? else {
        return Ok(None);
    };
    let folder = Folder::open(patch)?;
    let src = Source {
        folder: &folder,
        base,
        oodle,
    };
    let resolution = resolve(&record, map, &blend_sections(&src)?)?;
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
    let report = vblend::apply(patch, out, base, oodle, &requests, &mut |s| blend_values(s))?;
    Ok(Some((report, resolution)))
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
        assert_eq!(glb_stem("meshes/T_0_-6.glb"), "t_0_-6");
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
            ("meshes/t_1.glb", 9) => vec![0.0, 1.0, 1.0],
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
        let r = resolve_by(&rec, &sections(), weights).unwrap();
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
        let r = resolve_by(&rec, &s, weights).unwrap();
        assert_eq!(r.keys, BTreeMap::from([("a".into(), 0xa), ("b".into(), 0xb)]));
        assert!(!r.in_primitive_order["t_0"]);
    }

    #[test]
    fn every_ambiguity_is_refused() {
        let s = sections();
        // only 0 and 1: nothing identifies the section; the error names material and GLB
        let e = message(resolve_by(&record(&[("a", "t_1", 9)]), &s, weights));
        assert!(
            e.contains("only weights 0 and 1") && e.contains("material a in meshes/t_1.glb"),
            "{e}"
        );
        // no containing key
        let e = message(resolve_by(&record(&[("a", "t_1", 5)]), &s, weights));
        assert!(e.contains("0 vertex-blend keys of meshes/t_1.glb contain"), "{e}");
        // two containing keys
        let mut two = s.clone();
        two.push(section("t_0", 0xc, &[0.25, 0.5]));
        let e = message(resolve_by(
            &record(&[("a", "t_0", 2), ("b", "t_0", 3)]),
            &two,
            weights,
        ));
        assert!(e.contains("2 vertex-blend keys"), "{e}");
        // one material, two keys
        let e = message(resolve_by(
            &record(&[("a", "t_0", 2), ("a", "t_0", 3)]),
            &s,
            weights,
        ));
        assert!(e.contains("material a resolves to two keys"), "{e}");
        // two materials, one key
        let e = message(resolve_by(
            &record(&[("a", "t_0", 2), ("b", "t_0", 3), ("c", "t_1", 0)]),
            &s,
            weights,
        ));
        assert!(e.contains("materials b and c resolve to one key"), "{e}");
        // a vertex-blend key no record claims (discovery cross-check)
        let e = message(resolve_by(&record(&[("a", "t_0", 2)]), &s, weights));
        assert!(
            e.contains("keys no recorded material resolves to: 000000000000000b"),
            "{e}"
        );
        // a recorded GLB without vertex-blend sections names the Studio build
        let e = message(resolve_by(&record(&[("a", "t_9", 0)]), &s, weights));
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
        let e = message(resolve_by(&rec, &sections(), weights));
        assert!(e.contains("share a file stem"), "{e}");
    }

    /// A GLB with `meshes` copies of one primitive whose `_RESKATE_BLEND` accessor has `count` floats, over a buffer
    /// of 0.25 and 0.5.
    fn glb_file(name: &str, meshes: usize, count: u64) -> std::path::PathBuf {
        let mesh = serde_json::json!({"primitives": [{"attributes": {"_RESKATE_BLEND": 0}}]});
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 8}],
            "bufferViews": [{"buffer": 0, "byteLength": 8}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": count, "type": "SCALAR"}],
            "meshes": vec![mesh; meshes],
        });
        let bin: Vec<u8> = [0.25f32, 0.5].iter().flat_map(|w| w.to_le_bytes()).collect();
        let bytes = primase_gltf::glb::assemble(json.to_string().as_bytes(), &bin).unwrap();
        let path = std::env::temp_dir().join(format!("vbrecord-{name}-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn glb_weights_are_read_strictly() {
        let ok = glb_file("ok", 1, 2);
        assert_eq!(glb_weights(&ok, 0).unwrap(), vec![0.25, 0.5]);
        let two = glb_file("two", 2, 2);
        let e = glb_weights(&two, 0).unwrap_err().to_string();
        assert!(e.contains("2 meshes"), "{e}");
        // a count whose byte length overflows is an error, not a panic
        let huge = glb_file("huge", 1, u64::MAX / 2);
        let e = glb_weights(&huge, 0).unwrap_err().to_string();
        assert!(e.contains("runs past its buffer view"), "{e}");
        for p in [ok, two, huge] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// A GLB whose primitive has four weights and the u16 indices `idx`.
    fn indexed_glb(name: &str, idx: &[u16]) -> std::path::PathBuf {
        let mesh = serde_json::json!({"primitives": [{"attributes": {"_RESKATE_BLEND": 0}, "indices": 1}]});
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 16 + 2 * idx.len()}],
            "bufferViews": [{"buffer": 0, "byteLength": 16}, {"buffer": 0, "byteOffset": 16, "byteLength": 2 * idx.len()}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "SCALAR"},
                {"bufferView": 1, "componentType": 5123, "count": idx.len(), "type": "SCALAR"},
            ],
            "meshes": [mesh],
        });
        let mut bin: Vec<u8> = [0.1f32, 0.2, 0.3, 0.4]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        bin.extend(idx.iter().flat_map(|i| i.to_le_bytes()));
        let bytes = primase_gltf::glb::assemble(json.to_string().as_bytes(), &bin).unwrap();
        let path = std::env::temp_dir().join(format!("vbrecord-{name}-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn only_the_vertices_a_primitive_draws_give_its_weights() {
        // vertex 2 is not drawn (an overlay sharing its source primitive's vertices): its weight is not the
        // primitive's, since Studio compiles only drawn vertices
        let p = indexed_glb("idx", &[0, 1, 3]);
        assert_eq!(glb_weights(&p, 0).unwrap(), vec![0.1, 0.2, 0.4]);
        let bad = indexed_glb("idx-bad", &[0, 1, 7]);
        let e = glb_weights(&bad, 0).unwrap_err().to_string();
        assert!(e.contains("index 7 past its 4 vertices"), "{e}");
        for p in [p, bad] {
            let _ = std::fs::remove_file(p);
        }
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
