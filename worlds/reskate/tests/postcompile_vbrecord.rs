//! Gate of the vertex-blend record stage (`primase_reskate::postcompile::vbrecord`) on a real compile.
//!
//! Inputs: `PRIMASE_RESKATE_BLEND_MAP` (a map folder the writer built with `vertex_blend` materials, so with
//! `vertex_blend.json`), `PRIMASE_RESKATE_BLEND_PATCH` (Studio's Patch of that map, compiled by a build that carries
//! the weights), `PRIMASE_SKATE_DATA` and `PRIMASE_OODLE9` (the game's data and Oodle). Then:
//! - every recorded primitive resolves; each one's margin (the best containment among the other keys of its GLB) and
//!   whether each GLB's sections follow its primitive order are printed;
//! - the stage is applied into `PRIMASE_OUT` and read back with our readers: every MeshSet reads; each section of a
//!   resolved key has the stock decal declaration (both copies), `Color0` = `decal_opacity` of Studio's weight,
//!   sub-material 0, texture coordinates equal to Studio's `x, y`, tangent frames byte for byte, positions at half
//!   precision (w = 1) never behind their surface: Studio's position rounded to nearest where that is not behind the
//!   plane of the vertex's triangles (normals computed here from Studio's own positions and indices), otherwise
//!   moved out along the normal's dominant axis only and no further than needed (one half step back would be
//!   behind); every other section's vertices and every index buffer byte for byte; each key's program and
//!   texture-set rows are the stock decal's; the root's program records are the game's (Studio's private
//!   `vertex_blend` program gone) and every row indexes them;
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
use primase_reskate::postcompile::vblend::{DECAL_DONOR, Donor, StockPrograms, decal_opacity};
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
    let stock = StockPrograms::find(&game, ROOT)?;
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
        let (report, res) = vbrecord::apply(&patch, out, &map, Some(&base), Some(&oodle), &decal, &stock)?
            .ok_or("no record")?;
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
    let (mut checked, mut stepped) = (0, 0);
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
                let studio: Vec<[f32; 3]> = po
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .map(|f| [0, 1, 2].map(|c| f32::from_le_bytes(f[4 * c..4 * c + 4].try_into().unwrap())))
                    .collect();
                let normals = section_normals(lo, s0, go, &studio);
                for (vi, (h, f)) in pn.as_chunks::<8>().0.iter().zip(&studio).enumerate() {
                    let got = [0, 1, 2].map(|c| half::f16::from_le_bytes([h[2 * c], h[2 * c + 1]]));
                    in_front_or_nearest(*f, normals[vi], got, &format!("{at} vertex {vi}"));
                    stepped += usize::from(got != f.map(half::f16::from_f32));
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
    // the game's program records, not Studio's copy with its private program, and every row inside them
    assert_eq!(StockPrograms::find(&sn, ROOT)?, stock);
    println!("{checked} sections read back; {stepped} vertices stepped in front of their surface");
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

/// Area-weighted vertex normals of section `s` of LOD `l` from its indices (counted from its first vertex) and
/// Studio's positions.
fn section_normals(l: &meshset::Lod, s: &meshset::Section, g: &[u8], p: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let size = usize::from(l.index_size);
    let at = l.vertex_bytes as usize + s.start_index as usize * size;
    let raw = &g[at..at + s.primitives as usize * 3 * size];
    let idx: Vec<usize> = raw
        .chunks(size)
        .map(|c| {
            if size == 2 {
                u16::from_le_bytes([c[0], c[1]]) as usize
            } else {
                u32::from_le_bytes(c.try_into().unwrap()) as usize
            }
        })
        .collect();
    let mut n = vec![[0f32; 3]; p.len()];
    for t in idx.chunks(3) {
        let (a, b, c) = (p[t[0]], p[t[1]], p[t[2]]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &k in t {
            for j in 0..3 {
                n[k][j] += f[j];
            }
        }
    }
    n
}

/// The next half float after `h` toward +infinity (`up`) or -infinity, by value.
fn next_half(h: half::f16, up: bool) -> half::f16 {
    let x = h.to_f32();
    // the smallest step at x's magnitude, from the binade of |x| (subnormal spacing below 2^-14)
    let step = |m: f32| {
        if m < 2f32.powi(-14) {
            2f32.powi(-24)
        } else {
            2f32.powi(m.log2().floor() as i32 - 10)
        }
    };
    let y = if up {
        x + step(if x < 0.0 { (x + 2f32.powi(-25)).abs() } else { x })
    } else {
        x - step(if x > 0.0 { (x - 2f32.powi(-25)).abs() } else { -x })
    };
    half::f16::from_f32(y)
}

/// The stage's position rule for a decal vertex `f` with normal `n`, checked on what it wrote (`got`).
fn in_front_or_nearest(f: [f32; 3], n: [f32; 3], got: [half::f16; 3], at: &str) {
    let nearest = f.map(half::f16::from_f32);
    let depth = |h: [half::f16; 3]| (0..3).map(|i| (h[i].to_f32() - f[i]) * n[i]).sum::<f32>();
    let axis = (0..3).fold(0, |a, i| if n[i].abs() > n[a].abs() { i } else { a });
    if n[axis] == 0.0 || depth(nearest) >= 0.0 {
        assert_eq!(got, nearest, "{at}: nearest position expected");
        return;
    }
    assert!(
        depth(got) >= 0.0,
        "{at}: {got:?} behind its surface ({f:?}, normal {n:?})"
    );
    for i in (0..3).filter(|&i| i != axis) {
        assert_eq!(got[i], nearest[i], "{at}: moved off the dominant axis");
    }
    let mut back = got;
    back[axis] = next_half(got[axis], n[axis] < 0.0);
    assert!(
        depth(back) < 0.0,
        "{at}: {got:?} is further out than needed ({f:?}, normal {n:?})"
    );
}
