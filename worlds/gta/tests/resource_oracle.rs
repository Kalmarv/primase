//! Drawables and texture dictionaries against an independent reader's decoding of the same files.
//!
//! Oracle (`PRIMASE_GTA_RESOURCES`): a folder of JSON files `{path, kind: "drawable" | "ytd", value}`:
//! - drawable: bounds, LOD distances, render masks, the embedded texture count, shaders (`name`, and per texture or
//!   constant-buffer parameter in stored order: `g9_name`, `texture`, `values` padded to four floats), and per LOD
//!   the models (`render_mask`, `skeleton_binding`, `shader_mapping`) with their geometries (`shader`, `vertices`,
//!   `stride`, `indices`, `aabb`, SHA-256 of the indices and, per legacy vertex component, of that component's bytes
//!   across all vertices);
//! - ytd: per texture name, size, depth, levels, buffer format code, data length and SHA-256.
//!
//! That reader renames Enhanced parameter names to legacy ones and reorders vertex components into the legacy
//! layout; the comparison uses the stored names it also reports and compares components by semantic, so neither
//! transformation hides a difference. Floats compare bit for bit.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::fs::File;

use primase_gta::drawable::{Drawable, Geometry, Model, ParamValue};
use primase_gta::texture::{Texture, TextureDictionary};
use primase_gta::{Install, Keys, Resource};
use primase_testdata::real_data;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_f64().expect("number") as f32)
        .collect()
}

/// Legacy vertex component index -> Enhanced semantic slot (POSITION0, BLENDWEIGHT0, BLENDINDICES0, NORMAL0,
/// COLOR0, COLOR1, TEXCOORD0-5, TANGENT0).
fn slot_of_component(c: u8) -> Option<u8> {
    Some(match c {
        0 => 0,
        1 => 16,
        2 => 20,
        3 => 4,
        4 => 24,
        5 => 25,
        6..=11 => 28 + (c - 6),
        14 => 8,
        _ => return None,
    })
}

fn check_geometry(where_: &str, g: &Geometry, o: &Value, diffs: &mut Vec<String>) {
    let mut diff = |what: &str, ours: String, theirs: String| {
        if ours != theirs {
            diffs.push(format!("{where_}: {what}: ours {ours}, oracle {theirs}"));
        }
    };
    diff("shader", g.shader.to_string(), o["shader"].to_string());
    diff("vertices", g.vertex_count.to_string(), o["vertices"].to_string());
    diff("stride", g.stride.to_string(), o["stride"].to_string());
    diff("indices", g.indices.len().to_string(), o["indices"].to_string());
    let idx: Vec<u8> = g.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    diff(
        "index digest",
        sha(&idx),
        o["index_sha"].as_str().unwrap_or("").to_string(),
    );
    let aabb: Vec<u32> = g.bounds.iter().flatten().map(|f| f.to_bits()).collect();
    let want: Vec<u32> = f32s(&o["aabb"]).iter().map(|f| f.to_bits()).collect();
    diff("bounds", format!("{aabb:?}"), format!("{want:?}"));
    let comps = o["components"].as_object().expect("components");
    diff(
        "element count",
        g.elements.len().to_string(),
        comps.len().to_string(),
    );
    for (c, digest) in comps {
        let slot = slot_of_component(c.parse().expect("component index")).expect("known component");
        match g.elements.iter().find(|e| e.slot == slot) {
            Some(&e) => diff(
                e.semantic(),
                sha(&g.element_bytes(e)),
                digest.as_str().unwrap_or("").to_string(),
            ),
            None => diff(
                "element",
                format!("none in slot {slot}"),
                format!("component {c}"),
            ),
        }
    }
}

fn check_models(where_: &str, ms: &[Model], o: &Value, diffs: &mut Vec<String>) {
    let o = o.as_array().expect("models");
    if ms.len() != o.len() {
        diffs.push(format!("{where_}: {} models, oracle {}", ms.len(), o.len()));
        return;
    }
    for (i, (m, om)) in ms.iter().zip(o).enumerate() {
        let w = format!("{where_} model {i}");
        if om["render_mask"] != m.render_mask {
            diffs.push(format!(
                "{w}: render mask {}, oracle {}",
                m.render_mask, om["render_mask"]
            ));
        }
        if om["skeleton_binding"] != m.skeleton_binding {
            diffs.push(format!(
                "{w}: skeleton binding {}, oracle {}",
                m.skeleton_binding, om["skeleton_binding"]
            ));
        }
        let og = om["geometries"].as_array().expect("geometries");
        if m.geometries.len() != og.len() {
            diffs.push(format!(
                "{w}: {} geometries, oracle {}",
                m.geometries.len(),
                og.len()
            ));
            continue;
        }
        for (j, (g, o)) in m.geometries.iter().zip(og).enumerate() {
            check_geometry(&format!("{w} geometry {j}"), g, o, diffs);
        }
    }
}

fn check_drawable(d: &Drawable, o: &Value, diffs: &mut Vec<String>) {
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    for (what, ours, theirs) in [
        (
            "bounding centre",
            bits(&d.bounding_centre),
            bits(&f32s(&o["bounding_center"])),
        ),
        (
            "bounding radius",
            bits(&[d.bounding_radius]),
            bits(&[o["bounding_radius"].as_f64().expect("radius") as f32]),
        ),
        ("box min", bits(&d.bbox_min), bits(&f32s(&o["bbox_min"]))),
        ("box max", bits(&d.bbox_max), bits(&f32s(&o["bbox_max"]))),
        ("LOD distances", bits(&d.lod_dist), bits(&f32s(&o["lod_dist"]))),
    ] {
        if ours != theirs {
            diffs.push(format!("{what}: ours {ours:?}, oracle {theirs:?}"));
        }
    }
    let masks: Vec<u64> = o["render_mask"]
        .as_array()
        .expect("masks")
        .iter()
        .map(|v| v.as_u64().expect("mask"))
        .collect();
    if d.render_masks.iter().map(|&m| u64::from(m)).collect::<Vec<_>>() != masks {
        diffs.push(format!(
            "render masks: ours {:?}, oracle {masks:?}",
            d.render_masks
        ));
    }
    let embedded = d.textures.as_ref().map_or(0, |t| t.textures.len());
    if embedded as u64 != o["embedded_textures"].as_u64().expect("count") {
        diffs.push(format!(
            "embedded textures: ours {embedded}, oracle {}",
            o["embedded_textures"]
        ));
    }
    let os = o["shaders"].as_array().expect("shaders");
    if d.shaders.len() != os.len() {
        diffs.push(format!("{} shaders, oracle {}", d.shaders.len(), os.len()));
    }
    for (i, (s, o)) in d.shaders.iter().zip(os).enumerate() {
        if u64::from(s.name) != o["name"].as_u64().expect("name") {
            diffs.push(format!("shader {i}: name {:#x}, oracle {}", s.name, o["name"]));
        }
        // that reader lists texture and constant-buffer parameters only
        let ours: Vec<_> = s
            .params
            .iter()
            .filter(|p| matches!(p.value, ParamValue::Texture(_) | ParamValue::Floats(_)))
            .collect();
        let op = o["params"].as_array().expect("params");
        if ours.len() != op.len() {
            diffs.push(format!(
                "shader {i}: {} parameters, oracle {}",
                ours.len(),
                op.len()
            ));
            continue;
        }
        for (p, o) in ours.iter().zip(op) {
            let w = format!("shader {i} parameter {:#010x}", p.name);
            if u64::from(p.name) != o["g9_name"].as_u64().expect("g9 name") {
                diffs.push(format!("{w}: oracle name {}", o["g9_name"]));
            }
            match &p.value {
                ParamValue::Texture(t) => {
                    if t.as_deref() != o["texture"].as_str() {
                        diffs.push(format!("{w}: texture {t:?}, oracle {}", o["texture"]));
                    }
                }
                ParamValue::Floats(v) => {
                    // padded to whole vectors of four, as that reader keeps them
                    let mut padded = v.clone();
                    padded.resize(v.len().div_ceil(4).max(1) * 4, 0.0);
                    let theirs = if o["values"].is_null() {
                        Vec::new()
                    } else {
                        f32s(&o["values"])
                    };
                    if bits(&padded) != bits(&theirs) {
                        diffs.push(format!("{w}: values {padded:?}, oracle {theirs:?}"));
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    for (lod, name) in ["high", "med", "low", "vlow"].iter().enumerate() {
        check_models(name, &d.lods[lod], &o[*name], diffs);
    }
}

fn check_texture(i: usize, t: &Texture, o: &Value, diffs: &mut Vec<String>) {
    let ours = format!(
        "{} {}x{}x{} levels {} format {} data {} {}",
        t.name,
        t.width,
        t.height,
        t.depth,
        t.levels,
        t.format.0,
        t.data.len(),
        sha(&t.data)
    );
    let theirs = format!(
        "{} {}x{}x{} levels {} format {} data {} {}",
        o["name"].as_str().unwrap_or(""),
        o["width"],
        o["height"],
        o["depth"],
        o["levels"],
        o["g9_format_code"],
        o["data_len"],
        o["data_sha"].as_str().unwrap_or("")
    );
    if ours != theirs {
        diffs.push(format!("texture {i}: ours {ours}, oracle {theirs}"));
    }
}

fn check_ytd(td: &TextureDictionary, o: &Value, diffs: &mut Vec<String>) {
    let ot = o["textures"].as_array().expect("textures");
    if td.textures.len() != ot.len() {
        diffs.push(format!("{} textures, oracle {}", td.textures.len(), ot.len()));
    }
    for (i, (t, o)) in td.textures.iter().zip(ot).enumerate() {
        match t {
            Some(t) => check_texture(i, t, o, diffs),
            None => diffs.push(format!("texture {i}: a null entry, oracle {}", o["name"])),
        }
    }
}

#[test]
fn drawables_and_textures_against_the_oracle() {
    let (Some(dir), Some(key_dir), Some(oracle)) = (
        real_data("PRIMASE_GTA_DIR"),
        real_data("PRIMASE_GTA_KEYS"),
        real_data("PRIMASE_GTA_RESOURCES"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let mut files: Vec<_> = std::fs::read_dir(&oracle)
        .expect("oracle folder")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut failed = 0;
    for f in &files {
        let o: Value =
            serde_json::from_str(&std::fs::read_to_string(f).expect("oracle file")).expect("oracle json");
        let path = o["path"].as_str().expect("path");
        let r = install.file(path).expect("file in the install");
        let a = &install.archives[r.archive];
        let res: Resource = a
            .resource(&mut File::open(&a.file).expect("open archive"), r.entry, &keys)
            .expect("resource");
        let mut diffs = Vec::new();
        match o["kind"].as_str() {
            Some("drawable") => check_drawable(
                &Drawable::read_resource(&res).expect("drawable"),
                &o["value"],
                &mut diffs,
            ),
            Some("ytd") => check_ytd(
                &TextureDictionary::read_resource(&res).expect("ytd"),
                &o["value"],
                &mut diffs,
            ),
            k => panic!("kind {k:?}"),
        }
        println!("{path}: {} differences", diffs.len());
        for d in diffs.iter().take(20) {
            println!("    {d}");
        }
        failed += usize::from(!diffs.is_empty());
    }
    println!("{} files, {failed} with differences", files.len());
    assert_eq!(failed, 0);
}
