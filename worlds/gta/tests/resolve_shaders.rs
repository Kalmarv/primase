//! `resolve-gta-shaders` on a region of the map (Vespucci Beach by default, the debug key `gta.region` `x0,y0,x1,y1`):
//! the materials bound to the G-buffer programs of their effects, by each vertex family, and the ones left unbound with
//! the reason. Every binding is verified (`Component::verify`: its programs, resources, offsets and lanes are the
//! programs' own). The counts are printed.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::pass::{Options, Pass, PassLog};
use std::collections::BTreeMap;

use primase_core::shader::{ShaderBinding, Stage};
use primase_core::{Component, World};
use primase_gta::components::GtaShader;
use primase_gta::passes::resolve_shaders::ResolveGtaShaders;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, debug, install, keys};

fn run(world: &World, vertex: &str) -> (World, PassLog) {
    let options = Options::from([
        ("pixel".to_owned(), "PS_DeferredTextured".to_owned()),
        ("vertex".to_owned(), vertex.to_owned()),
        ("unset-textures".to_owned(), "white".to_owned()),
    ]);
    let mut w = world.clone();
    let mut log = PassLog::default();
    ResolveGtaShaders::from_options(&options)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    (w, log)
}

#[test]
fn a_regions_materials_are_bound_or_left_with_a_reason() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let region: Vec<f32> = debug("gta.region")
        .unwrap_or_else(|| "-1500,-1650,-1150,-1330".into())
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    let source = GtaSource::open(&dir, &keys).unwrap();
    let (read, _) = source
        .read([region[0], region[1], region[2], region[3]], 600.0)
        .unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    let materials = world.iter::<GtaShader>().count();
    let (table, join) = source.add_shader_database(&mut world).unwrap();
    println!(
        "{materials} materials, {} effects used; {} tables read of {} programs; {} effect parameter records",
        join.effects_used, table.tables, table.shaders, table.parameters
    );
    for vertex in ["VS_TransformD", "VS_TransformSkinD", "VS_TransformCube"] {
        let (w, log) = run(&world, vertex);
        let bound = w.iter::<ShaderBinding>().count();
        for (e, b) in w.iter::<ShaderBinding>() {
            b.verify(e, &w).unwrap_or_else(|why| panic!("{why}"));
        }
        println!("vertex={vertex}: {bound} of {materials} bound");
        // the roles of the pixel stages' outputs, by (target, lanes)
        let mut roles: BTreeMap<String, usize> = BTreeMap::new();
        for (_, b) in w.iter::<ShaderBinding>() {
            for stage in b.stages.iter().filter(|s| s.stage == Stage::Pixel) {
                for o in &stage.outputs {
                    *roles
                        .entry(format!(
                            "{}{} lanes {:04b}: {:?}",
                            o.label.name, o.label.index, o.lanes, o.role
                        ))
                        .or_default() += 1;
                }
            }
        }
        println!("  output roles of the bound pixel stages: {roles:#?}");
        if vertex != "VS_TransformCube" {
            assert!(
                roles.keys().any(|r| r.ends_with(": Albedo")) && roles.keys().any(|r| r.contains("Normal")),
                "{roles:?}"
            );
        }
        // naming the vertex family leaves no effect with two programs that fit
        assert!(
            !log.counts
                .keys()
                .any(|what| what.contains("permutations that fit")),
            "{vertex}: {:?}",
            log.counts
                .keys()
                .filter(|what| what.contains("permutations that fit"))
                .collect::<Vec<_>>()
        );
        if vertex == "VS_TransformCube" {
            // the cube-map pass shares the pixel programs of the effects that list it
            assert!(bound > 0);
            continue;
        }
        for (what, n) in &log.counts {
            println!("  {n:>7}  {what}");
        }
        for (what, n) in &log.approximations {
            println!("  {n:>7}  (approximation) {what}");
        }
    }
}
