//! `bake-anvil-material` on every Material of the USNAT forge and its patch (Riders Republic; `installs.riders`), with
//! the options the other bake tests use at 64 texels, and a SHA-256 of every baked Material's colour, roughness and
//! normal texels, by Material, and of the lot. The digest is the evidence that a change under the bake (the program
//! executor behind the shader) leaves its output byte for byte as it was: run before and after and compare, or give the
//! earlier total as the debug key `anvil.bake_digest`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_anvil::passes::BAKE_ANVIL_MATERIAL;
use primase_anvil::read::archive::Archive;
use primase_anvil::read::material::MATERIAL;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Texture, World};
use primase_testdata::{Game, install};
use sha2::{Digest, Sha256};

const ORDER: [&str; 4] = [
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_baked_materials_of_the_usnat_forge() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).unwrap();
    // every Material of the two USNAT forges
    let mut ids = Vec::new();
    for f in [0, 1] {
        let forge = archive.forge(f).unwrap();
        for k in 0..forge.names.len() {
            if forge.names[k].type_id == MATERIAL && !ids.contains(&forge.index[k].id) {
                ids.push(forge.index[k].id);
            }
        }
    }
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut materials: Vec<(u64, Entity)> = Vec::new();
    {
        let mut placer =
            Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot).with_shaders();
        for &id in &ids {
            if let Some(e) = placer.material(id, &mut placed) {
                materials.push((id, e));
            }
        }
    }
    let pass = BAKE_ANVIL_MATERIAL
        .instance(&Options::from(
            [
                ("size", "64"),
                ("normal-green", "up"),
                ("shader-uv", "texcoord"),
                ("mip-bias", "0"),
                ("terrain-ao", "0.5"),
                ("sun-elevation", "30"),
                ("view-cosine", "0.7"),
                ("camera-distance", "1000000"),
                ("unset", "white"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned())),
        ))
        .unwrap();
    let mut log = PassLog::default();
    let started = std::time::Instant::now();
    pass.run(&mut w, &mut log).unwrap();
    println!(
        "{} Materials placed, baked in {:.1} s",
        materials.len(),
        started.elapsed().as_secs_f64()
    );
    for (what, n) in &log.counts {
        println!("  {n:6}  {what}");
    }
    for (what, n) in &log.omitted {
        println!("  {n:6}  omitted: {what}");
    }
    let mut digests: BTreeMap<u64, String> = BTreeMap::new();
    for &(id, e) in &materials {
        let Some(pbr) = w.get::<PbrMaterial>(e) else {
            continue;
        };
        let mut h = Sha256::new();
        let mut any = false;
        for t in [
            pbr.base_color_texture.as_ref(),
            pbr.metallic_roughness_texture.as_ref(),
            pbr.normal_texture.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            // a baked map is a texture this pass made: named, 64 texels square
            if let Some(t) = w.get::<Texture>(t.texture)
                && t.width == 64
            {
                any = true;
                h.update(&t.levels[0]);
            }
        }
        if any {
            digests.insert(id, hex(&h.finalize()));
        }
    }
    let mut total = Sha256::new();
    for (id, d) in &digests {
        total.update(id.to_le_bytes());
        total.update(d.as_bytes());
    }
    let total = hex(&total.finalize());
    println!("{} Materials baked, digest {total}", digests.len());
    if let Some(expected) = primase_testdata::debug("anvil.bake_digest") {
        assert_eq!(
            total, expected,
            "the baked Materials differ from the digest given"
        );
    }
}
