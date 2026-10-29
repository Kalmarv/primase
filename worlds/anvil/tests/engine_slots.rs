//! Exact affected Materials, read from the install: engine resources stay unfilled with register-level evidence.
mod inputs;

use anvil_formats::archive::Archive;
use inputs::{Game, install};
use iridia_interface::Stage;
use primase_anvil::passes::RESOLVE_ANVIL_SHADERS;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::World;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::shader::{Shader, ShaderBinding, TextureSource};

#[test]
fn cityschool_engine_views_are_not_substituted_with_material_textures() {
    let Some(dir) = install(Game::Riders) else { return };
    let mut archive = Archive::open(&[
        dir.join("DataPC_MAD_USNAT_patch_01.forge"),
        dir.join("DataPC_MAD_USNAT.forge"),
        dir.join("DataPC_patch_01.forge"),
        dir.join("DataPC.forge"),
    ])
    .expect("archive");
    let names = [
        "MAD_MAT_TIL_Snow_Fluffy_A",
        "MAD_MAT_ATL_Rock_Mammoth_A",
        "MAD_MAT_TIL_Neon_EmissiveWhite_A",
    ];
    let mut world = World::new();
    let mut placed = Placed::default();
    let mut checked = 0;
    for name in names {
        let id = (0..4)
            .find_map(|f| {
                let forge = archive.forge(f)?;
                forge
                    .names
                    .iter()
                    .position(|n| n.name == name)
                    .map(|i| forge.index[i].id)
            })
            .expect("affected Material in the installed forges");
        let material = Placer::new(
            &mut archive,
            &mut world,
            SourceFrame::WORLD,
            InstancedLevels::Slot,
        )
        .with_shaders()
        .material(id, &mut placed)
        .expect("Material");
        let mut log = PassLog::default();
        RESOLVE_ANVIL_SHADERS
            .instance(&Options::from([("unset-textures".into(), "white".into())]))
            .expect("options")
            .run(&mut world, &mut log)
            .expect("resolve");
        let stage = world
            .get::<ShaderBinding>(material)
            .expect("binding")
            .stage(Stage::Pixel)
            .expect("pixel");
        let shader = world.get::<Shader>(stage.shader).expect("program");
        let mut slots = 0;
        for texture in &stage.textures {
            let register = shader
                .interface
                .resource(texture.resource)
                .expect("resource")
                .provenance
                .d3d
                .expect("address")
                .register;
            if register >= 75 {
                continue;
            }
            let TextureSource::Missing { why } = &texture.source else {
                panic!("{name} t{register}: invented runtime image")
            };
            assert!(
                why.contains(&format!("t{register}:")) && why.contains("runtime"),
                "{why}"
            );
            println!("{name}: {why}");
            slots += 1;
        }
        assert!(slots > 0, "{name}: no engine reads exercised");
        checked += 1;
    }
    assert_eq!(checked, 3);
}
