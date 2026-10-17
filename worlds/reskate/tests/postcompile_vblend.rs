//! Gate of the per-vertex blend pass (`primase_reskate::postcompile::vblend`) on a real Studio Patch.
//!
//! Inputs: `PRIMASE_RESKATE_PATCH` (a Studio-compiled Patch), `PRIMASE_SKATE_DATA` and `PRIMASE_OODLE9` (the game's
//! data and Oodle, for the stock donors and base-placed files) and `PRIMASE_RESKATE_VBLEND_SPEC`: JSON
//! `{"requests": [{"key": hex, "target": "decal" | "twoset", "second": hex}], "sections": {"<mesh>|<lod>|<section>":
//! {"color0": hex, "submaterial": hex}}}` naming the keys to retarget and every rewritten section's expected
//! per-vertex bytes. The pass is applied with those values into `PRIMASE_OUT`, then:
//! - read back with our readers: every MeshSet reads; a rewritten section has the donor's declaration (both
//!   copies), the spec's bytes, positions equal to Studio's to half precision (w = 1), texture coordinates and
//!   tangent frames byte for byte; every other section's vertices and every index buffer byte for byte; the program
//!   and texture-set rows of each key are the donor's;
//! - applied twice, the outputs are byte-identical (determinism);
//! - `PRIMASE_RESKATE_VBLEND_CHECK` (optional): an independent checker (oracle: a Python decode of the output with
//!   reskate-ports' fbpatch / sbdfmt readers, run as `python -I CHECK ORIG OUT SPEC DATA [REFERENCE]`) prints `PASS`;
//!   with `PRIMASE_RESKATE_VBLEND_REF` (a Patch from the reference patcher that was checked in game) it also
//!   compares the decoded output with that one.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{BAM, R, ROOT, all_files, index_buffer, meshes, rows, stream};
use primase_compress::oodle::{Oodle, Version};
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::meshset;
use primase_reskate::postcompile::vblend::{
    self, DECAL_DONOR, Donor, Request, SectionValues, StockPrograms, TWOSET_DONOR, Target,
};
use primase_testdata::real_data;
use serde_json::Value;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn quads(s: &str) -> Vec<[u8; 4]> {
    unhex(s)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect()
}

fn key(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap(), 16).unwrap()
}

#[test]
fn vblend_gate() -> R {
    let Some(patch) = real_data("PRIMASE_RESKATE_PATCH") else {
        return Ok(());
    };
    let Some(data) = real_data("PRIMASE_SKATE_DATA") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE9") else {
        return Ok(());
    };
    let Some(spec_path) = real_data("PRIMASE_RESKATE_VBLEND_SPEC") else {
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
    let spec: Value = serde_json::from_slice(&std::fs::read(&spec_path)?)?;
    let decal = Donor::find(&game, BAM, ROOT, DECAL_DONOR)?;
    let twoset = Donor::find(&game, BAM, ROOT, TWOSET_DONOR)?;
    let stock = StockPrograms::find(&game, ROOT)?;
    let requests: Vec<Request> = spec["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| match r["target"].as_str().unwrap() {
            "decal" => Request {
                key: key(&r["key"]),
                target: Target::Decal,
                donor: decal.clone(),
            },
            _ => Request {
                key: key(&r["key"]),
                target: Target::TwoSet {
                    second: key(&r["second"]),
                },
                donor: twoset.clone(),
            },
        })
        .collect();
    let expected: BTreeMap<String, SectionValues> = spec["sections"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                SectionValues {
                    color0: quads(v["color0"].as_str().unwrap()),
                    submaterial: quads(v["submaterial"].as_str().unwrap()),
                },
            )
        })
        .collect();
    let root = primase_testdata::real_data_value("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("reskate-vblend");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let run = |out: &Path| -> R<vblend::Report> {
        let mut seen = 0usize;
        let mut values = |s: &vblend::SectionInput| -> primase_reskate::postcompile::Result<SectionValues> {
            seen += 1;
            expected
                .get(&format!("{}|{}|{}", s.mesh, s.lod, s.section))
                .cloned()
                .ok_or_else(|| {
                    primase_reskate::postcompile::Error::Missing(format!(
                        "no spec values for {} {} {}",
                        s.mesh, s.lod, s.section
                    ))
                })
        };
        let report = vblend::apply(
            &patch,
            out,
            Some(&base),
            Some(&oodle),
            &stock,
            &requests,
            &mut values,
        )?;
        assert_eq!(seen, expected.len(), "every spec section is rewritten once");
        Ok(report)
    };
    let out = root.join("a");
    let report = run(&out)?;
    println!(
        "rewrote {} sections in {} MeshSets, max position error {} m, rows {:?}",
        report.sections, report.meshsets, report.max_position_error, report.rows
    );
    // ---- read back with our readers
    let fo = Folder::open(&patch)?;
    let fn_ = Folder::open(&out)?;
    let (so, sn) = (
        Source {
            folder: &fo,
            base: Some(&base),
            oodle: Some(&oodle),
        },
        Source {
            folder: &fn_,
            base: Some(&base),
            oodle: Some(&oodle),
        },
    );
    let (mo, mn) = (meshes(&so)?, meshes(&sn)?);
    assert_eq!(mo.keys().collect::<Vec<_>>(), mn.keys().collect::<Vec<_>>());
    let wanted: BTreeMap<u64, &Request> = requests.iter().map(|r| (r.key, r)).collect();
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
            for (si, (sn_, so_)) in ln.sections.iter().zip(&lo.sections).enumerate() {
                match wanted.get(&so_.key) {
                    None => {
                        assert_eq!(sn_.decl, so_.decl);
                        for j in 0..so_.decl.strides.len() {
                            assert_eq!(
                                stream(sn_, gn, j),
                                stream(so_, go, j),
                                "{name} untouched section {si} stream {j}"
                            );
                        }
                    }
                    Some(r) => {
                        assert_eq!(sn_.decl.bytes, r.donor.decl);
                        assert_eq!(sn_.draw_decl.bytes, r.donor.decl);
                        let e = &expected[&format!("{name}|{li}|{si}")];
                        assert_eq!(stream(sn_, gn, 1), e.color0.concat(), "{name} {li} {si} Color0");
                        assert_eq!(
                            stream(sn_, gn, 3),
                            e.submaterial.concat(),
                            "{name} {li} {si} sub-material"
                        );
                        assert_eq!(
                            stream(sn_, gn, 2),
                            stream(so_, go, 1),
                            "{name} {li} {si} texture coordinates"
                        );
                        assert_eq!(
                            stream(sn_, gn, 4),
                            stream(so_, go, 2),
                            "{name} {li} {si} tangent frames"
                        );
                        let (pn, po) = (stream(sn_, gn, 0), stream(so_, go, 0));
                        for (h, f) in pn.as_chunks::<8>().0.iter().zip(po.as_chunks::<12>().0.iter()) {
                            for c in 0..3 {
                                let want = half::f16::from_f32(f32::from_le_bytes(
                                    f[4 * c..4 * c + 4].try_into().unwrap(),
                                ));
                                assert_eq!(&h[2 * c..2 * c + 2], &want.to_le_bytes());
                            }
                            assert_eq!(&h[6..8], &half::f16::ONE.to_le_bytes());
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, expected.len());
    let (pn, tn) = rows(&sn)?;
    for r in &requests {
        assert_eq!(pn[&r.key], r.donor.program_row);
        assert_eq!(tn[&r.key], r.donor.texture_set);
    }
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
    // ---- independent checker
    if let Some(check) = real_data("PRIMASE_RESKATE_VBLEND_CHECK") {
        let mut cmd = Command::new("python");
        cmd.arg("-I")
            .arg(&check)
            .arg(&patch)
            .arg(&out)
            .arg(&spec_path)
            .arg(&data);
        if let Some(reference) = real_data("PRIMASE_RESKATE_VBLEND_REF") {
            cmd.arg(reference);
        }
        let o = cmd.output()?;
        let text = String::from_utf8_lossy(&o.stdout);
        println!("{text}{}", String::from_utf8_lossy(&o.stderr));
        assert!(
            o.status.success() && text.lines().last() == Some("PASS"),
            "independent checker did not pass"
        );
    }
    Ok(())
}
