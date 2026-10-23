//! Every material of a region joins an effect of the install's shader database: a drawable's shader names its effect
//! by the joaat of the effect's lowercase name ([`primase_gta::read::shaders::join_materials`]).
//!
//! The effects are the install's own (no oracle lists them); what is checked is that the join holds for every
//! material the region's drawables have: none left unjoined, no hash shared by two effects, and each joined effect
//! names a vertex and a pixel program with a `core.shader` entity whose container is what the database stores. The
//! counts are printed.
//!
//! Needs `installs.gta` and `keys.gta`; the debug key `gta.region` (`x0,y0,x1,y1`) names the region, Vespucci
//! Beach by default.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::BTreeSet;

use primase_core::World;
use primase_gta::components::{GtaEffect, GtaMaterialEffect, GtaShader, GtaStage};
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, debug, install, keys};

#[test]
fn every_material_of_a_region_joins_its_effect() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let region: Vec<f32> = debug("gta.region")
        .unwrap_or_else(|| "-1500,-1650,-1150,-1330".into())
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    let source = GtaSource::open(&dir, &keys).expect("install");
    let (read, _) = source
        .read([region[0], region[1], region[2], region[3]], 600.0)
        .expect("region");
    let (mut w, _): (World, _) = source.load_all(&read).expect("assets");
    let materials = w.iter::<GtaShader>().count();
    let (table, join) = source.add_shader_database(&mut w).expect("shader database");
    println!(
        "{materials} materials; {} effects, {} programs with an interface, {} refused; joined {}, using {} effects; unjoined {:?}; ambiguous {:?}",
        table.effects.len(),
        table.shaders,
        table.unread.len(),
        join.joined,
        join.effects_used,
        join.unjoined,
        join.ambiguous
    );
    assert!(materials > 0);
    assert!(join.ambiguous.is_empty(), "{:?}", join.ambiguous);
    assert!(
        join.unjoined.is_empty(),
        "materials with no effect: {:?}",
        join.unjoined
    );
    assert_eq!(join.joined, materials);
    let mut with_programs = BTreeSet::new();
    for (_, link) in w.iter::<GtaMaterialEffect>() {
        let effect = w.get::<GtaEffect>(link.effect).expect("an effect");
        let has = |stage| {
            effect
                .programs
                .iter()
                .any(|p| p.stage == stage && p.shader.is_some())
        };
        if has(GtaStage::Vertex) && has(GtaStage::Pixel) {
            with_programs.insert(link.effect);
        }
    }
    println!(
        "{} of the {} joined effects name a vertex and a pixel program with an interface",
        with_programs.len(),
        join.effects_used
    );
    assert_eq!(with_programs.len(), join.effects_used);
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "{} components fail verification, e.g. {:?}",
        bad.len(),
        bad.first()
    );
}
