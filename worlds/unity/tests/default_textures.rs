//! Whether a Unity player holds the textures a shader's property default names (`white`, `black`, `grey`, `bump`, `red`,
//! `linearGrey`): every Texture2D and Cubemap of SkateBIRD's and Sledding Game's serialized files whose name is one
//! of those or starts with `Default`, printed (`DEFAULT TEXTURE game file path_id class name: size, format, first
//! bytes`). The player does not hold the engine's own default textures when none is printed: they are then built into the
//! engine. Needs the installs and the type tree package.
#![allow(clippy::unwrap_used)]

use primase_testdata::{Game, Tool, install_file, tool};
use primase_unity::TypeTreePackage;
use primase_unity::player::{ObjRef, Player};

const NAMES: [&str; 8] = [
    "white",
    "black",
    "grey",
    "gray",
    "bump",
    "red",
    "lineargrey",
    "normal",
];

fn scan(label: &str, data: &std::path::Path, tpk: &std::path::Path) -> usize {
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut files: Vec<String> = Vec::new();
    for d in [data.to_path_buf(), data.join("Resources")] {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            if let Some(name) = e.file_name().to_str() {
                files.push(name.to_lowercase());
            }
        }
    }
    files.sort();
    files.dedup();
    let (mut seen, mut textures) = (0usize, 0usize);
    for file in files {
        let Ok(f) = player.file(&file) else { continue };
        for o in &f.objects {
            // 28 Texture2D, 89 Cubemap
            if o.class_id != 28 && o.class_id != 89 {
                continue;
            }
            textures += 1;
            let r = ObjRef {
                file: file.clone(),
                path_id: o.path_id,
            };
            let Ok(v) = player.decode(&r) else { continue };
            let name = v.get("m_Name").and_then(|n| n.as_str()).unwrap_or_default();
            let lower = name.to_lowercase();
            if !(NAMES.contains(&lower.as_str()) || lower.starts_with("default")) {
                continue;
            }
            seen += 1;
            let size = (
                v.get("m_Width").and_then(|n| n.as_i64()),
                v.get("m_Height").and_then(|n| n.as_i64()),
            );
            let format = v.get("m_TextureFormat").and_then(|n| n.as_i64());
            let bytes = v.get("image data").and_then(|b| b.as_bytes()).unwrap_or_default();
            println!(
                "DEFAULT TEXTURE {label} {file} {} class {} {name:?}: {size:?}, format {format:?}, first bytes {:?} of {}",
                o.path_id,
                o.class_id,
                &bytes[..bytes.len().min(8)],
                bytes.len()
            );
        }
    }
    println!("DEFAULT TEXTURE {label}: {seen} named like a default of {textures} textures and cubemaps");
    seen
}

#[test]
fn which_of_unitys_default_textures_a_player_holds() {
    let Some(tpk) = tool(Tool::UnityTpk) else {
        return;
    };
    if let Some(data) = install_file(Game::Skatebird, "SkateBIRD_Data") {
        scan("SkateBIRD", &data, &tpk);
    }
    if let Some(data) = install_file(Game::Sledding, "Sledding Game_Data") {
        scan("Sledding", &data, &tpk);
    }
}
