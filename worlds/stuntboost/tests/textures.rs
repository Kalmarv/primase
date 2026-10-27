//! The install's own textures (`installs.stuntboost`).
//!
//! - X1: every `Texture2DReader` XNB under `Content` reads strictly through `Xnb::texture2d` (raw `Color`, each level
//!   its pixels' size, the whole file used), and a level whose material names such a texture (AirGainSpeed's board
//!   skin) reads with it as a `core.texture`. There is no independent decoder for XNA's raw surface: this checks the
//!   reader's strictness over every file the game ships, not the pixels' meaning.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::path::Path;

use inputs::{Game, install};
use monogame_formats::xnb::Xnb;
use primase_core::{Texture, World};
use primase_stuntboost::read_level;

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "xnb") {
            out.push(path);
        }
    }
}

#[test]
fn x1_every_raw_texture_the_game_ships_reads_strictly() {
    let Some(install) = install(Game::Stuntboost) else {
        return;
    };
    let mut all = Vec::new();
    files(&install.join("Content"), &mut all);
    let (mut textures, mut pixels) = (0, 0u64);
    for path in all {
        let bytes = std::fs::read(&path).unwrap();
        let xnb = Xnb::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (reader, _) = xnb
            .primary()
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if reader.is_texture_2d() {
            let t = xnb
                .texture2d()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            textures += 1;
            pixels += u64::from(t.width) * u64::from(t.height);
        }
    }
    println!("X1: {textures} Texture2D files read, {pixels} base-level pixels");
    assert!(textures > 0);

    let mut w = World::new();
    let report = read_level(&install, "AirGainSpeed", 0.01, &mut w).unwrap();
    assert!(
        report.textures_missing.is_empty(),
        "{:?}",
        report.textures_missing
    );
    assert!(w.iter::<Texture>().count() >= report.textures);
}
