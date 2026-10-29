//! `resolve-gta-shaders` on a region of the map (Vespucci Beach by default, the debug key `gta.region` `x0,y0,x1,y1`):
//! the materials bound to the G-buffer programs of their effects, by each vertex family, and the ones left unbound with
//! the reason. Every binding is verified (`Component::verify`: its programs, resources, offsets and lanes are the
//! programs' own). The counts are printed.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use primase_core::pass::{Options, PassLog};
use std::collections::BTreeMap;

use inputs::{Game, debug, install, keys};
use iridia_interface::{SamplerState, Stage};
use primase_core::shader::ConstantRole;
use primase_core::{Component, World};
use primase_core::{shader::Shader, shader::ShaderBinding, shader::TextureSource};
use primase_gta::components::{GtaParam, GtaShader};
use primase_gta::passes::resolve_shaders::RESOLVE_GTA_SHADERS;
use primase_gta::read::source::GtaSource;

fn run(world: &World, technique: &str) -> (World, PassLog) {
    let options = Options::from([
        ("technique".to_owned(), technique.to_owned()),
        ("unset-textures".to_owned(), "white".to_owned()),
    ]);
    let mut w = world.clone();
    let mut log = PassLog::default();
    RESOLVE_GTA_SHADERS
        .instance(&options)
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
    (w, log)
}

/// The G-buffer pixel programs read the material's diffuse texture at register 0 of space 1 (the record of that bind
/// point with a texture tag is `DiffuseTex`, the sampler heap's record sharing the bind point is not a texture): where
/// the material stores a `DiffuseTex` texture, that slot is bound to it.
fn diffuse_is_bound(w: &World) {
    let key = primase_foundations::hash::joaat(b"DiffuseTex");
    let (mut checked, mut missing) = (0usize, 0usize);
    for (e, b) in w.iter::<ShaderBinding>() {
        let Some(GtaParam::Texture {
            texture: Some(diffuse),
            ..
        }) = w
            .get::<GtaShader>(e)
            .and_then(|m| m.params.iter().find(|(k, _)| *k == key).map(|(_, p)| p))
        else {
            continue;
        };
        for stage in b.stages.iter().filter(|s| s.stage == Stage::Pixel) {
            let interface = &w.get::<Shader>(stage.shader).unwrap().interface;
            for t in &stage.textures {
                let bind = interface
                    .resources
                    .iter()
                    .find(|r| r.id == t.resource)
                    .and_then(|r| r.provenance.d3d)
                    .unwrap();
                if (bind.space, bind.register) != (1, 0) {
                    continue;
                }
                checked += 1;
                match &t.source {
                    TextureSource::Texture { texture, .. } if texture == diffuse => {}
                    TextureSource::Missing { .. } => missing += 1,
                    other => panic!("the diffuse slot of {e:?} is {other:?}"),
                }
            }
        }
    }
    println!("  diffuse slots checked: {checked}, left Missing: {missing}");
    assert!(checked > 1_000, "{checked}");
    // the diffuse texture may not be loaded (its entity is then not a texture); the slot is bound otherwise
    assert!(missing * 20 < checked, "{missing} of {checked}");
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
    for technique in [
        "deferred_draw",
        "deferred_drawskinned",
        "deferredalphaclip_draw",
        "cube_draw",
        "lightweight0_draw",
        "wdcascade_draw",
    ] {
        let (w, log) = run(&world, technique);
        let bound = w.iter::<ShaderBinding>().count();
        for (e, b) in w.iter::<ShaderBinding>() {
            b.verify(e, &w).unwrap_or_else(|why| panic!("{why}"));
        }
        println!("technique={technique}: {bound} of {materials} bound");
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
        // the engine's matrices (the vertex stage's `g_rage_matrices`) have roles, however the effect names them
        let (mut matrices, mut unknown) = (0usize, Vec::new());
        for (_, b) in w.iter::<ShaderBinding>() {
            for stage in b.stages.iter().filter(|s| s.stage == Stage::Vertex) {
                for c in &stage.constants {
                    let Some(name) = c.name.as_deref() else { continue };
                    if ![
                        "gWorld",
                        "gWorldView",
                        "gWorldViewProj",
                        "gViewInverse",
                        "gRelativeWorld",
                        "gRelativeWorldView",
                        "gRelativeWorldViewProj",
                        "gRelativePrevWorld",
                        "gRelativePrevWorldView",
                        "gRelativePrevWorldViewProj",
                        "gPrevWorld",
                        "gPrevWorldView",
                        "gPrevWorldViewProj",
                        "gPrevViewProj",
                    ]
                    .contains(&name)
                    {
                        continue;
                    }
                    matrices += 1;
                    if matches!(c.role, ConstantRole::Unknown { .. }) && unknown.len() < 5 {
                        unknown.push(name.to_owned());
                    }
                }
            }
        }
        println!("  engine matrices bound: {matrices}, Unknown: {unknown:?}");
        assert!(unknown.is_empty(), "{unknown:?}");
        // the samplers of the bound pixel stages' textures: a static sampler's state, a declared sampler's resource
        let (mut textured, mut stated, mut declared) = (0usize, 0usize, 0usize);
        for (_, b) in w.iter::<ShaderBinding>() {
            for stage in b.stages.iter().filter(|s| s.stage == Stage::Pixel) {
                for t in &stage.textures {
                    if let TextureSource::Texture { sampler, .. } = &t.source {
                        textured += 1;
                        stated += usize::from(*sampler != SamplerState::default());
                    }
                    declared += usize::from(t.sampler.is_some());
                }
            }
        }
        println!(
            "  textures bound: {textured}, of them with a sampler state: {stated}; through a declared sampler: {declared}"
        );
        if technique == "deferred_draw" {
            assert!(stated > 0, "{textured} textures, none with a sampler state");
            diffuse_is_bound(&w);
        }
        assert!(bound > 0, "{technique}");
        // the G-buffer techniques have the roles of the G-buffer's lanes
        if technique.starts_with("deferred") {
            assert!(
                [
                    "Albedo",
                    "Normal",
                    "SpecularIntensity",
                    "SpecularExponent",
                    "Fresnel",
                    "MotionVector"
                ]
                .iter()
                .all(|role| roles.keys().any(|r| r.contains(role))),
                "{roles:?}"
            );
        }
        // the refusals are by name: no technique, several passes, tessellation, a missing table
        for what in log.counts.keys().filter(|w| w.contains("left unbound")) {
            assert!(
                [
                    "has no technique",
                    "passes",
                    "is tessellated",
                    "no technique table",
                    "no vertex program",
                    "has no interface"
                ]
                .iter()
                .any(|r| what.contains(r)),
                "{what}"
            );
        }
        for (what, n) in &log.counts {
            println!("  {n:>7}  {what}");
        }
        for (what, n) in &log.approximations {
            println!("  {n:>7}  (approximation) {what}");
        }
    }
}
