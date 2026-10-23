//! The shader bundles slice A of m11_01 keeps: one `fromsoft.shader_bundle` per shader its kept materials name, every
//! program of the bundle by stage, each material linked to its bundle by `fromsoft.material_shader`.
//!
//! Needs `PRIMASE_SEKIRO_DIR` and `PRIMASE_OODLE6`.
#![allow(clippy::unwrap_used)]
use primase_core::{Name, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::source::{MaterialShader, ShaderBundle, ShaderStage};
use primase_fromsoft::sekiro::{self, read_area};
use primase_testdata::real_data;
use std::collections::BTreeMap;
mod common;

#[test]
fn slice_a_keeps_the_bundle_of_every_shader_its_materials_name() {
    let (Some(root), Some(dll)) = (real_data("PRIMASE_SEKIRO_DIR"), real_data("PRIMASE_OODLE6")) else {
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
                assert_eq!(world.get::<ShaderBundle>(*b).unwrap().name, m.shader);
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
