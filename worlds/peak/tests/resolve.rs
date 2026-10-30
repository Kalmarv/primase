//! What PEAK's scripts hand every shader, bound: `resolve-unity-shaders` over a level material's programs (level4's
//! M_SaltRock, `resources.assets` path id 136, shader `W/Peak_Rock`, read alone, drawn with a stored variant's
//! keywords) and level4's globals (`read_globals`) binds the light volume, a 3D texture of 572 slices, at the
//! `_LightMap` the programs sample, and the binding verifies against core.

#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, Tool, install_file, tool};
use primase_core::pass::{Options, PassLog};
use primase_core::shader::{ShaderBinding, TextureSource};
use primase_core::{Component, TextureLayers, World};
use primase_peak::read_globals;
use primase_unity::asset::read_asset;
use primase_unity::components::UnityPrograms;
use primase_unity::program::read_programs;
use primase_unity::resolve_shaders::RESOLVE_UNITY_SHADERS;
use unity_formats::TypeTreePackage;
use unity_formats::player::{ObjRef, Player};

#[test]
fn a_level_material_samples_the_scripts_light_volume() {
    let (Some(data), Some(tpk)) = (install_file(Game::Peak, "PEAK_Data"), tool(Tool::UnityTpk)) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_globals(&player, "level4", &mut w).unwrap();
    let material = ObjRef {
        file: "resources.assets".into(),
        path_id: 136,
    };
    read_asset(&player, &material, &mut w).unwrap();
    let pipeline = player.render_pipeline().unwrap().unwrap();
    read_programs(&player, &mut w, &["W/Peak_Rock"], &pipeline).unwrap();
    let tier = w
        .iter::<UnityPrograms>()
        .flat_map(|(_, p)| p.variants.iter().map(|v| v.tier))
        .max()
        .unwrap();
    // the draw's keywords, which the scene's renderer and lights set and a material read alone does not state: a stored
    // variant's (the fewest: the main light's shadows without cascades, and the additional lights' shadows)
    let keywords = "_MAIN_LIGHT_SHADOWS+_ADDITIONAL_LIGHT_SHADOWS".to_owned();
    let options = Options::from([
        ("tier".to_owned(), tier.to_string()),
        ("keywords".to_owned(), keywords),
        ("colour-space".to_owned(), "linear".to_owned()),
    ]);
    let mut log = PassLog::default();
    RESOLVE_UNITY_SHADERS
        .instance(&options)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    let volumes: Vec<_> = w.iter::<TextureLayers>().map(|(e, _)| e).collect();
    assert_eq!(volumes.len(), 1, "level4's one light volume");
    let bindings: Vec<_> = w.iter::<ShaderBinding>().collect();
    assert_eq!(bindings.len(), 1, "{:?}", log.counts);
    let (e, binding) = bindings[0];
    binding.verify(e, &w).unwrap();
    let textures: Vec<_> = binding.stages.iter().flat_map(|s| &s.textures).collect();
    let volume_bound = textures
        .iter()
        .filter(|t| matches!(t.source, TextureSource::Texture { texture, .. } if texture == volumes[0]))
        .count();
    let missing: Vec<_> = textures
        .iter()
        .filter_map(|t| match &t.source {
            TextureSource::Missing { why } => Some(why.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        volume_bound, 1,
        "the light volume binds once; left missing: {missing:?}"
    );
}
