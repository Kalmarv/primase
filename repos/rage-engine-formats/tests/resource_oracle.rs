//! Drawables and texture dictionaries against an independent reader's decoding of the same files.
//!
//! Oracle (`oracles.codewalker.gta/resources`): a folder of JSON files `{path, kind: "drawable" | "ytd", value}`:
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
//! Needs `installs.gta` and `keys.gta`.
mod inputs;

use std::fs::File;

mod codewalker;

use codewalker::{check_drawable, check_ytd};
use inputs::{Game, OracleSet, install, keys, oracle};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::drawable::Drawable;
use rage_engine_formats::resource::Resource;
use rage_engine_formats::texture::TextureDictionary;
use serde_json::Value;

#[test]
fn drawables_and_textures_against_the_oracle() {
    let (Some(dir), Some(key_dir), Some(oracle)) = (
        install(Game::Gta),
        keys(Game::Gta),
        oracle(OracleSet::CodeWalker, Game::Gta, "resources"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir, None).expect("key material");
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
