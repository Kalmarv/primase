//! Every packaged pixel program of the disc classified from its microcode, and the reader's materials carrying the
//! class: the sky domes are unlit after lowering, the panorama cards (a different program) are not.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::{
    convert::Reader,
    frame::{Axis, SourceFrame},
    pass::{PassManager, PassRegistry, RunReport},
    pbr::{PbrMaterial, Shading},
};
use primase_shred::{
    components::{MaterialId, PixelProgram, ShredMaterial},
    read::{
        convert::ShredReader,
        material_join::{MaterialInputs, TexturePreference},
        pak::Pak,
        pixel_program::classify,
    },
};
use primase_testdata::{Game, install};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}

#[test]
fn the_disc_has_seven_scaled_texture_programs_and_every_copy_of_one_agrees() {
    let Some(root) = install(Game::Shred) else { return };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut programs = 0;
    for path in &paths {
        let pak = Pak::open(path).unwrap();
        for (i, e) in pak.entries().iter().enumerate() {
            if !e.name.ends_with(b".psp") {
                continue;
            }
            programs += 1;
            let class = classify(&pak.payload(i).unwrap()).unwrap();
            classes
                .entry(String::from_utf8_lossy(&e.name).into_owned())
                .or_default()
                .insert(format!("{class:?}"));
        }
    }
    assert!(programs > 0);
    let scaled: Vec<(&str, &BTreeSet<String>)> = classes
        .iter()
        .filter(|(_, c)| c.iter().any(|c| c.starts_with("ScaledTexture")))
        .map(|(n, c)| (n.rsplit(['\\', '/']).next().unwrap(), c))
        .collect();
    // every copy of a program has one class
    assert!(classes.values().all(|c| c.len() == 1), "{classes:?}");
    let names: Vec<&str> = scaled.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "base_textured_2d_simple.psp",
            "albedo_multiply.psp",
            "confident_arrows.psp",
            "confident_line.psp",
            "env_challenge.psp",
            "env_icon.psp",
            "env_skydome.psp"
        ],
        "{scaled:?}"
    );
    let sky = classes
        .iter()
        .find(|(n, _)| n.ends_with("env_skydome.psp"))
        .unwrap()
        .1;
    assert_eq!(
        *sky,
        BTreeSet::from([r#"ScaledTexture { unit: 0, uniforms: ["GlobalBrightness"] }"#.to_string()])
    );
    // the panorama cards' program samples more than a scaled texture
    let card = classes
        .iter()
        .find(|(n, _)| n.ends_with("env_texture.psp"))
        .unwrap()
        .1;
    assert_eq!(*card, BTreeSet::from(["Unclassified".to_string()]));
}

/// The lowered world and, per material layer, its name, shading and index.
type Lowered = (primase_core::World, Vec<(String, Shading, u32)>);

fn lowered(level: &str, layers: &[&str]) -> Option<Lowered> {
    let root = install(Game::Shred)?;
    let usr = root.join("PS3_GAME/USRDIR");
    let banks = Pak::open(usr.join("database/materials.pak")).unwrap();
    let paks: Vec<(String, Pak)> = std::iter::once(format!("levels/{level}.pak"))
        .chain(layers.iter().map(|l| format!("levels/layers/{l}.pak")))
        .map(|n| {
            let pak = Pak::open(usr.join(&n)).unwrap();
            (n, pak)
        })
        .collect();
    let mut reader = ShredReader::new(
        paks.iter().map(|(n, p)| (n.as_str(), p)).collect(),
        SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0),
    )
    .with_materials(MaterialInputs {
        banks: &banks,
        assets: vec![],
        textures: TexturePreference::FullChainThenPrefetch,
    });
    let (world, _) = reader.read().unwrap();
    let (mut world, _) = reader.load_all(world).unwrap();
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    let pipeline = registry
        .parse("lower-shred-materials{normals=omit,alpha=named}")
        .unwrap();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(&mut world, &pipeline, &mut RunReport::default())
        .unwrap();
    let rows = world
        .iter::<ShredMaterial>()
        .map(|(e, m)| {
            (
                m.fragment_shader.clone(),
                world.get::<PbrMaterial>(e).unwrap().shading,
                world.get::<MaterialId>(e).unwrap().0,
            )
        })
        .collect();
    Some((world, rows))
}

#[test]
fn charlies_sky_domes_are_unlit_and_nothing_with_another_program_is() {
    let Some((world, rows)) = lowered("charlie", &["00000004", "88000004"]) else {
        return;
    };
    let sky: Vec<_> = rows.iter().filter(|(s, ..)| s.ends_with("env_skydome")).collect();
    // the hangar dome (510) and the vista dome (4534)
    assert_eq!(
        sky.iter().map(|(_, _, id)| *id).collect::<BTreeSet<_>>(),
        BTreeSet::from([510, 4534])
    );
    assert!(
        sky.iter().all(|(_, shading, _)| *shading == Shading::Unlit),
        "{sky:?}"
    );
    // the only materials the lowering makes unlit in this level
    assert_eq!(
        rows.iter()
            .filter(|r| r.1 == Shading::Unlit)
            .map(|r| r.0.rsplit(['\\', '/']).next().unwrap().to_string())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["env_challenge", "env_icon", "env_skydome"].map(String::from))
    );
    for (e, m) in world.iter::<ShredMaterial>() {
        let unlit = world.get::<PbrMaterial>(e).unwrap().shading == Shading::Unlit;
        let scaled = matches!(
            world.get::<PixelProgram>(e),
            Some(PixelProgram::ScaledTexture { .. })
        );
        assert_eq!(
            unlit,
            scaled && world.get::<PbrMaterial>(e).unwrap().base_color_texture.is_some(),
            "{}",
            m.fragment_shader
        );
    }
}

#[test]
fn mikes_panorama_cards_stay_lit_and_its_sky_domes_are_unlit() {
    let Some((_, rows)) = lowered("mike", &[]) else {
        return;
    };
    assert!(rows.iter().any(|(s, ..)| s.ends_with("env_texture")));
    for (shader, shading, _) in &rows {
        if shader.ends_with("env_texture") {
            assert_eq!(*shading, Shading::Lit);
        }
        if shader.ends_with("env_skydome") {
            assert_eq!(*shading, Shading::Unlit);
        }
    }
    assert!(rows.iter().any(|(s, ..)| s.ends_with("env_skydome")));
}
