//! The installed Stormveil slab and chip materials use the plain map program's
//! four reflected CA colour components. Their bundle declares white defaults;
//! each default belongs at its reflected byte offset, not its sequence index.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use iridia_interface::{ResourceKind, Stage};
use primase_core::{
    Name, World,
    shader::{ConstantRole, Shader, ShaderBinding},
};
use primase_fromsoft::sekiro::source::{MaterialShader, ShaderBundle};
use primase_fromsoft::{game::ParamKey, materials::Material, sekiro};
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn stormveil_stone_colours_bind_the_installed_ca_defaults() -> Result<()> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::default();
    sekiro::read_game_area(
        &install,
        "m10_00_00_00",
        &decoder,
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: [-162., 5., 108., 250.],
                margin: 30.,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
    let expected = BTreeSet::from(["aet210_081_a", "aet210_037_a"]);
    let mut checked = BTreeSet::new();
    for (entity, material) in world.iter::<Material>() {
        // The lab's PBR lowering labels these by their albedo stem. The source
        // retains the MATBIN identity, so join by texture and compiled program.
        let Some(target) = expected
            .iter()
            .copied()
            .find(|name| material.bindings.values().any(|binding| binding.texture == *name))
        else {
            continue;
        };
        let stage = world
            .get::<ShaderBinding>(entity)
            .and_then(|b| b.stage(Stage::Pixel))
            .ok_or("stone has no pixel binding")?;
        let shader = world.get::<Shader>(stage.shader).ok_or("stone has no shader")?;
        if world.get::<Name>(stage.shader).map(|n| n.0.as_str()) != Some("M[AMSN_V]_Gbuf.ppo") {
            continue;
        }
        let metadata = world
            .get::<MaterialShader>(entity)
            .and_then(|s| world.get::<ShaderBundle>(s.0))
            .and_then(|s| s.parameters.as_ref())
            .ok_or("stone has no metaparam")?;
        let (resource, ca) = shader
            .interface
            .resources
            .iter()
            .find_map(|r| {
                let ResourceKind::ConstantBuffer { layout } = &r.kind else {
                    return None;
                };
                layout
                    .constants
                    .iter()
                    .find(|c| c.name == "SAT_CAParam")
                    .map(|c| (r.id, c))
            })
            .ok_or("stone has no reflected CA structure")?;
        assert_eq!(ca.offset, 272);
        assert_eq!(ca.ty.members().len(), 4);
        for component in 0..4 {
            let suffix = format!("_snp_0_color_0_{component}");
            let member = ca
                .ty
                .members()
                .iter()
                .find(|m| m.name.ends_with(&suffix))
                .ok_or("CA component is absent")?;
            let default = metadata
                .ca_params
                .iter()
                .find(|p| p.name.ends_with(&suffix))
                .ok_or("CA default is absent")?;
            assert_eq!(default.component, component);
            assert_eq!(default.default.to_bits(), 1f32.to_bits());
            let at = ca.offset + member.offset;
            let bound = stage
                .constants
                .iter()
                .filter(|c| c.resource == resource && c.offset == at)
                .collect::<Vec<_>>();
            assert_eq!(bound.len(), 1, "{} CA component{component} at{at}", material.mtd);
            assert_eq!(bound[0].size, 4);
            assert_eq!(
                bound[0].role,
                ConstantRole::Material(vec![default.default.to_bits()])
            );
            println!(
                "{}: component{component} at byte{at} = {} (installed metaparam)",
                material.mtd, default.default
            );
        }
        let unused = metadata
            .ca_params
            .iter()
            .find(|p| p.component == 4)
            .ok_or("missing unused intensity")?;
        assert!(
            !stage
                .constants
                .iter()
                .any(|c| c.name.as_deref() == Some(unused.name.as_str()))
        );
        checked.insert(target);
    }
    assert_eq!(checked, expected, "both problematic stone materials must be read");
    Ok(())
}
