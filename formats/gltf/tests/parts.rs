//! One GLB per object through a conversion: a world of two assets (and a loose placement) read by an in-memory
//! reader, divided by `partition-by-asset` (after `select-assets`, or not), each part placed by `place-assets` and
//! written by `GlbParts`: one file per asset named by its source, the loose content in a part of its own, what the
//! glTF does not write told once over the parts, two parts that would share a file refused; the Khronos validator on
//! every file when `PRIMASE_GLTF_VALIDATOR` is set.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;

use common::{rich_primitive, scratch};
use primase_core::convert::{NoticeKind, PlannedParts, Reader, Report, Result, convert_parts};
use primase_core::pass::{PassManager, PassRegistry};
use primase_core::pbr::PbrMaterial;
use primase_core::{Asset, Entity, Instance, Mesh, Name, Role, Source, Transform, World};
use primase_gltf::{GlbParts, WriteOptions};
use serde_json::Value;

/// A world held in memory: read whole, a part loaded as its roots' closure.
struct Held(World);

impl Reader for Held {
    fn read(&mut self) -> Result<(World, Report)> {
        let all = self.0.entities().collect();
        Ok((self.0.subset(&all), Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "pbr"]
    }
    fn prototype_box(&self, _: &World, _: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// Two assets with these sources, one part each drawn with one shared material, and a loose placement of the
/// second's part.
fn world(sources: [&str; 2]) -> World {
    let mut w = World::new();
    let material = w.spawn();
    w.insert(material, Name("paint".into()));
    w.insert(material, PbrMaterial::default());
    for (k, source) in sources.into_iter().enumerate() {
        let part = w.spawn();
        w.insert(part, Name(format!("part {k}")));
        w.insert(part, Mesh::new(vec![rich_primitive(Some(material), k as f32)]));
        let root = w.spawn();
        w.insert(root, Source(source.into()));
        w.insert(
            root,
            Asset {
                members: vec![(part, Role::Part)],
            },
        );
        if k == 1 {
            let placement = w.spawn();
            w.insert(
                placement,
                Instance {
                    prototype: part,
                    variant: 0,
                },
            );
            w.insert(placement, Transform::IDENTITY);
        }
    }
    w
}

fn manager() -> PassManager {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    PassManager::new(r).with_pass_checks(true)
}

/// The GLB's JSON chunk.
fn json(path: &std::path::Path) -> Value {
    primase_glb::Document::parse(&std::fs::read(path).unwrap())
        .unwrap()
        .json
}

/// The glTF's nodes that draw a mesh (not the group nodes).
fn drawing(j: &Value) -> usize {
    j["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n.get("mesh").is_some())
        .count()
}

/// The files in `dir`, by name.
fn files(dir: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn each_asset_is_its_own_glb() {
    let m = manager();
    let dir = scratch("parts-selected");
    let report = convert_parts(
        &mut Held(world(["props/crate", "props/barrel"])),
        &mut PlannedParts::default(),
        Box::new(GlbParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry
            .parse("select-assets{keys=props/barrel;props/crate}, partition-by-asset")
            .unwrap(),
        &m.registry.parse("place-assets").unwrap(),
    )
    .unwrap();
    assert_eq!(files(&dir), ["props_barrel.glb", "props_crate.glb"]);
    for (file, name) in [("props_crate.glb", "part 0"), ("props_barrel.glb", "part 1")] {
        let j = json(&dir.join(file));
        // one node drawing the asset's one mesh (the loose placement was left out by the selection)
        assert_eq!(drawing(&j), 1, "{file}");
        assert_eq!(j["meshes"].as_array().unwrap().len(), 1, "{file}");
        assert_eq!(j["meshes"][0]["name"], name, "{file}");
        assert_eq!(j["materials"][0]["name"], "paint", "{file}");
    }
    let bytes: u64 = files(&dir)
        .iter()
        .map(|f| std::fs::metadata(dir.join(f)).unwrap().len())
        .sum();
    assert_eq!(
        report.written[0].1,
        format!("2 files, {bytes} bytes, in {}", dir.display())
    );
    // the asset roots are not glTF objects: told once, both files as examples
    let notices = report.notices();
    let roots = notices
        .iter()
        .find(|n| {
            n.stage == "gltf" && n.reason == "core.asset components on entities the glTF does not write"
        })
        .unwrap();
    assert_eq!(roots.kind, NoticeKind::Unsupported);
    assert_eq!(roots.count, 2);
    assert_eq!(roots.examples, ["props_crate.glb", "props_barrel.glb"]);
}

/// Without a selection the loose placement is a part of its own, named by its place; it draws the part it places.
#[test]
fn content_outside_the_assets_is_a_part_of_its_own() {
    let m = manager();
    let dir = scratch("parts-outside");
    convert_parts(
        &mut Held(world(["props/crate", "props/barrel"])),
        &mut PlannedParts::default(),
        Box::new(GlbParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry.parse("partition-by-asset").unwrap(),
        &m.registry.parse("place-assets").unwrap(),
    )
    .unwrap();
    assert_eq!(files(&dir), ["part-2.glb", "props_barrel.glb", "props_crate.glb"]);
    let loose = json(&dir.join("part-2.glb"));
    assert_eq!(drawing(&loose), 1);
    assert_eq!(loose["meshes"][0]["name"], "part 1");
}

#[test]
fn two_parts_sharing_a_file_name_are_refused() {
    let m = manager();
    let dir = scratch("parts-clash");
    let err = convert_parts(
        &mut Held(world(["a/b", "a_b"])),
        &mut PlannedParts::default(),
        Box::new(GlbParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry
            .parse("select-assets{keys=a/b;a_b}, partition-by-asset")
            .unwrap(),
        &m.registry.parse("place-assets").unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("two parts are written to a_b.glb"), "{err}");
}

/// Oracle: the Khronos glTF-Validator on every file: no errors.
#[test]
fn khronos_validator_accepts_each_part() {
    let Some(validator) = primase_testdata::real_data("PRIMASE_GLTF_VALIDATOR") else {
        return;
    };
    let m = manager();
    let dir = scratch("parts-validator");
    convert_parts(
        &mut Held(world(["props/crate", "props/barrel"])),
        &mut PlannedParts::default(),
        Box::new(GlbParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry.parse("partition-by-asset").unwrap(),
        &m.registry.parse("place-assets").unwrap(),
    )
    .unwrap();
    for file in files(&dir) {
        let out = std::process::Command::new(&validator)
            .arg("-o")
            .arg(dir.join(&file))
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["issues"]["numErrors"], 0, "{file}: {}", report["issues"]);
    }
}
