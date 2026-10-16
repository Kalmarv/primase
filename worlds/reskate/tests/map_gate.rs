//! M3 gate: the Rust map writer against the reference C# writer's output, file by file, compared by what Studio
//! reads.
//!
//! For every area folder in `PRIMASE_RESKATE_REF` (the reference writer's map folders) with a scene folder of the
//! same name in `PRIMASE_RESKATE_SCENES`, the scene is built with the gate's options (`--tex-cap 256 --tex-caps
//! PRIMASE_RESKATE_CAPS --water-ocean --lights-max-per-cell 256`, `--piece-log` where the reference has a
//! pieces.json) into `PRIMASE_OUT`, and every file is compared:
//! - JSON files as parsed values (`primase_core::gate::compare_json`: keys and structure, array order kept, integers
//!   exactly, other numbers as f64 or else as f32, counted);
//! - GLB files by their JSON chunk, as parsed values, and their binary chunk, exactly; stored attributes (texture
//!   coordinates, indices) always exactly, derived normals (shared vector maths against the reference's) per
//!   component within four f32 epsilon at max(1, |reference|), counted; positions exactly, but those the overlay stack
//!   lifted along a derived normal (by the writer's lift record, `BuildReport::lifts`) within the lift times that
//!   bound plus two f32 steps at the vertex's largest |coordinate|, since the lift distance is itself derived
//!   (positions dotted with derived normals), and `stats.json` `overlay_stack.max_shift_m` within four f32 epsilon
//!   relative, both counted ('decal lift along derived normal'); POSITION bounds those of the data, by value;
//!
//! except for the order of a mesh's primitives. We draw a mesh's parts in a semantic order (draw class, then under
//! before over by the overlay stack, then first use; a unit test checks it), the reference writer by material name,
//! which ends in a content hash. Where only that order differs, a GLB is compared with its primitives as a set
//! (each its paired material, accessors and data) and `map.json` with each mesh section's object by its name without
//! the section number, its mesh and material; every object's primitive index must point at a primitive of its
//! material. Each such file is counted and named, the ones whose reordered primitives include a decal, alpha-tested
//! or blended material apart;
//! - PNG files by IHDR (size, bit depth, colour type, interlace), PLTE and tRNS, the set of other chunk kinds
//!   (besides IDAT and IEND: none may change the meaning), and the decoded pixels: exactly, except that a texture
//!   whose source is block compressed (BC1-5, by the reference's `textures_index.json`) may differ by 1 per channel
//!   (we decode BCn by the D3D rules, the reference with CodeWalker's arithmetic); each such texture is counted and
//!   listed in `PRIMASE_OUT/reskate-gate/bcn_within_1.txt`; `ui/map_image.png` (drawn in those textures' mean
//!   colours and shaded by slope from derived normals) within 2 per channel;
//! - Normal PNGs referenced by materials: outside-disk XY with rebuilt Z is projected to unit length,
//!   counted per pixel with encoded direction change <= 0.31 degrees; other pixels keep the codec channel bound.
//! - DDS files by their header fields and payload, exactly, except that a single-mip texture may have the
//!   `DDSD_MIPMAPCOUNT` flag on one side only (the DDS spec sets it for mipmapped textures, the reference writer
//!   always), each such file counted and named;
//! - every other file by bytes.
//!
//! Where the reference rounds what it writes (to 4 decimals: light values, spawn, travel and trainer positions, the
//! budget's base colour factors, the grind length; to 2 decimals in pieces.json, our own log), our unrounded f32 may
//! differ from its number by the rounding (plus one f32 step), counted and named per rounding.
//!
//! The reference's JSON is read as Studio reads it: its negative zeros, written as the integer `-0`, are 0 (counted);
//! we write every zero unsigned.
//!
//! Not compared: `stats.json` `totals.seconds` (run time), and our own values that depend on our encoders: the PNG
//! compressor and the GLB JSON text (`textures_index.json` `bytes` of `.png` entries, `stats.json`
//! `totals.texture_file_bytes` and `totals.mesh_file_bytes`).
//!
//! Texture file names end in a hash of the written pixels and material names in a hash of their textures, so a
//! texture whose pixels differ by the BCn arithmetic has another name, and so do the materials using it. Before
//! comparing, our renamed textures are paired with the reference's by the sources they were made from, and our
//! renamed materials by their name without the hash and their record (textures paired); every paired name is
//! compared as the reference's, and the pairs are counted.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::gate::{GateReport, JsonCompare, Tolerance, compare_json};
use primase_reskate::BuildOptions;
use primase_testdata::real_data;
use serde_json::Value;

fn files(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, p);
            }
        }
    }
    out
}

/// A PNG chunk: kind and data.
type Chunk = ([u8; 4], Vec<u8>);

/// The chunks of a PNG, in order.
fn chunks(b: &[u8]) -> Result<Vec<Chunk>, String> {
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("not a PNG file".into());
    }
    let mut out = Vec::new();
    let mut o = 8;
    while o + 8 <= b.len() {
        let len = u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = b[o + 4..o + 8].try_into().unwrap();
        let data = b.get(o + 8..o + 8 + len).ok_or("a chunk runs past the end")?;
        out.push((kind, data.to_vec()));
        o += 12 + len;
    }
    Ok(out)
}

/// Compared as PNGs: IHDR, PLTE, tRNS and the other chunk kinds equal, then the decoded pixels' largest difference
/// per channel (0: identical); else why not.
fn png_diff(a: &[u8], b: &[u8]) -> Result<u32, String> {
    let (ca, cb) = (chunks(a)?, chunks(b)?);
    let get = |c: &[Chunk], k: &[u8; 4]| c.iter().find(|x| &x.0 == k).map(|x| x.1.clone());
    for k in [b"IHDR", b"PLTE", b"tRNS"] {
        if get(&ca, k) != get(&cb, k) {
            return Err(format!("{} differs", String::from_utf8_lossy(k)));
        }
    }
    let kinds = |c: &[Chunk]| -> BTreeSet<[u8; 4]> {
        c.iter()
            .map(|x| x.0)
            .filter(|k| k != b"IDAT" && k != b"IEND")
            .collect()
    };
    if kinds(&ca) != kinds(&cb) {
        return Err("chunk kinds differ".into());
    }
    let (pa, pb) = (
        primase_image::read_png(a).map_err(|e| e.to_string())?,
        primase_image::read_png(b).map_err(|e| e.to_string())?,
    );
    let (ia, ib) = (&pa.image, &pb.image);
    if (ia.width, ia.height) != (ib.width, ib.height) {
        return Err("sizes differ".into());
    }
    use primase_image::Samples;
    match (&ia.samples, &ib.samples) {
        (Samples::U8(x), Samples::U8(y)) if x.len() == y.len() => Ok(x
            .iter()
            .zip(y)
            .map(|(p, q)| u32::from(p.abs_diff(*q)))
            .max()
            .unwrap_or(0)),
        (Samples::U16(x), Samples::U16(y)) if x.len() == y.len() => Ok(x
            .iter()
            .zip(y)
            .map(|(p, q)| u32::from(p.abs_diff(*q)))
            .max()
            .unwrap_or(0)),
        _ => Err("sample layouts differ".into()),
    }
}

/// Normal PNGs retain the ordinary per-channel codec bound except for XY
/// outside the disk with rebuilt blue 128. Those vectors must project onto the
/// unit hemisphere and retain direction within 0.31 degrees after quantization.
fn normal_png_diff(a: &[u8], b: &[u8], codec_bound: u8) -> Result<(u64, f64), String> {
    png_diff(a, b)?; // Validate envelope and sample layout before pixel comparison.
    let a = primase_image::read_png(a).map_err(|e| e.to_string())?.image;
    let b = primase_image::read_png(b).map_err(|e| e.to_string())?.image;
    let (primase_image::Samples::U8(a), primase_image::Samples::U8(b)) = (a.samples, b.samples) else {
        return Err("normal gate requires RGBA8".into());
    };
    let mut count = 0;
    let mut maximum = 0f64;
    for (a, b) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let decode = |p: &[u8; 4]| [p[0], p[1], p[2]].map(|v| f64::from(v) / 127.5 - 1.);
        let x = decode(a);
        let y = decode(b);
        if x[0] * x[0] + x[1] * x[1] > 1. && a[2] == 128 {
            let length = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
            let dot = x.iter().zip(y).map(|(a, b)| a * b).sum::<f64>();
            let angle = (dot / (length(x) * length(y))).clamp(-1., 1.).acos().to_degrees();
            // RGB8 unit-vector quantization error is at most sqrt(3)/255.
            if angle > 0.31 || (length(y) - 1.).abs() > 3f64.sqrt() / 255. || b[2] != 128 || a[3] != b[3] {
                return Err(format!(
                    "projected normal direction {angle} degrees or unit/blue/alpha mismatch"
                ));
            }
            count += 1;
            maximum = maximum.max(angle);
        } else if a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > codec_bound) {
            return Err("unprojected normal exceeds codec channel bound".into());
        }
    }
    Ok((count, maximum))
}

#[test]
fn normal_projection_gate_measures_direction_and_rejects_wrong_vectors() {
    let png = |rgba: Vec<u8>| {
        primase_image::write_png(&primase_image::Image {
            width: 1,
            height: 1,
            samples: primase_image::Samples::U8(rgba),
        })
        .unwrap()
    };
    let a = png(vec![10, 235, 128, 255]);
    let b = png(vec![33, 214, 128, 255]);
    assert_eq!(normal_png_diff(&a, &b, 0).unwrap().0, 1);
    assert!(
        normal_png_diff(&a, &a, 0).is_err(),
        "unnormalized vector must fail"
    );
    assert!(normal_png_diff(&a, &png(vec![214, 33, 128, 255]), 0).is_err());
    assert!(normal_png_diff(&a, &png(vec![33, 214, 128, 0]), 0).is_err());
}

/// `DDSD_MIPMAPCOUNT`, the DDS header flag that says the mip count field is valid.
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;

/// How two DDS files compare: magic, every DDS_HEADER (and DX10 header) field and the payload exact, except that a
/// texture with one mip may have `DDSD_MIPMAPCOUNT` on one side only (the spec sets it for mipmapped textures; the
/// reference writer sets it always). `Ok(true)`: equal but for that flag.
fn dds_diff(a: &[u8], b: &[u8]) -> Result<bool, String> {
    let u32_at = |d: &[u8], o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    if a.len() < 128 || b.len() < 128 || a[..4] != *b"DDS " || b[..4] != *b"DDS " {
        return Err("not DDS files".into());
    }
    let dx10 = |d: &[u8]| d[84..88] == *b"DX10";
    let head = if dx10(a) { 148 } else { 128 };
    if dx10(a) != dx10(b) || a.len() < head || b.len() < head {
        return Err("DX10 header on one side only".into());
    }
    if a[head..] != b[head..] {
        return Err("payload differs".into());
    }
    let (fa, fb) = (u32_at(a, 8), u32_at(b, 8));
    let flag_only = fa != fb && fa ^ fb == DDSD_MIPMAPCOUNT && u32_at(a, 28) <= 1 && u32_at(b, 28) <= 1;
    if fa != fb && !flag_only {
        return Err(format!("header flags {fa:#x} vs {fb:#x}"));
    }
    if a[12..head] != b[12..head] || a[4..8] != b[4..8] {
        let o = (4..head).find(|&o| a[o] != b[o]).unwrap_or(4);
        return Err(format!("header differs at byte {o}"));
    }
    Ok(flag_only)
}

/// A GLB's JSON chunk, parsed, and its binary chunk.
fn glb_parts(b: &[u8]) -> Result<(Value, Vec<u8>), String> {
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    if b.len() < 20 || b[..4] != *b"glTF" || u32_at(4) != 2 || u32_at(8) != b.len() || b[16..20] != *b"JSON" {
        return Err("not a GLB 2 file".into());
    }
    let jl = u32_at(12);
    let json: Value = serde_json::from_slice(&b[20..20 + jl]).map_err(|e| e.to_string())?;
    let rest = &b[20 + jl..];
    let bin = if rest.len() >= 8 && rest[4..8] == *b"BIN\0" {
        rest[8..8 + u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize].to_vec()
    } else {
        Vec::new()
    };
    Ok((json, bin))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The reference's `textures_index.json` files (`textures/...`) whose source is block compressed (BC1-5).
fn bcn_sourced(index: &Value) -> BTreeSet<String> {
    index["textures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            let f = t["src_format"].as_str().unwrap_or("");
            [
                "DXT1", "DXT2", "DXT3", "DXT4", "DXT5", "ATI1", "ATI2", "BC4", "BC5",
            ]
            .iter()
            .any(|k| f.contains(k))
        })
        .filter_map(|t| t["file"].as_str().map(str::to_owned))
        .collect()
}

/// Our names that are the reference's under another hash: texture files (`textures/...`) and materials.
#[derive(Default)]
struct Renames {
    names: HashMap<String, String>,
    textures: usize,
    materials: usize,
}

impl Renames {
    fn get<'a>(&'a self, s: &'a str) -> &'a str {
        self.names.get(s).map_or(s, String::as_str)
    }

    /// Every string and object key of `v` that is one of our renamed names, as the reference's.
    fn apply(&self, v: &mut Value) {
        match v {
            Value::String(s) => {
                if let Some(r) = self.names.get(s.as_str()) {
                    *s = r.clone();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|x| self.apply(x)),
            Value::Object(m) => {
                let old = std::mem::take(m);
                for (k, mut x) in old {
                    self.apply(&mut x);
                    m.insert(self.get(&k).to_owned(), x);
                }
            }
            _ => {}
        }
    }
}

/// Whether our texture file has the reference's content: PNG pixels within 1, other files byte-identical.
fn same_texture(ours: &Path, theirs: &Path) -> bool {
    let (a, b) = (std::fs::read(ours).unwrap(), std::fs::read(theirs).unwrap());
    if ours.extension().is_some_and(|e| e == "png") {
        png_diff(&a, &b).is_ok_and(|d| d <= 1)
    } else {
        a == b
    }
}

/// Pair our renamed textures with the reference's: the same sources, file kind, size and source format, and among
/// several such (tinted variants of one source) the one with the same content; then our renamed materials by their
/// name without the 10-digit hash and their record with the textures paired.
fn pair_names(ref_dir: &Path, our_dir: &Path) -> Renames {
    let mut r = Renames::default();
    let (ri, oi) = (
        read_json(&ref_dir.join("textures_index.json")),
        read_json(&our_dir.join("textures_index.json")),
    );
    let files_of = |v: &Value| -> BTreeMap<String, String> {
        v["textures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let file = t["file"].as_str().unwrap();
                let key = format!(
                    "{} {} {}x{} {}",
                    t["sources"],
                    file.rsplit('.').next().unwrap_or(""),
                    t["w"],
                    t["h"],
                    t["src_format"]
                );
                (file.to_owned(), key)
            })
            .collect()
    };
    let (rf, of) = (files_of(&ri), files_of(&oi));
    let mut theirs_by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (file, key) in rf.iter().filter(|(f, _)| !of.contains_key(*f)) {
        theirs_by_key.entry(key).or_default().push(file);
    }
    for (ours, key) in of.iter().filter(|(f, _)| !rf.contains_key(*f)) {
        let candidates = theirs_by_key.get(key.as_str()).cloned().unwrap_or_default();
        let same: Vec<&str> = if candidates.len() == 1 {
            candidates
        } else {
            candidates
                .into_iter()
                .filter(|t| same_texture(&our_dir.join(ours), &ref_dir.join(t)))
                .collect()
        };
        if let [theirs] = same.as_slice() {
            r.names.insert(ours.clone(), (*theirs).to_owned());
            r.textures += 1;
        }
    }
    let (rm, om) = (
        read_json(&ref_dir.join("map.json"))["materials"].clone(),
        read_json(&our_dir.join("map.json"))["materials"].clone(),
    );
    let (rm, om) = (rm.as_object().unwrap(), om.as_object().unwrap());
    let stem = |n: &str| n.len().checked_sub(11).map(|i| n[..i].to_owned());
    let mut theirs_by_stem: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for k in rm.keys().filter(|k| !om.contains_key(*k)) {
        if let Some(s) = stem(k) {
            theirs_by_stem.entry(s).or_default().push(k);
        }
    }
    for (k, v) in om.iter().filter(|(k, _)| !rm.contains_key(*k)) {
        let Some(s) = stem(k) else { continue };
        let mut mine = v.clone();
        r.apply(&mut mine);
        let same: Vec<&&String> = theirs_by_stem
            .get(&s)
            .into_iter()
            .flatten()
            .filter(|t| compare_json(&rm[t.as_str()], &mine, 0).equal())
            .collect();
        if let [t] = same.as_slice() {
            r.names.insert(k.clone(), (**t).clone());
            r.materials += 1;
        }
    }
    r
}

fn data_hash(b: &[u8]) -> String {
    use std::hash::{DefaultHasher, Hasher};
    let mut h = DefaultHasher::new();
    h.write(b);
    format!("{:016x}", h.finish())
}

/// One primitive's data that is compared with a tolerance: its normals (derived), its positions (exact, but for the
/// overlay lift) and its index in the file.
struct PrimData {
    normals: Vec<f32>,
    positions: Vec<f32>,
    index: usize,
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// A GLB taken apart for comparing: its JSON with the mesh's primitives as a sorted set (each its material's name
/// and its accessors, the stored ones with the data of their views; NORMAL, and POSITION with its bounds, without
/// their data) and materials, accessors, views and buffers taken out; and, in that order, each primitive's normals and
/// positions. Fails when a POSITION accessor's bounds are not those of its data.
fn glb_canonical(json: &Value, bin: &[u8]) -> Result<(Value, Vec<PrimData>), String> {
    let names: Vec<Value> = json["materials"]
        .as_array()
        .map(|m| m.iter().map(|x| x["name"].clone()).collect())
        .unwrap_or_default();
    let view = |i: &Value| -> &[u8] {
        let a = &json["accessors"][i.as_u64().unwrap() as usize];
        let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        &bin[off..off + v["byteLength"].as_u64().unwrap() as usize]
    };
    let accessor = |i: &Value, stored: bool| -> Value {
        let mut out = json["accessors"][i.as_u64().unwrap() as usize].clone();
        let o = out.as_object_mut().unwrap();
        o.remove("bufferView");
        if stored {
            o.insert("data".into(), Value::from(data_hash(view(i))));
        }
        out
    };
    let mut prims: Vec<(String, PrimData)> = Vec::new();
    for (index, p) in json["meshes"][0]["primitives"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut attrs = serde_json::Map::new();
        let (mut normals, mut positions) = (Vec::new(), Vec::new());
        for (k, i) in p["attributes"].as_object().unwrap() {
            match k.as_str() {
                "NORMAL" | "TANGENT" => {
                    attrs.insert(k.clone(), accessor(i, false));
                    normals = f32s(view(i));
                }
                "POSITION" => {
                    let mut acc = accessor(i, false);
                    let o = acc.as_object_mut().unwrap();
                    let bounds = (o.remove("min"), o.remove("max"));
                    attrs.insert(k.clone(), acc);
                    positions = f32s(view(i));
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for v in positions.as_chunks::<3>().0 {
                        for c in 0..3 {
                            lo[c] = lo[c].min(v[c]);
                            hi[c] = hi[c].max(v[c]);
                        }
                    }
                    // by value: the sign of a zero bound means nothing (the reference writes 0 for -0.0)
                    let same = |b: &Option<Value>, want: &[f32; 3]| {
                        b.as_ref().and_then(|v| v.as_array()).is_some_and(|v| {
                            v.len() == 3
                                && v.iter()
                                    .zip(want)
                                    .all(|(x, w)| x.as_f64().is_some_and(|x| x as f32 == *w))
                        })
                    };
                    if !positions.is_empty() && !(same(&bounds.0, &lo) && same(&bounds.1, &hi)) {
                        return Err(format!("primitive {index}: POSITION bounds are not its data's"));
                    }
                }
                _ => {
                    attrs.insert(k.clone(), accessor(i, true));
                }
            }
        }
        let rec = serde_json::json!({
            "material": p.get("material").and_then(|m| m.as_u64()).map_or(Value::Null, |m| names[m as usize].clone()),
            "mode": p["mode"],
            "indices": accessor(&p["indices"], true),
            "attributes": attrs,
        });
        prims.push((
            rec.to_string(),
            PrimData {
                normals,
                positions,
                index,
            },
        ));
    }
    prims.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = json.clone();
    let o = out.as_object_mut().unwrap();
    for k in ["materials", "accessors", "bufferViews", "buffers"] {
        o.remove(k);
    }
    o["meshes"][0]["primitives"] = Value::Array(
        prims
            .iter()
            .map(|p| serde_json::from_str(&p.0).unwrap())
            .collect(),
    );
    Ok((out, prims.into_iter().map(|p| p.1).collect()))
}

/// Positions compared, per vertex: an unlifted one (by the writer's lift record) must be exact; a lifted one may
/// differ per component by its lift times the normals' bound (four f32 epsilon) plus two f32 steps at the vertex's
/// largest |coordinate| (the lift distance is itself derived).
#[derive(Default)]
struct Positions {
    vertices: u64,
    exact: u64,
    lifted_within: u64,
    lifted_beyond: u64,
    unlifted_moved: u64,
    max_lifted_abs: f64,
    /// The largest lifted difference as a fraction of its bound.
    max_ratio: f64,
}

impl Positions {
    fn compare(&mut self, theirs: &[f32], ours: &[f32], lift: &[f32]) {
        for (v, (a, b)) in theirs
            .as_chunks::<3>()
            .0
            .iter()
            .zip(ours.as_chunks::<3>().0)
            .enumerate()
        {
            self.vertices += 1;
            if a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()) {
                self.exact += 1;
                continue;
            }
            let l = f64::from(lift.get(v).copied().unwrap_or(0.0));
            if l <= 0.0 {
                self.unlifted_moved += 1;
                continue;
            }
            let mut ok = true;
            // the lift distance is itself derived (positions dotted with derived normals), so beyond the normal's
            // bound the vertex may move by two f32 steps at its own coordinate scale
            let scale = a.iter().fold(0f32, |m, c| m.max(c.abs()));
            let steps = 2.0 * f64::from(f32::from_bits(scale.to_bits() + 1) - scale);
            for (&x, &y) in a.iter().zip(b) {
                let d = (f64::from(x) - f64::from(y)).abs();
                let bound = l * 4.0 * f64::from(f32::EPSILON) + steps;
                self.max_lifted_abs = self.max_lifted_abs.max(d);
                self.max_ratio = self.max_ratio.max(d / bound);
                ok &= d <= bound;
            }
            if ok {
                self.lifted_within += 1;
            } else {
                self.lifted_beyond += 1;
            }
        }
    }

    fn add(&mut self, o: &Positions) {
        self.vertices += o.vertices;
        self.exact += o.exact;
        self.lifted_within += o.lifted_within;
        self.lifted_beyond += o.lifted_beyond;
        self.unlifted_moved += o.unlifted_moved;
        self.max_lifted_abs = self.max_lifted_abs.max(o.max_lifted_abs);
        self.max_ratio = self.max_ratio.max(o.max_ratio);
    }
}

/// Derived normal components compared: exact, or within four f32 epsilon at max(1, |reference|) (the shared
/// vector maths' bound), else beyond it.
#[derive(Default)]
struct Normals {
    checked: u64,
    exact: u64,
    within: u64,
    beyond: u64,
    max_abs: f64,
}

impl Normals {
    fn compare(&mut self, theirs: &[f32], ours: &[f32]) {
        for (&a, &b) in theirs.iter().zip(ours) {
            self.checked += 1;
            let d = (f64::from(a) - f64::from(b)).abs();
            self.max_abs = self.max_abs.max(d);
            if a.to_bits() == b.to_bits() {
                self.exact += 1;
            } else if d <= 4.0 * f64::from(f32::EPSILON) * f64::from(a.abs().max(1.0)) {
                self.within += 1;
            } else {
                self.beyond += 1;
            }
        }
    }
}

/// The materials (paired names) whose first primitive comes at another place in our GLB than in the reference's.
fn moved_materials(theirs: &Value, ours: &Value) -> Vec<String> {
    let order = |j: &Value| -> Vec<String> {
        let names: Vec<&str> = j["materials"]
            .as_array()
            .map(|m| m.iter().map(|x| x["name"].as_str().unwrap_or("")).collect())
            .unwrap_or_default();
        let mut seen = Vec::new();
        for p in j["meshes"][0]["primitives"].as_array().unwrap() {
            let n = p
                .get("material")
                .and_then(|m| m.as_u64())
                .map_or("", |m| names[m as usize])
                .to_owned();
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
        seen
    };
    let (a, b) = (order(theirs), order(ours));
    a.iter()
        .enumerate()
        .filter(|(i, n)| b.get(*i) != Some(*n))
        .map(|(_, n)| n.clone())
        .collect()
}

/// Whether a material's draw order can show: a decal, alpha tested or blended (by its `map.json` record).
fn order_sensitive(materials: &Value, name: &str) -> bool {
    let m = &materials[name];
    m["domain"] == "decal" || m["domain"] == "ocean" || m["alpha"].as_str().is_some_and(|a| a != "opaque")
}

/// `map.json` with each mesh section's object by its name without the section number, its mesh and material, in a
/// fixed order, primitive indices taken out.
fn objects_canonical(map: &Value) -> Value {
    let mut out = map.clone();
    let mut objs: Vec<Value> = out["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            let mut o = o.clone();
            let m = o.as_object_mut().unwrap();
            m.remove("primitive");
            if let Some(Value::String(n)) = m.get_mut("name")
                && let Some(i) = n.rfind(" / section ")
            {
                n.truncate(i);
            }
            o
        })
        .collect();
    // Parsed objects retain producer field order. Sort keys recursively before
    // serializing the comparison key, so equivalent records stay paired.
    for o in &mut objs {
        o.sort_all_objects();
    }
    objs.sort_by_key(|o| o.to_string());
    out["objects"] = Value::Array(objs);
    out
}

#[test]
fn canonical_objects_ignore_nested_field_order() {
    let a: Value = serde_json::from_str(
        r#"{"objects":[{"name":"b / section 1","primitive":0,"meta":{"z":2,"a":1}},
        {"name":"a / section 2","primitive":1,"meta":{"z":4,"a":3}}]}"#,
    )
    .unwrap();
    let b: Value = serde_json::from_str(
        r#"{"objects":[{"meta":{"a":3,"z":4},"primitive":7,"name":"a / section 8"},
        {"meta":{"a":1,"z":2},"primitive":6,"name":"b / section 9"}]}"#,
    )
    .unwrap();
    assert_eq!(objects_canonical(&a), objects_canonical(&b));
    let mut different = b;
    different["objects"][0]["meta"]["a"] = Value::from(9);
    assert_ne!(objects_canonical(&a), objects_canonical(&different));
}

/// Objects whose primitive index does not point at a primitive of their material in their (our) GLB.
fn misplaced_sections(dir: &Path, map: &Value) -> Vec<String> {
    let mut meshes: HashMap<String, Vec<Option<String>>> = HashMap::new();
    let mut bad = Vec::new();
    for o in map["objects"].as_array().unwrap() {
        let (Some(mesh), Some(i)) = (o["mesh"].as_str(), o["primitive"].as_u64()) else {
            continue;
        };
        let mats = meshes.entry(mesh.to_owned()).or_insert_with(|| {
            let (j, _) = glb_parts(&std::fs::read(dir.join(mesh)).unwrap()).unwrap();
            let names: Vec<String> = j["materials"]
                .as_array()
                .map(|m| {
                    m.iter()
                        .map(|x| x["name"].as_str().unwrap_or("").to_owned())
                        .collect()
                })
                .unwrap_or_default();
            j["meshes"][0]["primitives"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    p.get("material")
                        .and_then(|m| m.as_u64())
                        .map(|m| names[m as usize].clone())
                })
                .collect()
        });
        if mats.get(i as usize).cloned().flatten().as_deref() != o["material"].as_str() {
            bad.push(o["name"].as_str().unwrap_or("").to_owned());
        }
    }
    bad
}

/// Remove `path` (keys) from a JSON value.
fn drop_key(v: &mut Value, path: &[&str]) {
    match path {
        [k] => {
            if let Some(m) = v.as_object_mut() {
                m.remove(*k);
            }
        }
        [k, rest @ ..] => {
            if let Some(x) = v.get_mut(*k) {
                drop_key(x, rest);
            }
        }
        [] => {}
    }
}

/// A JSON file as compared: the values not compared removed, our renamed names as the reference's.
fn prepared(rel: &str, mut v: Value, renames: Option<&Renames>) -> Value {
    if let Some(r) = renames {
        r.apply(&mut v);
    }
    match rel {
        "stats.json" => {
            drop_key(&mut v, &["totals", "seconds"]);
            drop_key(&mut v, &["totals", "texture_file_bytes"]);
            drop_key(&mut v, &["totals", "mesh_file_bytes"]);
        }
        "textures_index.json" => {
            if let Some(ts) = v.get_mut("textures").and_then(|t| t.as_array_mut()) {
                for t in ts.iter_mut() {
                    if t["file"].as_str().is_some_and(|f| f.ends_with(".png")) {
                        t.as_object_mut().unwrap().remove("bytes");
                    }
                }
                // in file order (renamed files sort elsewhere)
                ts.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
            }
        }
        _ => {}
    }
    v
}

/// How far from the reference's value ours may be where the reference rounds what it writes: to 4 decimals (lights,
/// spawn and travel positions, trainer spots, the budget's base colour factors, the grind length) and, in our own
/// piece log, to 2 decimals. `path`: `$` then `.key` and `[]` per level.
fn reference_rounding(file: &str, path: &str) -> Option<f64> {
    // not a rounding: the largest lift, derived, within 4 f32 epsilon relative (see `fold_rounded`)
    if file == "stats.json" && path == "$.overlay_stack.max_shift_m" {
        return Some(-4.0 * f64::from(f32::EPSILON));
    }
    const LIGHT: [&str; 9] = [
        "position",
        "direction",
        "basis",
        "color_linear",
        "energy",
        "cutoff_distance",
        "radius",
        "spot_size_radians",
        "spot_blend",
    ];
    let four = match file {
        "map.json" => {
            path.strip_prefix("$.lights[].")
                .is_some_and(|k| LIGHT.iter().any(|l| k.starts_with(l)))
                || [
                    "$.spawn.position[]",
                    "$.spawn_points[].position[]",
                    "$.travel_points[].position[]",
                ]
                .contains(&path)
                || (path.starts_with("$.materials.") && path.ends_with(".base_color_factor[]"))
        }
        "trainer.json" => path == "$.spots[].position[]",
        "stats.json" => path == "$.totals.grind_length_m",
        _ => false,
    };
    if four {
        return Some(5e-5);
    }
    (file == "pieces.json"
        && ["$.pieces[].gta[]", "$.pieces[].entity[]", "$.pieces[].shift_mm"].contains(&path))
    .then_some(5e-3)
}

/// Numbers taken as the reference's because it rounds them (`reference_rounding`): how many, and the largest
/// difference, per rounding.
#[derive(Default)]
struct Rounded {
    count: BTreeMap<&'static str, u64>,
    max: BTreeMap<&'static str, f64>,
}

/// Where the reference rounds a number and ours (an f32) is within the rounding of it (plus one f32 step, since
/// the reference rounds the f32's exact value), take the reference's number, counted.
fn fold_rounded(file: &str, theirs: &Value, ours: &mut Value, path: &str, r: &mut Rounded) {
    match (theirs, ours) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, x) in a {
                if let Some(y) = b.get_mut(k) {
                    fold_rounded(file, x, y, &format!("{path}.{k}"), r);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (x, y) in a.iter().zip(b.iter_mut()) {
                fold_rounded(file, x, y, &format!("{path}[]"), r);
            }
        }
        (Value::Number(x), y @ Value::Number(_)) => {
            let Some(step) = reference_rounding(file, path) else {
                return;
            };
            let (a, b) = (x.as_f64().unwrap(), y.as_f64().unwrap() as f32);
            let d = (a - f64::from(b)).abs();
            let spacing = f64::from(f32::from_bits(b.abs().to_bits() + 1) - b.abs());
            // a negative step: relative (a derived value), else absolute plus one f32 step
            let bound = if step < 0.0 {
                -step * a.abs()
            } else {
                step + spacing
            };
            // bits, not value: a rounded zero may carry the other sign
            if a.to_bits() != f64::from(b).to_bits() && d <= bound {
                let name = if step < 0.0 {
                    "decal lift along derived normal (stats.json max_shift_m)"
                } else if step < 1e-3 {
                    "reference writer rounds to 4 decimals"
                } else {
                    "reference writer rounds to 2 decimals (pieces.json, our own log)"
                };
                *r.count.entry(name).or_default() += 1;
                let m = r.max.entry(name).or_default();
                *m = m.max(d);
                *y = Value::Number(x.clone());
            }
        }
        _ => {}
    }
}

/// The reference as Studio reads it: the reference writes a negative zero as the integer `-0`, which Studio's JSON
/// reader takes as 0 (serde_json, unlike it, as -0.0), so every negative zero of the reference becomes 0, counted. We
/// write every zero unsigned.
fn unsign_reference(v: &mut Value, count: &mut u64) {
    match v {
        Value::Number(n) if n.as_f64().is_some_and(|x| x == 0.0 && x.is_sign_negative()) => {
            *v = Value::from(0.0);
            *count += 1;
        }
        Value::Array(items) => items.iter_mut().for_each(|x| unsign_reference(x, count)),
        Value::Object(m) => m.values_mut().for_each(|x| unsign_reference(x, count)),
        _ => {}
    }
}

/// The numbers the JSON comparisons saw, summed.
#[derive(Default)]
struct Numbers {
    numbers: u64,
    integer_and_float: u64,
    f32_equal: u64,
}

impl Numbers {
    fn add(&mut self, c: &JsonCompare) {
        self.numbers += c.numbers;
        self.integer_and_float += c.integer_and_float;
        self.f32_equal += c.f32_equal;
    }
}

/// The writer's two inputs agree: every M3 scene built from its prototypes' `core.mesh` (each GLB loaded into the
/// world first) writes byte-identical files to the same scene built from the GLB files (the legacy path).
#[test]
fn core_mesh_input_matches_the_file_input() {
    let (Some(reference), Some(scenes)) = (
        real_data("PRIMASE_RESKATE_REF"),
        real_data("PRIMASE_RESKATE_SCENES"),
    ) else {
        return;
    };
    let caps = real_data("PRIMASE_RESKATE_CAPS").map(|p| p.to_string_lossy().into_owned());
    let root = std::env::var_os("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-mesh-input");
    let filter = std::env::var("PRIMASE_SCENE_FILTER").unwrap_or_default();
    let mut areas: Vec<String> = std::fs::read_dir(&reference)
        .unwrap()
        .flatten()
        .filter(|e| scenes.join(e.file_name()).join("scene.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|a| a.contains(&filter))
        .collect();
    areas.sort();
    assert!(!areas.is_empty(), "no scene");
    let mut report = GateReport::new(
        "core.mesh input vs GLB file input: files byte-identical",
        Tolerance::Exact,
    );
    for area in &areas {
        let scene = scenes.join(area);
        let build = |from_mesh: bool| -> PathBuf {
            let out_root = root.join(if from_mesh { "mesh" } else { "file" });
            let o = BuildOptions {
                out_root: out_root.clone(),
                tex_cap: 256,
                tex_caps: caps.clone(),
                water_ocean: true,
                lights_max_per_cell: 256,
                piece_log: reference.join(area).join("pieces.json").is_file(),
                ..BuildOptions::default()
            };
            let out = out_root.join(area);
            if out.exists() {
                std::fs::remove_dir_all(&out).unwrap();
            }
            let mut w = World::new();
            primase_skate_scene::read(&scene, &mut w).unwrap();
            if from_mesh {
                let r = primase_gltf::load_mesh_files(&mut w, &scene).unwrap();
                println!(
                    "{area}: {} meshes, {} primitives loaded into core.mesh",
                    r.meshes, r.primitives
                );
            }
            primase_reskate::build(&w, &scene, &o).unwrap();
            out
        };
        let (a, b) = (build(false), build(true));
        let (fa, fb) = (files(&a), files(&b));
        let names_a: Vec<&String> = fa.keys().collect();
        let names_b: Vec<&String> = fb.keys().collect();
        if names_a != names_b {
            report.fail(area.clone(), "different file sets");
            continue;
        }
        for (rel, pa) in &fa {
            let (x, y) = (std::fs::read(pa).unwrap(), std::fs::read(&fb[rel]).unwrap());
            if rel == "stats.json" {
                // the build's run time is the one value two builds of the same input do not share
                let strip = |b: &[u8]| {
                    let mut v: Value = serde_json::from_slice(b).unwrap();
                    v["totals"].as_object_mut().map(|t| t.remove("seconds"));
                    serde_json::to_vec(&v).unwrap()
                };
                report.compare_bytes(
                    format!("{area}/{rel} (without totals.seconds)"),
                    &strip(&x),
                    &strip(&y),
                );
            } else {
                report.compare_bytes(format!("{area}/{rel}"), &x, &y);
            }
        }
    }
    println!("{report}");
    report.assert_passed();
}

#[test]
fn rust_writer_matches_the_reference() {
    let (Some(reference), Some(scenes)) = (
        real_data("PRIMASE_RESKATE_REF"),
        real_data("PRIMASE_RESKATE_SCENES"),
    ) else {
        return;
    };
    let caps = real_data("PRIMASE_RESKATE_CAPS").map(|p| p.to_string_lossy().into_owned());
    let out_root = primase_testdata::real_data_value("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-gate");
    let filter = primase_testdata::real_data_value("PRIMASE_SCENE_FILTER")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_default();
    let mut areas: Vec<String> = std::fs::read_dir(&reference)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path().join("map.json").is_file() && scenes.join(e.file_name()).join("scene.json").is_file()
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|a| a.contains(&filter))
        .collect();
    areas.sort();
    assert!(!areas.is_empty(), "no area with both a reference map and a scene");
    let mut sets = GateReport::new("areas: the same files (paired names)", Tolerance::Exact);
    let mut json = GateReport::new("JSON files equal as parsed values", Tolerance::Exact);
    let mut glbs = GateReport::new(
        "GLB files: JSON equal as parsed values, binary chunk byte-identical",
        Tolerance::Exact,
    );
    let mut bytes = GateReport::new("other files byte-identical", Tolerance::Exact);
    let mut pngs = GateReport::new("PNG files identical as images", Tolerance::Exact);
    let mut bcn_pngs = GateReport::new(
        "PNG files from BCn sources: identical, or within 1 per channel (D3D vs CodeWalker BCn arithmetic)",
        Tolerance::Abs(1.0),
    );
    let mut projected_normals =
        GateReport::new("projected normal PNG direction in degrees", Tolerance::Abs(0.31));
    let (mut normal_files, mut projected_pixels) = (0u64, 0u64);
    let mut bcn_within: Vec<String> = Vec::new();
    let mut pause_maps = GateReport::new(
        "pause map pictures within 2 per channel (BCn means + slope shading from derived normals)",
        Tolerance::Abs(2.0),
    );
    let mut dds = GateReport::new(
        "DDS files: header fields and payload exact, but DDSD_MIPMAPCOUNT on single-mip textures",
        Tolerance::Exact,
    );
    let mut dds_flag: Vec<String> = Vec::new();
    let mut numbers = Numbers::default();
    let mut rounded = Rounded::default();
    let mut zero_signs = 0u64;
    let mut normals = Normals::default();
    let mut positions = Positions::default();
    let mut sections = GateReport::new(
        "objects: the primitive index points at a primitive of the object's material",
        Tolerance::Exact,
    );
    // files equal but for primitive order (the reference writer orders by material name): GLBs whose moved
    // materials are all opaque, the ones with a decal, alpha-tested or blended one, and map.json files
    let (mut reordered_opaque, mut reordered_sensitive, mut reordered_maps) =
        (Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new());
    let (mut renamed_textures, mut renamed_materials) = (0usize, 0usize);
    for area in &areas {
        let ref_dir = reference.join(area);
        let scene = scenes.join(area);
        let o = BuildOptions {
            out_root: out_root.clone(),
            tex_cap: 256,
            tex_caps: caps.clone(),
            water_ocean: true,
            lights_max_per_cell: 256,
            piece_log: ref_dir.join("pieces.json").is_file(),
            ..BuildOptions::default()
        };
        let out = out_root.join(area);
        if out.exists() {
            std::fs::remove_dir_all(&out).unwrap();
        }
        let mut w = World::new();
        primase_skate_scene::read(&scene, &mut w).unwrap();
        let report = primase_reskate::build(&w, &scene, &o).unwrap();
        let renames = pair_names(&ref_dir, &out);
        renamed_textures += renames.textures;
        renamed_materials += renames.materials;
        let fa = files(&ref_dir);
        // our files under the reference's names
        let fb: BTreeMap<String, PathBuf> = files(&out)
            .into_iter()
            .map(|(rel, p)| (renames.get(&rel).to_owned(), p))
            .collect();
        let texture_index = read_json(&ref_dir.join("textures_index.json"));
        let bcn = bcn_sourced(&texture_index);
        let scene_manifest = read_json(&scene.join("scene.json"));
        let rebuild_sources: BTreeSet<_> = scene_manifest["textures"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .filter(|t| t["z"].as_str() == Some("rebuild"))
            .filter_map(|t| t["name"].as_str())
            .collect();
        let rebuilt_files: BTreeSet<_> = texture_index["textures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                t["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|s| s.as_str().is_some_and(|s| rebuild_sources.contains(s)))
            })
            .filter_map(|t| t["file"].as_str())
            .collect();
        let ref_map = read_json(&ref_dir.join("map.json"));
        let normal_textures: BTreeSet<_> = ref_map["materials"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(|m| m["normal_texture"].as_str())
            .collect();
        let our_map = read_json(&out.join("map.json"));
        let objects = our_map["objects"].as_array().map_or(0, Vec::len);
        let bad = misplaced_sections(&out, &our_map);
        for b in &bad {
            sections.fail(format!("{area}/{b}"), "primitive of another material");
        }
        for i in 0..objects - bad.len() {
            sections.record(format!("{area}/object {i}"), 0.0);
        }
        for rel in fa.keys().chain(fb.keys()).collect::<BTreeSet<_>>() {
            let item = format!("{area}/{rel}");
            let (Some(pa), Some(pb)) = (fa.get(rel), fb.get(rel)) else {
                sets.fail(
                    &item,
                    if fa.contains_key(rel) {
                        "only in the reference"
                    } else {
                        "only in ours"
                    },
                );
                continue;
            };
            sets.record(&item, 0.0);
            let (a, b) = (std::fs::read(pa).unwrap(), std::fs::read(pb).unwrap());
            if rel.ends_with(".png") {
                let from_bcn = bcn.contains(rel.as_str());
                if normal_textures.contains(rel.as_str()) && rebuilt_files.contains(rel.as_str()) {
                    normal_files += 1;
                    match normal_png_diff(&a, &b, u8::from(from_bcn)) {
                        Ok((count, angle)) => {
                            projected_pixels += count;
                            projected_normals.record(&item, angle);
                        }
                        Err(e) => projected_normals.fail(&item, e),
                    }
                    continue;
                }
                let gate = if rel == "ui/map_image.png" {
                    &mut pause_maps
                } else if from_bcn {
                    &mut bcn_pngs
                } else {
                    &mut pngs
                };
                match png_diff(&a, &b) {
                    Ok(d) => {
                        gate.record(&item, f64::from(d));
                        if from_bcn && d == 1 {
                            bcn_within.push(item.clone());
                        }
                    }
                    Err(e) => gate.fail(&item, e),
                }
            } else if rel.ends_with(".dds") {
                match dds_diff(&a, &b) {
                    Ok(flag_only) => {
                        dds.record(&item, 0.0);
                        if flag_only {
                            dds_flag.push(item.clone());
                        }
                    }
                    Err(e) => dds.fail(&item, e),
                }
            } else if rel.ends_with(".glb") {
                match (glb_parts(&a), glb_parts(&b)) {
                    (Ok((mut ja, ba)), Ok((mut jb, bb))) => {
                        renames.apply(&mut jb);
                        unsign_reference(&mut ja, &mut zero_signs);
                        let c = compare_json(&ja, &jb, 0);
                        numbers.add(&c);
                        if c.equal() && ba == bb {
                            glbs.record(&item, 0.0);
                        } else {
                            let (ga, gb) = (glb_canonical(&ja, &ba), glb_canonical(&jb, &bb));
                            let ((ca, da), (cb, db)) = match (ga, gb) {
                                (Ok(x), Ok(y)) => (x, y),
                                (Err(e), _) | (_, Err(e)) => {
                                    glbs.fail(&item, e);
                                    continue;
                                }
                            };
                            let cc = compare_json(&ca, &cb, 0);
                            let mut n = Normals::default();
                            let mut pos = Positions::default();
                            let lifts = report.lifts.get(rel.as_str());
                            for (x, y) in da.iter().zip(&db) {
                                n.compare(&x.normals, &y.normals);
                                let lift = lifts.and_then(|l| l.get(y.index)).map_or(&[][..], Vec::as_slice);
                                pos.compare(&x.positions, &y.positions, lift);
                            }
                            positions.add(&pos);
                            if !cc.equal() {
                                glbs.fail(&item, cc.differences.join("; "));
                            } else if pos.unlifted_moved > 0 || pos.lifted_beyond > 0 {
                                glbs.fail(
                                    &item,
                                    format!(
                                        "{} unlifted positions moved, {} lifted beyond the bound (max {:e}, {:.2} of \
                                         the bound)",
                                        pos.unlifted_moved, pos.lifted_beyond, pos.max_lifted_abs, pos.max_ratio
                                    ),
                                );
                            } else if n.beyond > 0 {
                                glbs.fail(
                                    &item,
                                    format!(
                                        "{} normal components beyond the bound (max {:e})",
                                        n.beyond, n.max_abs
                                    ),
                                );
                            } else {
                                glbs.record(&item, 0.0);
                                let moved = moved_materials(&ja, &jb);
                                let mats = &ref_map["materials"];
                                if moved.iter().any(|m| order_sensitive(mats, m)) {
                                    reordered_sensitive.push(format!("{item} ({})", moved.join(", ")));
                                } else if !moved.is_empty() {
                                    reordered_opaque.push(item.clone());
                                }
                            }
                            normals.checked += n.checked;
                            normals.exact += n.exact;
                            normals.within += n.within;
                            normals.beyond += n.beyond;
                            normals.max_abs = normals.max_abs.max(n.max_abs);
                        }
                    }
                    (Err(e), _) | (_, Err(e)) => glbs.fail(&item, e),
                }
            } else if rel.ends_with(".json") {
                let mut ja = prepared(rel, serde_json::from_slice(&a).unwrap(), None);
                unsign_reference(&mut ja, &mut zero_signs);
                let mut jb = prepared(rel, serde_json::from_slice(&b).unwrap(), Some(&renames));
                fold_rounded(rel, &ja, &mut jb, "$", &mut rounded);
                let c = compare_json(&ja, &jb, 0);
                numbers.add(&c);
                if c.equal() {
                    json.record(&item, 0.0);
                } else if rel == "map.json"
                    && compare_json(&objects_canonical(&ja), &objects_canonical(&jb), 0).equal()
                {
                    json.record(&item, 0.0);
                    reordered_maps.push(item.clone());
                } else {
                    let d = if rel == "map.json" {
                        // the differences that remain with sections reordered
                        compare_json(&objects_canonical(&ja), &objects_canonical(&jb), 0).differences
                    } else {
                        c.differences
                    };
                    json.fail(&item, d.join("; "));
                }
            } else {
                bytes.compare_bytes(&item, &a, &b);
            }
        }
        println!(
            "{area}: {} files in the reference, {} in ours; renamed and paired: {} textures, {} materials",
            fa.len(),
            fb.len(),
            renames.textures,
            renames.materials
        );
    }
    println!(
        "rebuilt normal PNGs {normal_files}, projected pixels {projected_pixels}; direction bound 0.31 degrees"
    );
    let gates = [
        &sets,
        &json,
        &glbs,
        &sections,
        &bytes,
        &pngs,
        &bcn_pngs,
        &pause_maps,
        &projected_normals,
        &dds,
    ];
    for g in gates {
        println!("{g}");
    }
    println!(
        "JSON numbers (files and GLB JSON): {} compared; {} equal with one side an integer and the other a float; {} \
         equal only as f32",
        numbers.numbers, numbers.integer_and_float, numbers.f32_equal
    );
    println!(
        "renamed by a content hash and paired (D3D vs CodeWalker BCn arithmetic): {renamed_textures} textures, \
         {renamed_materials} materials"
    );
    let listed = out_root.join("bcn_within_1.txt");
    std::fs::write(&listed, bcn_within.join("\n") + "\n").unwrap();
    println!(
        "PNG files within 1 (D3D vs CodeWalker BCn arithmetic): {}, listed in {}",
        bcn_within.len(),
        listed.display()
    );
    println!(
        "DDS files equal but for DDSD_MIPMAPCOUNT on single-mip, reference writer sets it: {}{}",
        dds_flag.len(),
        if dds_flag.is_empty() {
            String::new()
        } else {
            format!(" ({})", dds_flag.join(", "))
        }
    );
    println!(
        "equal but for primitive order (reference writer orders by material name, a content hash): {} GLBs with \
         only opaque materials moved; {} with a decal, alpha-tested or blended one moved; {} map.json files",
        reordered_opaque.len(),
        reordered_sensitive.len(),
        reordered_maps.len()
    );
    for r in &reordered_sensitive {
        println!("  {r}");
    }
    std::fs::write(
        out_root.join("reordered_glbs.txt"),
        reordered_opaque
            .iter()
            .chain(&reordered_sensitive)
            .map(|x| format!("{x}\n"))
            .collect::<String>(),
    )
    .unwrap();
    println!(
        "derived normals: shared maths vs reference SharpDX (components of GLBs not byte-identical): {} checked, {} \
         exact, {} within 4 f32 epsilon, {} beyond; largest difference {:e}",
        normals.checked, normals.exact, normals.within, normals.beyond, normals.max_abs
    );
    println!(
        "decal lift along derived normal (positions of GLBs not byte-identical, by the writer's lift record): {} \
         vertices, {} exact, {} lifted within lift x 4 f32 epsilon + 2 f32 steps at the vertex's scale, {} lifted beyond, {} unlifted \
         moved; largest lifted difference {:e} ({:.2} of its bound)",
        positions.vertices,
        positions.exact,
        positions.lifted_within,
        positions.lifted_beyond,
        positions.unlifted_moved,
        positions.max_lifted_abs,
        positions.max_ratio
    );
    println!(
        "reference writes negative zero as -0, which Studio reads as 0 (we write 0): {zero_signs} reference numbers read so (JSON files and GLB JSON)"
    );
    for (name, n) in &rounded.count {
        println!(
            "{name}: {n} numbers within it, largest difference {:e}",
            rounded.max[name]
        );
    }
    println!(
        "not compared: stats.json totals.seconds (run time); our own values: textures_index.json bytes of .png \
         entries, stats.json totals.texture_file_bytes, stats.json totals.mesh_file_bytes"
    );
    for g in gates {
        assert!(g.failures.is_empty(), "{g}");
    }
}

/// Real normal-map writer differential, with exact unprojected pixels and a
/// counted 0.31-degree direction bound only on outside-disk reconstruction.
#[test]
fn real_normal_writer_images() {
    let (Some(reference), Some(scenes), Some(output)) = (
        real_data("PRIMASE_RESKATE_NORMAL_REF"),
        real_data("PRIMASE_RESKATE_NORMAL_SCENES"),
        primase_testdata::real_data_value("PRIMASE_OUT").map(PathBuf::from),
    ) else {
        return;
    };
    let mut areas: Vec<_> = std::fs::read_dir(&reference)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("map.json").is_file())
        .collect();
    areas.sort_by_key(|e| e.file_name());
    assert!(!areas.is_empty());
    for area in areas {
        let reference = area.path();
        let scene = scenes.join(area.file_name());
        let manifest = read_json(&scene.join("scene.json"));
        let rebuild = manifest["textures"]
            .as_object()
            .unwrap()
            .values()
            .any(|t| t["z"].as_str() == Some("rebuild"));
        let mut world = World::new();
        primase_skate_scene::read(&scene, &mut world).unwrap();
        let options = BuildOptions {
            out_root: output.clone(),
            tex_cap: 256,
            water_ocean: true,
            lights_max_per_cell: 256,
            ..BuildOptions::default()
        };
        primase_reskate::build(&world, &scene, &options).unwrap();
        let ours = output.join(manifest["name"].as_str().unwrap());
        let renames = pair_names(&reference, &ours);
        let map = read_json(&reference.join("map.json"));
        let normal_files: BTreeSet<_> = map["materials"]
            .as_object()
            .unwrap()
            .values()
            .filter_map(|m| m["normal_texture"].as_str())
            .collect();
        assert!(!normal_files.is_empty(), "real normal coverage required");
        let paired: BTreeMap<_, _> = files(&ours)
            .into_iter()
            .map(|(name, path)| (renames.get(&name).to_owned(), path))
            .collect();
        let (mut pixels, mut projected, mut changed, mut maximum) = (0u64, 0u64, 0u64, 0f64);
        for file in &normal_files {
            let a = std::fs::read(reference.join(file)).unwrap();
            let b = std::fs::read(&paired[*file]).unwrap();
            if rebuild {
                let (n, angle) = normal_png_diff(&a, &b, 0).unwrap_or_else(|e| panic!("{file}: {e}"));
                projected += n;
                maximum = maximum.max(angle);
            } else {
                assert_eq!(png_diff(&a, &b).unwrap(), 0, "{file}");
            }
            let a = primase_image::read_png(&a).unwrap().image;
            let b = primase_image::read_png(&b).unwrap().image;
            let (primase_image::Samples::U8(a), primase_image::Samples::U8(b)) = (a.samples, b.samples)
            else {
                panic!("RGBA8 required")
            };
            pixels += a.len() as u64 / 4;
            changed += a
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .filter(|(a, b)| a != b)
                .count() as u64;
        }
        println!(
            "{}: {} normal PNGs, {pixels} pixels, {projected} outside-disk pixels, {changed} changed, maximum direction {maximum} degrees; unprojected pixels exact",
            area.file_name().to_string_lossy(),
            normal_files.len()
        );
    }
}
