//! The shader bundles slice A of m11_01 keeps: one `fromsoft.shader_bundle` per shader its kept materials name, every
//! program of the bundle by stage, each material linked to its bundle by `fromsoft.material_shader`.
//!
//! Needs `installs.sekiro` and `tools.oodle6`.
#![allow(clippy::unwrap_used)]
use primase_core::{Name, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::source::{MaterialShader, ShaderBundle, ShaderStage};
use primase_fromsoft::sekiro::{self, read_area};
use primase_testdata::{Game, Tool, install, oodle};
use std::collections::BTreeMap;
mod common;

#[test]
fn slice_a_keeps_the_bundle_of_every_shader_its_materials_name() {
    let (Some(root), Some(dll)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else {
        return;
    };
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let options = sekiro::Options {
        region: Some(sekiro::Region {
            rect: [-170.0, 180.0, -50.0, 300.0],
            margin: 30.0,
        }),
        ..Default::default()
    };
    let mut world = World::new();
    let report = read_area(&root, "m11_01", &decoder, &options, &mut world).unwrap();
    println!("{report:?}");
    let mut by_stage: BTreeMap<ShaderStage, usize> = BTreeMap::new();
    let mut bytes = 0;
    for (e, b) in world.iter::<ShaderBundle>() {
        assert_eq!(world.get::<Name>(e).unwrap().0, b.name);
        for p in &b.programs {
            *by_stage.entry(p.stage).or_default() += 1;
            bytes += p.code.len();
        }
    }
    println!("{} bundles {by_stage:?} {bytes} bytes", report.shader_bundles);
    println!("{:?}", world.payload_bytes());
    let mut linked = 0;
    for (e, m) in world.iter::<Material>() {
        match world.get::<MaterialShader>(e) {
            Some(MaterialShader(b)) => {
                linked += 1;
                assert!(
                    world
                        .get::<ShaderBundle>(*b)
                        .unwrap()
                        .name
                        .eq_ignore_ascii_case(&m.shader)
                );
            }
            None => println!("unlinked {} ({}) {:?}", m.mtd, m.shader, m.definition.is_some()),
        }
    }
    println!("{linked} of {} materials linked", report.materials);
    println!(
        "missing {:?} unread metaparams {:?}",
        report.missing_shader_bundles, report.unread_metaparams
    );
}

/// Elden Ring's Academy of Raya Lucaria (m14_00_00_00): every material whose material binary names a shader the
/// install bundles is linked to its bundle, read from the game's bundle binders; every program of every bundle is a
/// Shader Model 6 (DXIL) container whose shader kind is its file's stage, and every bundle's metaparam reads. The
/// shaders no binder holds are the report's `missing_shader_bundles`.
///
/// Needs `installs.elden`, `keys.elden` and `tools.oodle6`.
#[test]
fn elden_ring_materials_link_their_shader_model_6_bundles() {
    use primase_core::shader::ShaderFormat;
    use primase_fromsoft::{Game as FromSoft, Install, Keys};
    let (Some(root), Some(keys), Some(dll)) = (
        install(Game::Elden),
        primase_testdata::keys(Game::Elden),
        oodle(Tool::Oodle6),
    ) else {
        return;
    };
    let install = Install::new(
        root,
        FromSoft::EldenRing,
        Keys::from_folder(keys, FromSoft::EldenRing).unwrap(),
    );
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: u32::MAX as usize,
    };
    let mut world = World::new();
    let report = sekiro::read_game_area(
        &install,
        "m14_00_00_00",
        &decoder,
        &sekiro::Options::default(),
        &mut world,
    )
    .unwrap();
    let mut failures = Vec::new();
    let mut by_stage: BTreeMap<ShaderStage, usize> = BTreeMap::new();
    for (e, b) in world.iter::<ShaderBundle>() {
        assert_eq!(world.get::<Name>(e).unwrap().0, b.name);
        if b.parameters.is_none() {
            failures.push(format!("{}: metaparam not read", b.name));
        }
        for p in &b.programs {
            *by_stage.entry(p.stage).or_default() += 1;
            if p.format != ShaderFormat::Dxil {
                failures.push(format!("{}: {:?}", p.name, p.format));
            }
        }
    }
    let mut linked = 0;
    for (e, m) in world.iter::<Material>() {
        if m.definition.is_none() {
            continue;
        }
        match world.get::<MaterialShader>(e) {
            Some(MaterialShader(b)) => {
                linked += 1;
                assert!(
                    world
                        .get::<ShaderBundle>(*b)
                        .unwrap()
                        .name
                        .eq_ignore_ascii_case(&m.shader)
                );
            }
            None if report
                .missing_shader_bundles
                .contains(&m.shader.to_ascii_lowercase()) => {}
            None => failures.push(format!("{} ({}): not linked", m.mtd, m.shader)),
        }
    }
    println!(
        "{} bundles {by_stage:?}, {linked} materials linked, missing {:?}",
        report.shader_bundles, report.missing_shader_bundles
    );
    for f in failures.iter().take(20) {
        println!("FAILED {f}");
    }
    assert!(report.shader_bundles > 0 && linked > 0 && failures.is_empty());
}
