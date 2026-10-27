//! Field parity with CodeWalker over whole areas: every file the placements of the gate regions reach, read by our
//! reader and by CodeWalker (an independent reader), compared field by field.
//!
//! Oracle (`oracles.codewalker.gta/parity`): a folder holding `dump.jsonl`, written by the CodeWalker harness's `batch`
//! mode from a list of archive paths, one JSON line per file: `{path, kind, value}` with `kind` one of `drawable` (a
//! ydr), `fragment` (a yft's main drawable), `ydd` (`[{hash, drawable}]`), `ytd`, `ymap` (the header, `entities` and
//! `interiors` apart, each in file order, every field) and `ytyp` (`archetypes` in file order, every base field, a time
//! archetype's `time_flags`), or `{path, kind: "error", message}` where CodeWalker could not read the file. Drawables
//! and texture dictionaries are compared as `resource_oracle.rs` compares them; floats compare bit for bit.
//!
//! Known disagreements are listed in `parity-exceptions.toml` with a reason: each matches the differences of the files
//! whose path contains its `path` and whose field starts with its `field`. An exception that matches nothing fails the
//! test, so the list cannot go stale.
//!
//! The list itself is made by `parity_list` (ignored; run it explicitly): every ymap, ytyp, drawable, fragment,
//! drawable dictionary and texture dictionary the placements of Legion and Vespucci reach (read with 600 m of
//! reach), written to `gta-parity/list.txt` under the scratch folder.
//!
//! Needs `installs.gta` and `keys.gta`.
mod inputs;

use std::collections::BTreeSet;
use std::fs::File;

mod codewalker;

use codewalker::{check_drawable, check_ytd};
use inputs::{Game, OracleSet, install, keys, oracle};
use primase_core::Instance;
use primase_gta::components::{GtaArchetype, GtaEntity};
use primase_gta::read::source::GtaSource;
use rage_engine_formats::archive::archive_set::{ArchiveSet, Install, MapState};
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::drawable::{Drawable, DrawableDictionary, Fragment};
use rage_engine_formats::map::{ArchetypeDef, ArchetypeKind, EntityDef, EntityKind, MapData, MapTypes};
use rage_engine_formats::meta::Meta;
use rage_engine_formats::resource::Resource;
use rage_engine_formats::texture::TextureDictionary;
use serde_json::Value;

/// The areas listed: the gate regions, `[x0, y0, x1, y1]` in GTA ground coordinates, read with this reach.
const AREAS: &[(&str, [f32; 4])] = &[
    ("legion", [95.0, -1034.0, 295.0, -834.0]),
    ("vespucci", [-1500.0, -1650.0, -1150.0, -1330.0]),
];
const REACH: f32 = 600.0;

#[derive(serde::Deserialize)]
struct Exceptions {
    #[serde(default)]
    exception: Vec<Exception>,
}

#[derive(serde::Deserialize)]
struct Exception {
    path: String,
    field: String,
    reason: String,
}

fn u(diffs: &mut Vec<String>, at: &str, field: &str, ours: u64, o: &Value) {
    if o[field].as_u64() != Some(ours) {
        diffs.push(format!("{at}{field}: ours {ours}, codewalker {}", o[field]));
    }
}

fn i(diffs: &mut Vec<String>, at: &str, field: &str, ours: i64, o: &Value) {
    if o[field].as_i64() != Some(ours) {
        diffs.push(format!("{at}{field}: ours {ours}, codewalker {}", o[field]));
    }
}

/// Bit for bit: CodeWalker writes a float's shortest round-trip form.
fn same(ours: &[f32], o: &Value) -> bool {
    let theirs: Option<Vec<f32>> = match o {
        Value::Array(a) => a.iter().map(|v| v.as_f64().map(|x| x as f32)).collect(),
        v => v.as_f64().map(|x| vec![x as f32]),
    };
    theirs
        .is_some_and(|t| t.len() == ours.len() && t.iter().zip(ours).all(|(a, b)| a.to_bits() == b.to_bits()))
}

fn f(diffs: &mut Vec<String>, at: &str, field: &str, ours: &[f32], o: &Value) {
    if !same(ours, &o[field]) {
        diffs.push(format!("{at}{field}: ours {ours:?}, codewalker {}", o[field]));
    }
}

fn check_entity(at: &str, e: &EntityDef, o: &Value, d: &mut Vec<String>) {
    u(d, at, "archetype", e.archetype.into(), o);
    u(d, at, "flags", e.flags.into(), o);
    u(d, at, "guid", e.guid.into(), o);
    f(d, at, "position", &e.position, o);
    f(d, at, "rotation", &e.rotation, o);
    f(d, at, "scale_xy", &[e.scale_xy], o);
    f(d, at, "scale_z", &[e.scale_z], o);
    i(d, at, "parent_index", e.parent_index.into(), o);
    f(d, at, "lod_dist", &[e.lod_dist], o);
    f(d, at, "child_lod_dist", &[e.child_lod_dist], o);
    i(d, at, "lod_level", e.lod_level.into(), o);
    u(d, at, "num_children", e.num_children.into(), o);
    i(d, at, "priority_level", e.priority_level.into(), o);
    u(d, at, "extensions", e.extensions as u64, o);
    i(
        d,
        at,
        "ambient_occlusion_multiplier",
        e.ambient_occlusion_multiplier.into(),
        o,
    );
    i(
        d,
        at,
        "artificial_ambient_occlusion",
        e.artificial_ambient_occlusion.into(),
        o,
    );
    u(d, at, "tint_value", e.tint_value.into(), o);
}

fn check_list<T>(
    what: &str,
    ours: &[&T],
    o: &Value,
    d: &mut Vec<String>,
    check: impl Fn(&str, &T, &Value, &mut Vec<String>),
) {
    let theirs = o.as_array().map_or(&[][..], Vec::as_slice);
    if ours.len() != theirs.len() {
        d.push(format!(
            "{what} count: ours {}, codewalker {}",
            ours.len(),
            theirs.len()
        ));
    }
    for (k, (a, b)) in ours.iter().zip(theirs).enumerate() {
        check(&format!("{what} {k} "), a, b, d);
    }
}

fn check_ymap(m: &MapData, o: &Value, d: &mut Vec<String>) {
    u(d, "", "name", m.name.into(), o);
    u(d, "", "parent", m.parent.into(), o);
    u(d, "", "flags", m.flags.into(), o);
    u(d, "", "content_flags", m.content_flags.into(), o);
    for (field, ours) in [
        ("streaming_extents", &m.streaming_extents),
        ("entities_extents", &m.entities_extents),
    ] {
        if !same(ours.as_flattened(), &flat(&o[field])) {
            d.push(format!("{field}: ours {ours:?}, codewalker {}", o[field]));
        }
    }
    let of = |k: EntityKind| m.entities.iter().filter(|e| e.kind == k).collect::<Vec<_>>();
    check_list("entity", &of(EntityKind::Entity), &o["entities"], d, check_entity);
    check_list(
        "interior",
        &of(EntityKind::Interior),
        &o["interiors"],
        d,
        check_entity,
    );
}

/// An array of arrays of floats as one flat array.
fn flat(v: &Value) -> Value {
    Value::Array(
        v.as_array()
            .into_iter()
            .flatten()
            .flat_map(|x| x.as_array().cloned().unwrap_or_default())
            .collect(),
    )
}

fn check_archetype(at: &str, a: &ArchetypeDef, o: &Value, d: &mut Vec<String>) {
    let kind = match a.kind {
        ArchetypeKind::Base => "base",
        ArchetypeKind::Time => "time",
        ArchetypeKind::Interior => "mlo",
    };
    if o["kind"].as_str() != Some(kind) {
        d.push(format!("{at}kind: ours {kind}, codewalker {}", o["kind"]));
    }
    u(d, at, "name", a.name.into(), o);
    f(d, at, "lod_dist", &[a.lod_dist], o);
    u(d, at, "flags", a.flags.into(), o);
    u(d, at, "special_attribute", a.special_attribute.into(), o);
    f(d, at, "bb_min", &a.bb_min, o);
    f(d, at, "bb_max", &a.bb_max, o);
    f(d, at, "bs_centre", &a.bs_centre, o);
    f(d, at, "bs_radius", &[a.bs_radius], o);
    f(d, at, "hd_texture_dist", &[a.hd_texture_dist], o);
    u(d, at, "texture_dictionary", a.texture_dictionary.into(), o);
    u(d, at, "clip_dictionary", a.clip_dictionary.into(), o);
    u(d, at, "drawable_dictionary", a.drawable_dictionary.into(), o);
    u(d, at, "physics_dictionary", a.physics_dictionary.into(), o);
    i(d, at, "asset_type", a.asset_type.into(), o);
    u(d, at, "asset_name", a.asset_name.into(), o);
    if o["time_flags"].as_u64() != a.time_flags.map(u64::from) {
        d.push(format!(
            "{at}time_flags: ours {:?}, codewalker {}",
            a.time_flags, o["time_flags"]
        ));
    }
    u(d, at, "extensions", a.extensions as u64, o);
}

fn check_ytyp(t: &MapTypes, o: &Value, d: &mut Vec<String>) {
    u(d, "", "name", t.name.into(), o);
    let ours: Vec<&ArchetypeDef> = t.archetypes.iter().collect();
    check_list("archetype", &ours, &o["archetypes"], d, check_archetype);
}

fn check_ydd(dict: &DrawableDictionary, o: &Value, d: &mut Vec<String>) {
    let theirs = o.as_array().map_or(&[][..], Vec::as_slice);
    if dict.drawables.len() != theirs.len() {
        d.push(format!(
            "entries: ours {}, codewalker {}",
            dict.drawables.len(),
            theirs.len()
        ));
    }
    for (k, (h, t)) in dict.hashes.iter().zip(theirs).enumerate() {
        if t["hash"].as_u64() != Some(u64::from(*h)) {
            d.push(format!(
                "entry {k} hash: ours {h:#010x}, codewalker {}",
                t["hash"]
            ));
            continue;
        }
        let mut sub = Vec::new();
        if let Some(ours) = dict.get(*h) {
            check_drawable(ours, &t["drawable"], &mut sub);
        }
        d.extend(sub.into_iter().map(|s| format!("entry {h:#010x} {s}")));
    }
}

fn open(dir: &std::path::Path, key_dir: &std::path::Path) -> (Install, Keys) {
    let keys = Keys::from_dir(key_dir, Some(&primase_gta::read::key_digests::DIGESTS)).expect("key material");
    let install = Install::open(dir, &keys).expect("install");
    (install, keys)
}

/// The list of files the parity dump covers (see the module documentation).
#[test]
#[ignore = "writes the list for the harness; run explicitly"]
fn parity_list() {
    let (Some(dir), Some(key_dir)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let out = inputs::scratch();
    let source = GtaSource::open(&dir, &key_dir).expect("install");
    let (install, keys) = open(&dir, &key_dir);
    let set = ArchiveSet::build(install, &keys, MapState::Online).expect("archive set");
    let live = |ext: &str, hash: u32| {
        set.tables
            .get(ext)
            .and_then(|t| t.get(&hash))
            .map(|f| set.path(*f).to_string())
    };
    let mut files: BTreeSet<String> = BTreeSet::new();
    for (name, region) in AREAS {
        let (w, _) = source.read(*region, REACH).expect("region");
        for (_, e) in w.iter::<GtaEntity>() {
            files.insert(e.ymap.clone());
        }
        let used: BTreeSet<_> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        for p in used {
            let Some(a) = w.get::<GtaArchetype>(p) else {
                continue;
            };
            files.insert(a.ytyp.clone());
            files.extend(match a.asset_type {
                1 => live("yft", a.asset_name),
                2 => live("ydr", a.asset_name),
                3 => live("ydd", a.drawable_dictionary),
                _ => None,
            });
            if a.texture_dictionary != 0 {
                files.extend(live("ytd", a.texture_dictionary));
            }
        }
        println!("{name}: {} files so far", files.len());
    }
    let folder = std::path::Path::new(&out).join("gta-parity");
    std::fs::create_dir_all(&folder).expect("output folder");
    let list: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|p| !p.is_empty())
        .collect();
    std::fs::write(folder.join("list.txt"), list.join("\n") + "\n").expect("list");
    println!(
        "{} files listed in {}",
        list.len(),
        folder.join("list.txt").display()
    );
}

#[test]
fn parity_with_codewalker() {
    let (Some(dir), Some(key_dir), Some(oracle)) = (
        install(Game::Gta),
        keys(Game::Gta),
        oracle(OracleSet::CodeWalker, Game::Gta, "parity"),
    ) else {
        return;
    };
    let (install, keys) = open(&dir, &key_dir);
    let exceptions: Exceptions = toml::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/parity-exceptions.toml"
        ))
        .expect("exceptions"),
    )
    .expect("exceptions toml");
    let mut used = vec![0usize; exceptions.exception.len()];
    let dump = std::fs::read_to_string(oracle.join("dump.jsonl")).expect("dump.jsonl");
    let (mut files, mut failed, mut unreadable) = (0usize, 0usize, 0usize);
    for line in dump.lines().filter(|l| !l.trim().is_empty()) {
        let o: Value = serde_json::from_str(line).expect("dump line");
        let path = o["path"].as_str().expect("path");
        files += 1;
        if o["kind"] == "error" {
            println!("{path}: codewalker could not read it: {}", o["message"]);
            unreadable += 1;
            continue;
        }
        let r = install.file(path).expect("file in the install");
        let a = &install.archives[r.archive];
        let res: Resource = a
            .resource(&mut File::open(&a.file).expect("open archive"), r.entry, &keys)
            .expect("resource");
        let mut diffs = Vec::new();
        let v = &o["value"];
        match o["kind"].as_str() {
            Some("drawable") => {
                check_drawable(&Drawable::read_resource(&res).expect("drawable"), v, &mut diffs)
            }
            Some("fragment") => match Fragment::read_resource(&res).expect("fragment").drawable {
                Some(d) => check_drawable(&d, v, &mut diffs),
                None => diffs.push("fragment without a main drawable, codewalker has one".into()),
            },
            Some("ydd") => check_ydd(
                &DrawableDictionary::read_resource(&res).expect("ydd"),
                v,
                &mut diffs,
            ),
            Some("ytd") => check_ytd(
                &TextureDictionary::read_resource(&res).expect("ytd"),
                v,
                &mut diffs,
            ),
            Some("ymap") => check_ymap(
                &MapData::from_root(&Meta::read(&res).expect("meta").root().expect("root")).expect("ymap"),
                v,
                &mut diffs,
            ),
            Some("ytyp") => check_ytyp(
                &MapTypes::from_root(&Meta::read(&res).expect("meta").root().expect("root")).expect("ytyp"),
                v,
                &mut diffs,
            ),
            k => panic!("{path}: kind {k:?}"),
        }
        // the reviewed exceptions take the differences they name
        diffs.retain(|d| {
            match exceptions
                .exception
                .iter()
                .position(|e| path.contains(&e.path) && d.starts_with(&e.field))
            {
                Some(k) => {
                    used[k] += 1;
                    false
                }
                None => true,
            }
        });
        if !diffs.is_empty() {
            println!("{path}: {} differences", diffs.len());
            for d in diffs.iter().take(20) {
                println!("    {d}");
            }
            failed += 1;
        }
    }
    inputs::expect_found(
        inputs::Query::Oracle(OracleSet::CodeWalker, Game::Gta, "parity"),
        "dumped files",
        files,
    );
    for (e, n) in exceptions.exception.iter().zip(&used) {
        println!(
            "exception {} / {}: {n} differences ({})",
            e.path, e.field, e.reason
        );
    }
    let stale: Vec<String> = exceptions
        .exception
        .iter()
        .zip(&used)
        .filter(|(_, n)| **n == 0)
        .map(|(e, _)| format!("{} / {}", e.path, e.field))
        .collect();
    println!("{files} files, {failed} with differences, {unreadable} codewalker could not read");
    assert!(stale.is_empty(), "exceptions that match nothing: {stale:?}");
    assert_eq!(failed, 0);
}
