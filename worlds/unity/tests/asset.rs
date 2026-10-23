//! One asset read as a preview ([`primase_unity::asset::read_asset`]) against independent readers' decodings of the
//! same objects, for the player `installs.sledding/Sledding Game_Data` read with `tools.unity_tpk`: meshes against
//! UnityPy's (`oracles.unitypy.sledding/meshes.json`, JSON `meshes` with `file`, `path_id`, `name` and
//! `indices.count`), textures against its decoded textures' headers (`oracles.unitypy.sledding/textures`, `index.json`:
//! `name`, `width`, `height`, `mips`), materials against its materials' (`oracles.unitypy.sledding/materials.json`:
//! `name`, `shader`, `textures` as [property, texture or null, ..]).
//!
//! A mesh: the prototype's primitives index exactly the oracle's index buffer (the same count; compared where the read
//! converted no topology, the others counted), and the placement is named as the mesh. A texture: the entity's name,
//! size and level count are the oracle's. A material: its shader name, the properties that name a texture and the
//! textures read for them are the oracle's. Pixels, vertex data and material values are gated in `texture.rs`,
//! `mesh.rs` and `material.rs`, whose readers this reads through.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::Path;

use primase_core::gate::{GateReport, Tolerance};
use primase_core::{Instance, Mesh, Name, Texture, World};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_unity::TypeTreePackage;
use primase_unity::asset::read_asset;
use primase_unity::components::{UnityMaterial, UnityTexture};
use primase_unity::player::{ObjRef, Player};
use serde_json::Value as Json;

fn player() -> Option<Player> {
    let (data, tpk) = (
        install_file(Game::Sledding, "Sledding Game_Data")?,
        tool(Tool::UnityTpk)?,
    );
    Some(
        Player::open(
            &data,
            TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
        )
        .unwrap(),
    )
}

fn object(row: &Json) -> ObjRef {
    ObjRef {
        file: row["file"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_lowercase(),
        path_id: row["path_id"].as_i64().unwrap(),
    }
}

#[test]
fn meshes_read_as_assets_match_the_oracle() {
    let (Some(player), Some(oracle)) = (
        player(),
        oracle(OracleSet::UnityPy, Game::Sledding, "meshes.json"),
    ) else {
        return;
    };
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut gate = GateReport::new(
        "meshes read as assets: name, placement, indices",
        Tolerance::Exact,
    );
    let (mut converted, mut empty) = (0u64, 0u64);
    for m in oracle["meshes"].as_array().unwrap() {
        let r = object(m);
        let item = format!("{}/{} {}", r.file, r.path_id, m["name"].as_str().unwrap());
        let mut w = World::new();
        let read = read_asset(&player, &r, &mut w).unwrap();
        assert_eq!(read.class, "Mesh");
        if w.get::<Name>(read.entity).map(|n| n.0.as_str()) != m["name"].as_str() {
            gate.fail(&item, "name");
            continue;
        }
        let Some(inst) = w.get::<Instance>(read.entity) else {
            gate.fail(&item, "no placement");
            continue;
        };
        let want = m["indices"]["count"].as_u64().unwrap();
        let Some(mesh) = w.get::<Mesh>(inst.prototype) else {
            // a mesh with no submesh that has indices draws nothing
            empty += 1;
            if want == 0 {
                gate.record(&item, 0.0);
            } else {
                gate.fail(&item, format!("no geometry, the oracle has {want} indices"));
            }
            continue;
        };
        if !read.report.notices.is_empty() {
            converted += 1;
            gate.record(&item, 0.0);
            continue;
        }
        let got: u64 = mesh
            .primitives()
            .iter()
            .map(|p| p.indices.as_ref().map_or(p.vertex_count(), Vec::len) as u64)
            .sum();
        if got == want {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, format!("{got} indices vs {want}"));
        }
    }
    println!("{gate}; {converted} meshes read with notices (not compared), {empty} drawing nothing");
    gate.assert_passed();
}

#[test]
fn textures_read_as_assets_match_the_oracle() {
    let (Some(player), Some(oracle)) = (player(), oracle(OracleSet::UnityPy, Game::Sledding, "textures"))
    else {
        return;
    };
    let index: Json =
        serde_json::from_slice(&std::fs::read(Path::new(&oracle).join("index.json")).unwrap()).unwrap();
    let mut gate = GateReport::new("textures read as assets: name, size, levels", Tolerance::Exact);
    let mut not_decoded = 0u64;
    // the first 200 of the oracle's textures keep the gate quick; texture.rs reads them all
    for t in index["textures"].as_array().unwrap().iter().take(200) {
        let r = object(t);
        let item = format!("{}/{} {}", r.file, r.path_id, t["name"].as_str().unwrap());
        let mut w = World::new();
        let read = read_asset(&player, &r, &mut w).unwrap();
        assert_eq!(read.class, "Texture2D");
        if w.get::<Name>(read.entity).map(|n| n.0.as_str()) != t["name"].as_str() {
            gate.fail(&item, "name");
            continue;
        }
        let Some(pixels) = w.get::<Texture>(read.entity) else {
            // pixels the reader leaves undecoded on purpose say why on the texture
            if w.get::<UnityTexture>(read.entity)
                .is_some_and(|u| u.not_decoded.is_some())
            {
                not_decoded += 1;
                gate.record(&item, 0.0);
            } else {
                gate.fail(&item, "no pixels and no reason");
            }
            continue;
        };
        let want = (
            t["width"].as_u64().unwrap(),
            t["height"].as_u64().unwrap(),
            t["mips"].as_u64().unwrap(),
        );
        let got = (
            u64::from(pixels.width),
            u64::from(pixels.height),
            pixels.levels.len() as u64,
        );
        if got == want {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, format!("{got:?} vs {want:?}"));
        }
    }
    println!("{gate}; {not_decoded} textures left undecoded on purpose");
    gate.assert_passed();
}

#[test]
fn materials_read_as_assets_match_the_oracle() {
    let (Some(player), Some(oracle)) = (
        player(),
        oracle(OracleSet::UnityPy, Game::Sledding, "materials.json"),
    ) else {
        return;
    };
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut gate = GateReport::new("materials read as assets: shader and textures", Tolerance::Exact);
    for m in oracle["materials"].as_array().unwrap() {
        let r = object(m);
        let item = format!("{}/{} {}", r.file, r.path_id, m["name"].as_str().unwrap());
        let mut w = World::new();
        let read = read_asset(&player, &r, &mut w).unwrap();
        assert_eq!(read.class, "Material");
        let u = w.get::<UnityMaterial>(read.entity).unwrap();
        // the oracle's texture properties (a null reference names none), and this read's
        let named: Vec<(&str, bool)> = m["textures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t[0].as_str().unwrap(), !t[1].is_null()))
            .collect();
        let ours: Vec<(&str, bool)> = u
            .textures
            .iter()
            .map(|t| (t.name.as_str(), t.referenced))
            .collect();
        let same = u.shader.as_deref() == m["shader"].as_str()
            && w.get::<Name>(read.entity).map(|n| n.0.as_str()) == m["name"].as_str()
            && named == ours;
        // a texture entity per distinct Texture2D the properties reference (other classes are counted, not read)
        let wanted: BTreeSet<String> = m["textures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t[1])
            .filter(|t| !t.is_null())
            .map(Json::to_string)
            .collect();
        let made: BTreeSet<_> = u.textures.iter().filter_map(|t| t.texture).collect();
        if same && made.len() <= wanted.len() && read.report.textures == made.len() {
            gate.record(&item, 0.0);
        } else {
            gate.fail(
                &item,
                format!("shader {:?}, textures {ours:?} vs {named:?}", u.shader),
            );
        }
    }
    println!("{gate}");
    gate.assert_passed();
}
