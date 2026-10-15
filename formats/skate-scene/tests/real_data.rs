//! M0 gate over real scene folders: every folder under `PRIMASE_SCENES_GTA` and `PRIMASE_SCENES_SEKIRO` that holds a
//! `scene.json` is read and written back.
//!
//! Oracle: the reference validator (`PRIMASE_SCENE_SKATE`, the C# `scene-skate validate`, the writer's own reader).
//! - Agreement: the reference accepts a folder exactly when our reader does; when both refuse, the causes are
//!   compared as text (a different cause is reported, not failed).
//! - For accepted folders: every binary table written back byte-identical; the manifest equal as parsed JSON
//!   (numbers by value, floats by f32 bit pattern where the text differs, counted); every referenced file copied
//!   byte-identical; the reference validator accepts the output with the same summary and the same warnings.
//!
//! Outputs go to `PRIMASE_OUT` (default: the OS temp folder) and are deleted when they pass.
//! `PRIMASE_SCENE_FILTER` keeps only folders whose path contains it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use primase_core::World;
use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_skate_scene::json::{self, J};
use primase_skate_scene::{self as scene};

fn scene_folders(root: &Path, out: &mut Vec<PathBuf>) {
    if root.join("scene.json").is_file() {
        out.push(root.to_owned());
    }
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        scene_folders(&d, out);
    }
}

/// The reference validator's verdict: Ok(summary, warnings) or Err(message).
fn validate(validator: &Path, dir: &Path) -> Result<(String, Vec<String>), String> {
    let mut cmd = if validator.extension().is_some_and(|e| e == "dll") {
        let mut c = Command::new("dotnet");
        c.arg(validator);
        c
    } else {
        Command::new(validator)
    };
    let out = cmd.arg("validate").arg(dir).output().expect("run the validator");
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let warnings: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("warning: "))
        .filter(|l| !l.contains("has no NORMAL"))
        .map(str::to_owned)
        .collect();
    if out.status.success() {
        let summary = text
            .lines()
            .find_map(|l| l.split_once(": valid scene ").map(|(_, s)| s.to_owned()))
            .unwrap_or_default();
        Ok((summary, warnings))
    } else {
        Err(text
            .lines()
            .find_map(|l| {
                l.strip_prefix("SCENE ERROR: ")
                    .or_else(|| l.strip_prefix("ERROR: "))
            })
            .unwrap_or(text.trim())
            .to_owned())
    }
}

/// Every file the manifest references, split into tables (written by us) and copied files.
fn referenced(m: &J) -> (Vec<String>, Vec<String>) {
    fn refs(v: &J, out: &mut Vec<String>) {
        match v {
            J::Str(s) => out.push(s.clone()),
            J::Arr(items) => items.iter().for_each(|x| refs(x, out)),
            J::Obj(_) => {
                if let Some(f) = v.get("file") {
                    refs(f, out)
                }
            }
            _ => {}
        }
    }
    let mut tables = Vec::new();
    let mut files = Vec::new();
    for k in ["placements", "lights", "grind_curves", "collision"] {
        if let Some(v) = m.get(k) {
            refs(v, &mut tables);
        }
    }
    if let Some(J::Arr(ps)) = m.get("prototypes") {
        for p in ps {
            if let Some(c) = p.get("collision") {
                refs(c, &mut tables);
            }
            if let Some(g) = p.get("glb") {
                refs(g, &mut files);
            }
        }
    }
    if let Some(J::Obj(ts)) = m.get("textures") {
        for (_, t) in ts {
            refs(t, &mut files);
        }
    }
    if let Some(img) = m.get("pause_map").and_then(|p| p.get("image")) {
        refs(img, &mut files);
    }
    tables.sort();
    tables.dedup();
    files.sort();
    files.dedup();
    (tables, files)
}

fn same_file(a: &Path, b: &Path) -> Result<bool, String> {
    use std::io::Read;
    let (ma, mb) = (
        std::fs::metadata(a).map_err(|e| e.to_string())?,
        std::fs::metadata(b).map_err(|e| e.to_string())?,
    );
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (
        std::fs::File::open(a).map_err(|e| e.to_string())?,
        std::fs::File::open(b).map_err(|e| e.to_string())?,
    );
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = fa.read(&mut ba).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n]).map_err(|e| e.to_string())?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// A refusal message with the scene folder's own path taken out (the reference names files by full path).
fn norm_cause(msg: &str, dir: &Path) -> String {
    let full = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
    let mut m = msg.replace('\\', "/");
    for prefix in [
        full.to_string_lossy().replace('\\', "/"),
        dir.to_string_lossy().replace('\\', "/"),
    ] {
        m = m.replace(&format!("{prefix}/"), "");
    }
    m
}

fn manifest(dir: &Path) -> J {
    json::parse(&std::fs::read(dir.join("scene.json")).expect("scene.json")).expect("parse")
}

#[test]
fn scene_folders_round_trip() {
    let roots: Vec<PathBuf> = ["PRIMASE_SCENES_GTA", "PRIMASE_SCENES_SEKIRO"]
        .iter()
        .filter_map(|v| real_data(v))
        .collect();
    if roots.is_empty() {
        return;
    }
    let validator = real_data("PRIMASE_SCENE_SKATE");
    let filter = std::env::var("PRIMASE_SCENE_FILTER").unwrap_or_default();
    let out_root = std::env::var_os("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("skate-scene-gate");
    let mut folders = Vec::new();
    for r in &roots {
        scene_folders(r, &mut folders);
    }
    folders.retain(|f| f.to_string_lossy().contains(&filter));

    let mut agree = GateReport::new("reader agrees with the reference validator", Tolerance::Exact);
    let mut tables = GateReport::new("tables byte-identical", Tolerance::Exact);
    let mut manifests = GateReport::new("manifest equal as JSON", Tolerance::Exact);
    let mut copies = GateReport::new("referenced files byte-identical", Tolerance::Exact);
    let mut revalid = GateReport::new(
        "output accepted by the reference validator, same summary and warnings",
        Tolerance::Exact,
    );
    let mut causes: Vec<String> = Vec::new();
    let mut tolerated: BTreeMap<String, u64> = BTreeMap::new();
    let (mut f64_equal, mut f32_equal) = (0u64, 0u64);

    for (n, dir) in folders.iter().enumerate() {
        let item = dir.display().to_string();
        let reference = validator.as_ref().map(|v| validate(v, dir));
        let mut w = World::new();
        let ours = scene::read(dir, &mut w);
        match (&ours, &reference) {
            (Ok(_), Some(Err(msg))) => {
                agree.fail(&item, format!("we accept, the reference refuses: {msg}"));
                continue;
            }
            (Err(e), Some(Ok(_))) => {
                agree.fail(&item, format!("we refuse, the reference accepts: {e}"));
                continue;
            }
            (Err(e), Some(Err(msg))) => {
                agree.record(&item, 0.0);
                if norm_cause(&e.to_string(), dir) != norm_cause(msg, dir) {
                    causes.push(format!("{item}\n    ours: {e}\n    ref:  {msg}"));
                }
                continue;
            }
            (Err(e), None) => {
                causes.push(format!("{item}: refused (no reference to compare): {e}"));
                continue;
            }
            (Ok(_), _) => agree.record(&item, 0.0),
        }
        let rep = ours.expect("accepted");
        for (k, c) in &rep.tolerated {
            *tolerated.entry(k.clone()).or_default() += c;
        }
        let out = out_root.join(format!("{n:03}"));
        if out.exists() {
            std::fs::remove_dir_all(&out).expect("clear the output");
        }
        if let Err(e) = scene::write(&w, &out) {
            manifests.fail(&item, format!("write failed: {e}"));
            continue;
        }
        let mut ok = true;
        let (min, mout) = (manifest(dir), manifest(&out));
        let c = json::compare(&min, &mout);
        f64_equal += c.f64_equal;
        f32_equal += c.f32_equal;
        if c.differences.is_empty() {
            manifests.record(&item, 0.0);
        } else {
            ok = false;
            manifests.fail(&item, c.differences.join("; "));
        }
        let (tin, fin) = referenced(&min);
        for t in &tin {
            match same_file(&dir.join(t), &out.join(t)) {
                Ok(true) => tables.record(format!("{item}/{t}"), 0.0),
                Ok(false) => {
                    ok = false;
                    tables.fail(format!("{item}/{t}"), "bytes differ");
                }
                Err(e) => {
                    ok = false;
                    tables.fail(format!("{item}/{t}"), e);
                }
            }
        }
        let mut files_ok = true;
        for f in &fin {
            match same_file(&dir.join(f), &out.join(f)) {
                Ok(true) => {}
                Ok(false) => {
                    files_ok = false;
                    copies.fail(format!("{item}/{f}"), "bytes differ");
                }
                Err(e) => {
                    files_ok = false;
                    copies.fail(format!("{item}/{f}"), e);
                }
            }
        }
        if files_ok {
            copies.record(format!("{item} ({} files)", fin.len()), 0.0);
        }
        ok &= files_ok;
        if let (Some(v), Some(Ok((summary, warnings)))) = (&validator, &reference) {
            match validate(v, &out) {
                Ok((s2, w2)) if s2 == *summary && w2 == *warnings => revalid.record(&item, 0.0),
                Ok((s2, w2)) => {
                    ok = false;
                    revalid.fail(
                        &item,
                        format!(
                            "summary {s2:?} vs {summary:?}; warnings {} vs {}",
                            w2.len(),
                            warnings.len()
                        ),
                    );
                }
                Err(e) => {
                    ok = false;
                    revalid.fail(&item, e);
                }
            }
            // our reader's warnings against the reference's (GLB warnings excepted: we do not read GLBs)
            let mut a = rep.warnings.clone();
            let mut b = warnings.clone();
            a.sort();
            b.sort();
            if a != b {
                causes.push(format!(
                    "{item}: warnings differ: ours {} vs ref {}",
                    a.len(),
                    b.len()
                ));
            }
        }
        if ok {
            std::fs::remove_dir_all(&out).expect("remove a passing output");
        }
    }
    println!("{} scene folders", folders.len());
    for g in [&agree, &tables, &manifests, &copies, &revalid] {
        println!("{g}");
    }
    println!("manifest numbers equal by value (text differs): {f64_equal}; equal only as f32: {f32_equal}");
    println!("tolerated: {tolerated:?}");
    println!(
        "{} other notes (different refusal causes, warning mismatches):",
        causes.len()
    );
    for c in &causes {
        println!("  {c}");
    }
    for g in [&agree, &tables, &manifests, &copies] {
        assert!(g.failures.is_empty(), "{g}");
    }
    if validator.is_some() {
        assert!(revalid.failures.is_empty(), "{revalid}");
    }
}
