//! Gate of the vertex-blend record stage (`primase_reskate::postcompile::vbrecord`) on a real compile.
//!
//! Inputs: `PRIMASE_RESKATE_BLEND_MAP` (a map folder the writer built with `vertex_blend` materials, so with
//! `vertex_blend.json`), `PRIMASE_RESKATE_BLEND_PATCH` (Studio's Patch of that map, compiled by a build that carries
//! the weights), `PRIMASE_SKATE_DATA` and `PRIMASE_OODLE9` (the game's data and Oodle). Then:
//! - every recorded primitive resolves; each one's margin (the best containment among the other keys of its GLB) and
//!   whether each GLB's sections follow its primitive order are printed;
//! - the stage is applied into `PRIMASE_OUT` and read back with our readers: every MeshSet reads; each section of a
//!   resolved key has the stock decal declaration (both copies), `Color0` = `decal_opacity` of Studio's weight,
//!   sub-material 0, texture coordinates equal to Studio's `x, y`, tangent frames byte for byte, positions equal to
//!   Studio's to half precision (w = 1); every other section's vertices and every index buffer byte for byte; each
//!   key's program and texture-set rows are the stock decal's;
//! - applied twice, the outputs are byte-identical (determinism);
//! - `PRIMASE_RESKATE_BLEND_SPEC` (optional): the same resolution and values from an independent implementation
//!   (oracle: a Python decode of the Patch and the map's GLBs), as the S2 gate's spec: its keys must equal ours, and
//!   with `PRIMASE_RESKATE_VBLEND_CHECK` the independent checker must pass on our output.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{BAM, R, ROOT, all_files, index_buffer, meshes, rows, stream};
use primase_compress::oodle::{Oodle, Version};
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::meshset;
use primase_reskate::postcompile::vblend::{DECAL_DONOR, Donor, decal_opacity};
use primase_reskate::postcompile::vbrecord::{self, Record};
use primase_testdata::real_data;
use serde_json::Value;

#[test]
fn vbrecord_gate() -> R {
    let (Some(map), Some(patch), Some(data), Some(dll)) = (
        real_data("PRIMASE_RESKATE_BLEND_MAP"),
        real_data("PRIMASE_RESKATE_BLEND_PATCH"),
        real_data("PRIMASE_SKATE_DATA"),
        real_data("PRIMASE_OODLE9"),
    ) else {
        return Ok(());
    };
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let base = Folder::open(&data)?;
    let game = Source {
        folder: &base,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let decal = Donor::find(&game, BAM, ROOT, DECAL_DONOR)?;
    let record = Record::read(&map)?.ok_or("the map has no vertex_blend.json")?;
    // ---- resolution, with its margins
    let fo = Folder::open(&patch)?;
    let so = Source {
        folder: &fo,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let sections = vbrecord::blend_sections(&so)?;
    let resolution = vbrecord::resolve(&record, &map, &sections)?;
    for r in &resolution.primitives {
        println!(
            "{} {} primitive {} -> key {:016x}: {} distinct weights, all contained; next-best key contains {:.1}%",
            r.material,
            r.mesh,
            r.primitive,
            r.key,
            r.distinct_weights,
            100.0 * r.runner_up
        );
        assert!(r.runner_up < 1.0);
    }
    let worst = resolution
        .primitives
        .iter()
        .map(|r| r.runner_up)
        .fold(0.0, f64::max);
    println!(
        "{} materials -> {} keys over {} primitives, {} vertex-blend sections; largest next-best containment {:.1}%; \
         sections in primitive order: {:?}",
        record.materials.len(),
        resolution.keys.values().collect::<BTreeSet<_>>().len(),
        resolution.primitives.len(),
        sections.len(),
        100.0 * worst,
        resolution.in_primitive_order
    );
    assert_eq!(resolution.keys.len(), record.materials.len());
    // ---- apply, read back
    let root = primase_testdata::real_data_value("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-vbrecord");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let run = |out: &Path| -> R<vbrecord::Resolution> {
        let (report, res) =
            vbrecord::apply(&patch, out, &map, Some(&base), Some(&oodle), &decal)?.ok_or("no record")?;
        println!(
            "rewrote {} sections in {} MeshSets, max position error {} m",
            report.sections, report.meshsets, report.max_position_error
        );
        assert_eq!(report.sections, sections.len());
        Ok(res)
    };
    let out = root.join("a");
    assert_eq!(run(&out)?, resolution);
    let fnew = Folder::open(&out)?;
    let sn = Source {
        folder: &fnew,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let (mo, mn) = (meshes(&so)?, meshes(&sn)?);
    assert_eq!(mo.keys().collect::<Vec<_>>(), mn.keys().collect::<Vec<_>>());
    let keys: BTreeSet<u64> = resolution.keys.values().copied().collect();
    let mut checked = 0;
    for ((bn, name), (dn, cn)) in &mn {
        let (d0, c0) = &mo[&(bn.clone(), name.clone())];
        let (msn, mso) = (meshset::read(dn)?, meshset::read(d0)?);
        for (li, (ln, lo)) in msn.lods.iter().zip(&mso.lods).enumerate() {
            let (gn, go) = (&cn[&ln.chunk], &c0[&lo.chunk]);
            assert_eq!(
                index_buffer(ln, gn),
                index_buffer(lo, go),
                "{name} LOD {li} index buffer"
            );
            for (si, (s1, s0)) in ln.sections.iter().zip(&lo.sections).enumerate() {
                let at = format!("{name} LOD {li} section {si}");
                if !keys.contains(&s0.key) {
                    assert_eq!(s1.decl, s0.decl);
                    for j in 0..s0.decl.strides.len() {
                        assert_eq!(stream(s1, gn, j), stream(s0, go, j), "{at} stream {j}");
                    }
                    continue;
                }
                assert_eq!(s1.decl.bytes, decal.decl, "{at}");
                assert_eq!(s1.draw_decl.bytes, decal.decl, "{at}");
                let uv = stream(s0, go, 1);
                let (uv, rest) = uv.as_chunks::<8>();
                assert!(rest.is_empty(), "{at}: Studio's TexCoord0 is Half4");
                let want_color: Vec<u8> = uv
                    .iter()
                    .flat_map(|c| decal_opacity(half::f16::from_le_bytes([c[4], c[5]]).to_f32()))
                    .collect();
                assert_eq!(stream(s1, gn, 1), want_color, "{at} Color0");
                assert!(stream(s1, gn, 3).iter().all(|&b| b == 0), "{at} sub-material");
                let want_uv: Vec<u8> = uv.iter().flat_map(|c| c[..4].to_vec()).collect();
                assert_eq!(stream(s1, gn, 2), want_uv, "{at} texture coordinates");
                assert_eq!(stream(s1, gn, 4), stream(s0, go, 2), "{at} tangent frames");
                let (pn, po) = (stream(s1, gn, 0), stream(s0, go, 0));
                for (h, f) in pn.as_chunks::<8>().0.iter().zip(po.as_chunks::<12>().0) {
                    for c in 0..3 {
                        let want =
                            half::f16::from_f32(f32::from_le_bytes(f[4 * c..4 * c + 4].try_into().unwrap()));
                        assert_eq!(&h[2 * c..2 * c + 2], &want.to_le_bytes(), "{at} position");
                    }
                    assert_eq!(&h[6..8], &half::f16::ONE.to_le_bytes());
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, sections.len());
    let (pn, tn) = rows(&sn)?;
    for k in &keys {
        assert_eq!(pn[k], decal.program_row);
        assert_eq!(tn[k], decal.texture_set);
    }
    println!("{checked} sections read back");
    // ---- determinism
    let out2 = root.join("b");
    run(&out2)?;
    let (mut fa, mut fb) = (Vec::new(), Vec::new());
    all_files(&out, &mut fa);
    all_files(&out2, &mut fb);
    assert_eq!(fa.len(), fb.len());
    for (a, b) in fa.iter().zip(&fb) {
        assert_eq!(a.strip_prefix(&out)?, b.strip_prefix(&out2)?);
        assert!(
            std::fs::read(a)? == std::fs::read(b)?,
            "{} differs between runs",
            a.display()
        );
    }
    // ---- independent resolution and checker
    if let Some(spec_path) = real_data("PRIMASE_RESKATE_BLEND_SPEC") {
        let spec: Value = serde_json::from_slice(&std::fs::read(&spec_path)?)?;
        let theirs: BTreeMap<String, u64> = spec["materials"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(m, k)| (m.clone(), u64::from_str_radix(k.as_str().unwrap(), 16).unwrap()))
            .collect();
        assert_eq!(theirs, resolution.keys, "independent resolution");
        println!("independent resolution agrees: {} materials", theirs.len());
        if let Some(check) = real_data("PRIMASE_RESKATE_VBLEND_CHECK") {
            let o = Command::new("python")
                .arg("-I")
                .arg(&check)
                .arg(&patch)
                .arg(&out)
                .arg(&spec_path)
                .arg(&data)
                .output()?;
            let text = String::from_utf8_lossy(&o.stdout);
            println!("{text}{}", String::from_utf8_lossy(&o.stderr));
            assert!(
                o.status.success() && text.lines().last() == Some("PASS"),
                "independent checker did not pass"
            );
        }
    }
    Ok(())
}
