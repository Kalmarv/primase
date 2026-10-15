//! M3 gate: the Rust map writer against the reference C# writer's output, file by file.
//!
//! For every area folder in `PRIMASE_RESKATE_REF` (the reference writer's map folders) with a scene folder of the
//! same name in `PRIMASE_RESKATE_SCENES`, the scene is built with the gate's options (`--tex-cap 256 --tex-caps
//! PRIMASE_RESKATE_CAPS --water-ocean --lights-max-per-cell 256`, `--piece-log` where the reference has a
//! pieces.json) into `PRIMASE_OUT`, and every file is compared:
//! - PNG files by IHDR (size, bit depth, colour type, interlace), PLTE and tRNS, the set of other chunk kinds
//!   (besides IDAT and IEND: none may change the meaning), and the decoded pixels, exactly;
//! - every other file by bytes, except: `stats.json`'s `totals.seconds` (run time), and the texture sizes that
//!   depend on the PNG compressor (`textures_index.json` `bytes` of `.png` entries and `stats.json`
//!   `totals.texture_file_bytes`), counted apart.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_reskate::BuildOptions;

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

/// The chunks of a PNG: (kind, data), in order.
fn chunks(b: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out = Vec::new();
    let mut o = 8;
    while o + 8 <= b.len() {
        let len = u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = b[o + 4..o + 8].try_into().unwrap();
        out.push((kind, b[o + 8..o + 8 + len].to_vec()));
        o += 12 + len;
    }
    out
}

/// Equal as PNGs (IHDR, PLTE, tRNS, other chunk kinds, decoded pixels); else why not.
fn png_equal(a: &[u8], b: &[u8]) -> Result<(), String> {
    let (ca, cb) = (chunks(a), chunks(b));
    let get = |c: &[([u8; 4], Vec<u8>)], k: &[u8; 4]| c.iter().find(|x| &x.0 == k).map(|x| x.1.clone());
    for k in [b"IHDR", b"PLTE", b"tRNS"] {
        if get(&ca, k) != get(&cb, k) {
            return Err(format!("{} differs", String::from_utf8_lossy(k)));
        }
    }
    let kinds = |c: &[([u8; 4], Vec<u8>)]| -> BTreeSet<[u8; 4]> {
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
    if pa.image != pb.image {
        return Err("decoded pixels differ".into());
    }
    Ok(())
}

/// The text with `"key": value` lines' values replaced (indented JSON, one property per line).
fn blank(text: &str, keys: &[&str]) -> String {
    text.split("\r\n")
        .map(|l| {
            for k in keys {
                let p = format!("\"{k}\": ");
                if let Some(i) = l.find(&p) {
                    let rest = &l[i + p.len()..];
                    let comma = if rest.ends_with(',') { "," } else { "" };
                    return format!("{}{p}_{comma}", &l[..i]);
                }
            }
            l.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// textures_index.json with the `bytes` of `.png` entries blanked (compact JSON, one object per texture).
fn blank_png_bytes(text: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(text).unwrap();
    if let Some(ts) = v.get_mut("textures").and_then(|t| t.as_array_mut()) {
        for t in ts {
            if t.get("file")
                .and_then(|f| f.as_str())
                .is_some_and(|f| f.ends_with(".png"))
            {
                t["bytes"] = serde_json::Value::Null;
            }
        }
    }
    v.to_string()
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
    let out_root = std::env::var_os("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-gate");
    let filter = std::env::var("PRIMASE_SCENE_FILTER").unwrap_or_default();
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
    let mut bytes = GateReport::new("files byte-identical", Tolerance::Exact);
    let mut pngs = GateReport::new("PNG files identical as images", Tolerance::Exact);
    let mut excepted: BTreeMap<&str, u64> = BTreeMap::new();
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
        primase_reskate::build(&w, &scene, &o).unwrap();
        let (fa, fb) = (files(&ref_dir), files(&out));
        for rel in fa.keys().chain(fb.keys()).collect::<BTreeSet<_>>() {
            let item = format!("{area}/{rel}");
            let (Some(pa), Some(pb)) = (fa.get(rel), fb.get(rel)) else {
                bytes.fail(
                    &item,
                    if fa.contains_key(rel) {
                        "only in the reference"
                    } else {
                        "only in ours"
                    },
                );
                continue;
            };
            let (a, b) = (std::fs::read(pa).unwrap(), std::fs::read(pb).unwrap());
            if rel.ends_with(".png") {
                match png_equal(&a, &b) {
                    Ok(()) => pngs.record(&item, 0.0),
                    Err(e) => pngs.fail(&item, e),
                }
                continue;
            }
            let (ta, tb) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
            let (a2, b2) = match rel.as_str() {
                "stats.json" => {
                    if blank(&ta, &["seconds"]) != blank(&tb, &["seconds"]) {
                        *excepted
                            .entry("stats.json totals.texture_file_bytes")
                            .or_default() += 1;
                    }
                    (
                        blank(&ta, &["seconds", "texture_file_bytes"]),
                        blank(&tb, &["seconds", "texture_file_bytes"]),
                    )
                }
                "textures_index.json" => {
                    if ta != tb {
                        *excepted
                            .entry("textures_index.json bytes of .png entries")
                            .or_default() += 1;
                    }
                    (blank_png_bytes(&ta), blank_png_bytes(&tb))
                }
                _ => (ta.into_owned(), tb.into_owned()),
            };
            if a2 == b2 {
                bytes.record(&item, 0.0);
            } else {
                let i = a2
                    .bytes()
                    .zip(b2.bytes())
                    .position(|(x, y)| x != y)
                    .unwrap_or(a2.len().min(b2.len()));
                bytes.fail(
                    &item,
                    format!("first difference at byte {i} (sizes {} vs {})", a.len(), b.len()),
                );
            }
        }
        println!(
            "{area}: {} files in the reference, {} in ours",
            fa.len(),
            fb.len()
        );
    }
    println!("{bytes}\n{pngs}");
    println!("exceptions applied (files where they made a difference): {excepted:?}");
    bytes.assert_passed();
    pngs.assert_passed();
}
