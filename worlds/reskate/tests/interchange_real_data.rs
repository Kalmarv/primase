//! M0 gate over real scene folders: every folder under `PRIMASE_SCENES_GTA` and `PRIMASE_SCENES_SEKIRO` that holds a
//! `scene.json` is read and written back.
//!
//! Oracle: the reference validator (`PRIMASE_SCENE_SKATE`, the C# `scene-skate validate`, the writer's own reader).
//! - Agreement: the reference accepts a folder exactly when our reader does; when both refuse, the causes are
//!   compared as text (a different cause is reported, not failed).
//! - For accepted folders: every binary table written back byte-identical; the manifest equal as parsed JSON
//!   (`primase_core::gate::compare_json`: integers exactly, other numbers as f64 or else as f32, counted); every
//!   referenced file copied byte-identical; the reference validator accepts the output with the same summary and the
//!   same warnings.
//!
//! Counts: agreement, manifests and re-validation per folder; tables and referenced files per file. An input folder
//! whose scene.json or referenced files change (size or modification time) while it is checked is reported as
//! changed and counted in no gate; the test fails on one unless `PRIMASE_ALLOW_LIVE=1`.
//!
//! Outputs go to `PRIMASE_OUT` (default: the OS temp folder) and are deleted when they pass.
//! `PRIMASE_SCENE_FILTER` keeps only folders whose path contains it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use primase_core::World;
use primase_core::gate::{GateReport, Tolerance, compare_json};
use primase_reskate::{read, write};
use primase_testdata::real_data;
use serde_json::Value;

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
fn referenced(m: &Value) -> (Vec<String>, Vec<String>) {
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|x| refs(x, out)),
            Value::Object(_) => {
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
    if let Some(Value::Array(ps)) = m.get("prototypes") {
        for p in ps {
            if let Some(c) = p.get("collision") {
                refs(c, &mut tables);
            }
            if let Some(g) = p.get("glb") {
                refs(g, &mut files);
            }
        }
    }
    if let Some(Value::Object(ts)) = m.get("textures") {
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

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("scene.json")).expect("scene.json")).expect("parse")
}

/// Size and modification time of scene.json and every file it references (None: missing), to notice an input
/// folder that another process rewrites while the gate reads it.
type Snapshot = BTreeMap<String, Option<(u64, std::time::SystemTime)>>;

fn snapshot(dir: &Path) -> Snapshot {
    let stat = |rel: &str| {
        std::fs::metadata(dir.join(rel))
            .ok()
            .map(|m| (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)))
    };
    let mut snap = Snapshot::new();
    snap.insert("scene.json".into(), stat("scene.json"));
    let parsed = std::fs::read(dir.join("scene.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if let Some(m) = parsed {
        let (t, f) = referenced(&m);
        for rel in t.iter().chain(&f) {
            snap.insert(rel.clone(), stat(rel));
        }
    }
    snap
}

/// The gates, per folder or summed.
struct Gates {
    agree: GateReport,
    tables: GateReport,
    manifests: GateReport,
    copies: GateReport,
    revalid: GateReport,
    notes: Vec<String>,
    tolerated: BTreeMap<String, u64>,
    numbers: u64,
    integer_and_float: u64,
    f32_equal: u64,
    /// Folders that passed every check that applied to them.
    folders_passed: u64,
}

impl Gates {
    fn new() -> Gates {
        Gates {
            agree: GateReport::new(
                "folders: reader agrees with the reference validator",
                Tolerance::Exact,
            ),
            tables: GateReport::new("tables byte-identical", Tolerance::Exact),
            manifests: GateReport::new("folders: manifest equal as JSON", Tolerance::Exact),
            copies: GateReport::new("referenced files byte-identical", Tolerance::Exact),
            revalid: GateReport::new(
                "folders: output accepted by the reference validator, same summary and warnings",
                Tolerance::Exact,
            ),
            notes: Vec::new(),
            tolerated: BTreeMap::new(),
            numbers: 0,
            integer_and_float: 0,
            f32_equal: 0,
            folders_passed: 0,
        }
    }

    fn all(&mut self) -> [&mut GateReport; 5] {
        [
            &mut self.agree,
            &mut self.tables,
            &mut self.manifests,
            &mut self.copies,
            &mut self.revalid,
        ]
    }

    fn passed(&mut self) -> bool {
        self.all().iter().all(|g| g.failures.is_empty())
    }

    fn add(&mut self, mut o: Gates) {
        for (t, p) in self.all().into_iter().zip(o.all()) {
            t.checked += p.checked;
            t.exact += p.exact;
            t.within += p.within;
            t.failures.append(&mut p.failures);
        }
        self.notes.append(&mut o.notes);
        for (k, c) in o.tolerated {
            *self.tolerated.entry(k).or_default() += c;
        }
        self.numbers += o.numbers;
        self.integer_and_float += o.integer_and_float;
        self.f32_equal += o.f32_equal;
        self.folders_passed += o.folders_passed;
    }
}

/// One folder through every gate. Returns the gates and whether the output folder may be removed.
fn check_folder(dir: &Path, out: &Path, validator: Option<&Path>) -> (Gates, bool) {
    let mut g = Gates::new();
    let item = dir.display().to_string();
    let reference = validator.map(|v| validate(v, dir));
    let mut w = World::new();
    let ours = read::interchange::read(dir, &mut w);
    match (&ours, &reference) {
        (Ok(_), Some(Err(msg))) => {
            g.agree
                .fail(&item, format!("we accept, the reference refuses: {msg}"));
            return (g, true);
        }
        (Err(e), Some(Ok(_))) => {
            g.agree
                .fail(&item, format!("we refuse, the reference accepts: {e}"));
            return (g, true);
        }
        (Err(e), Some(Err(msg))) => {
            g.agree.record(&item, 0.0);
            if norm_cause(&e.to_string(), dir) != norm_cause(msg, dir) {
                g.notes.push(format!("{item}\n    ours: {e}\n    ref:  {msg}"));
            }
            g.folders_passed += 1;
            return (g, true);
        }
        (Err(e), None) => {
            g.notes
                .push(format!("{item}: refused (no reference to compare): {e}"));
            return (g, true);
        }
        (Ok(_), _) => g.agree.record(&item, 0.0),
    }
    let rep = ours.expect("accepted");
    g.tolerated = rep.tolerated.clone();
    // the writer reads no file: what it writes is what the scene loads into the world
    if let Err(e) = read::interchange::load(dir, &mut w) {
        g.manifests.fail(&item, format!("load failed: {e}"));
        return (g, false);
    }
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clear the output");
    }
    if let Err(e) = write::interchange::write(&w, out) {
        g.manifests.fail(&item, format!("write failed: {e}"));
        return (g, false);
    }
    let (min, mout) = (manifest(dir), manifest(out));
    let c = compare_json(&min, &mout, 0);
    g.numbers = c.numbers;
    g.integer_and_float = c.integer_and_float;
    g.f32_equal = c.f32_equal;
    if c.differences.is_empty() {
        g.manifests.record(&item, 0.0);
    } else {
        g.manifests.fail(&item, c.differences.join("; "));
    }
    let (tin, fin) = referenced(&min);
    for (gate, rels) in [(&mut g.tables, &tin), (&mut g.copies, &fin)] {
        for r in rels {
            let what = format!("{item}/{r}");
            match same_file(&dir.join(r), &out.join(r)) {
                Ok(true) => gate.record(what, 0.0),
                Ok(false) => gate.fail(what, "bytes differ"),
                Err(e) => gate.fail(what, e),
            }
        }
    }
    if let (Some(v), Some(Ok((summary, warnings)))) = (validator, &reference) {
        match validate(v, out) {
            Ok((s2, w2)) if s2 == *summary && w2 == *warnings => g.revalid.record(&item, 0.0),
            Ok((s2, w2)) => g.revalid.fail(
                &item,
                format!(
                    "summary {s2:?} vs {summary:?}; warnings {} vs {}",
                    w2.len(),
                    warnings.len()
                ),
            ),
            Err(e) => g.revalid.fail(&item, e),
        }
        // our reader's warnings against the reference's (GLB warnings excepted: we do not read GLBs)
        let mut a = rep.warnings.clone();
        let mut b = warnings.clone();
        a.sort();
        b.sort();
        if a != b {
            g.notes.push(format!(
                "{item}: warnings differ: ours {} vs ref {}",
                a.len(),
                b.len()
            ));
        }
    }
    let ok = g.passed();
    if ok {
        g.folders_passed += 1;
    }
    (g, ok)
}

#[test]
fn scene_folders_round_trip() {
    let roots: Vec<(&str, PathBuf)> = ["PRIMASE_SCENES_GTA", "PRIMASE_SCENES_SEKIRO"]
        .iter()
        .filter_map(|v| real_data(v).map(|path| (*v, path)))
        .collect();
    if roots.is_empty() {
        return;
    }
    let validator = real_data("PRIMASE_SCENE_SKATE");
    let filter = primase_testdata::real_data_value("PRIMASE_SCENE_FILTER")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_default();
    let allow_live = primase_testdata::real_data_value("PRIMASE_ALLOW_LIVE")
        .and_then(|value| value.into_string().ok())
        .is_some_and(|v| v == "1");
    let out_root = primase_testdata::real_data_value("PRIMASE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("skate-scene-gate");
    let mut folders = Vec::new();
    for (name, root) in &roots {
        let before = folders.len();
        scene_folders(root, &mut folders);
        primase_testdata::expect_items(name, "scene folders", folders.len() - before);
    }
    folders.retain(|f| f.to_string_lossy().contains(&filter));
    assert!(
        filter.is_empty() || !folders.is_empty(),
        "PRIMASE_SCENE_FILTER={filter:?} matched no scene folder under {roots:?}"
    );

    let mut total = Gates::new();
    let mut changed: Vec<String> = Vec::new();
    for (n, dir) in folders.iter().enumerate() {
        let out = out_root.join(format!("{n:03}"));
        let before = snapshot(dir);
        let (g, remove) = check_folder(dir, &out, validator.as_deref());
        if snapshot(dir) != before {
            // another process rewrote the input while we read it: neither a pass nor a mismatch
            changed.push(dir.display().to_string());
        } else {
            total.add(g);
        }
        if remove && out.exists() {
            std::fs::remove_dir_all(&out).expect("remove a passing output");
        }
    }
    println!(
        "{} scene folders: {} passed, {} changed during the run (not counted)",
        folders.len(),
        total.folders_passed,
        changed.len()
    );
    for c in &changed {
        println!("  input changed during run: {c}");
    }
    for g in total.all() {
        println!("{g}");
    }
    println!(
        "manifest numbers: {} compared; {} equal with one side an integer and the other a float; {} equal only as f32",
        total.numbers, total.integer_and_float, total.f32_equal
    );
    println!("tolerated: {:?}", total.tolerated);
    println!(
        "{} other notes (different refusal causes, warning mismatches):",
        total.notes.len()
    );
    for c in &total.notes {
        println!("  {c}");
    }
    let need_reference = validator.is_some();
    for (i, g) in total.all().into_iter().enumerate() {
        if i < 4 || need_reference {
            assert!(g.failures.is_empty(), "{g}");
        }
    }
    assert!(
        changed.is_empty() || allow_live,
        "{} input folders changed during the run (PRIMASE_ALLOW_LIVE=1 reports and continues)",
        changed.len()
    );
}

/// A JSON document walked only to refuse an object with a key twice (serde_json's `Value` keeps the last one).
struct UniqueKeys;

impl<'de> serde::Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UniqueKeys)
    }
}

impl<'de> serde::de::Visitor<'de> for UniqueKeys {
    type Value = UniqueKeys;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_i64<E>(self, _: i64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_u64<E>(self, _: u64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_f64<E>(self, _: f64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_str<E>(self, _: &str) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_unit<E>(self) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<UniqueKeys, A::Error> {
        while a.next_element::<UniqueKeys>()?.is_some() {}
        Ok(UniqueKeys)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<UniqueKeys, A::Error> {
        let mut seen = std::collections::HashSet::new();
        while let Some(k) = a.next_key::<String>()? {
            if !seen.insert(k.clone()) {
                return Err(serde::de::Error::custom(format!("key {k:?} twice in one object")));
            }
            a.next_value::<UniqueKeys>()?;
        }
        Ok(UniqueKeys)
    }
}

/// The text of every number in a JSON document that parses.
fn number_texts(b: &[u8]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    i += 1;
                }
                out.push(std::str::from_utf8(&b[start..i]).expect("ASCII"));
            }
            _ => i += 1,
        }
    }
    out
}

/// Census behind reading manifests with plain serde_json: every scene.json under the roots is strict JSON (serde_json
/// refuses a byte-order mark, comments, NaN and infinity), no object has a key twice, and every number gives the
/// same f32 read through f64 (what the reader does) as read straight from its text. A failure means the real data
/// now needs more than plain serde_json.
#[test]
fn manifests_need_only_plain_json() {
    let roots: Vec<(&str, PathBuf)> = ["PRIMASE_SCENES_GTA", "PRIMASE_SCENES_SEKIRO"]
        .iter()
        .filter_map(|v| real_data(v).map(|path| (*v, path)))
        .collect();
    if roots.is_empty() {
        return;
    }
    let mut folders = Vec::new();
    for (name, root) in &roots {
        let before = folders.len();
        scene_folders(root, &mut folders);
        primase_testdata::expect_items(name, "scene folders", folders.len() - before);
    }
    let mut gate = GateReport::new(
        "manifests: strict JSON, unique keys, every number the same f32 through f64",
        Tolerance::Exact,
    );
    let mut numbers = 0u64;
    let mut shortened = 0u64;
    for dir in &folders {
        let path = dir.join("scene.json");
        let item = path.display().to_string();
        let bytes = std::fs::read(&path).expect("scene.json");
        if let Err(e) = serde_json::from_slice::<UniqueKeys>(&bytes) {
            gate.fail(&item, e.to_string());
            continue;
        }
        let differ: Vec<&str> = number_texts(&bytes)
            .into_iter()
            .inspect(|_| numbers += 1)
            .filter(|t| {
                let direct: f32 = t.parse().expect("a JSON number");
                let via = t.parse::<f64>().expect("a JSON number") as f32;
                if direct.is_finite() {
                    let decimal = primase_core::json::f32_decimal(direct);
                    let text = serde_json::to_string(&decimal).expect("finite number");
                    let parsed: serde_json::Value = serde_json::from_str(&text).expect("JSON number");
                    assert_eq!(
                        (parsed.as_f64().unwrap() as f32).to_bits(),
                        direct.to_bits(),
                        "{t} -> {text}"
                    );
                    shortened += u64::from(decimal != f64::from(direct));
                }
                direct.to_bits() != via.to_bits()
            })
            .collect();
        if differ.is_empty() {
            gate.record(&item, 0.0);
        } else {
            gate.fail(
                &item,
                format!(
                    "{} numbers round differently through f64: {:?}",
                    differ.len(),
                    &differ[..differ.len().min(5)]
                ),
            );
        }
    }
    println!("{gate}\n{numbers} numbers; {shortened} differ from widened f64, all retain f32 bits");
    gate.assert_passed();
}
