//! Missing engine textures retain the shader database's own name and address rather than an invented image.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use inputs::{Game, install, keys};
use primase_core::World;
use primase_core::pass::{Options, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::shader::{Shader, ShaderBinding, TextureSource};
use primase_gta::components::GtaShaderTable;
use primase_gta::passes::resolve_shaders::RESOLVE_GTA_SHADERS;
use primase_gta::read::source::GtaSource;

#[test]
fn legion_missing_slots_retain_their_stored_table_identity() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let (mut read, _) = source.read([95.0, -1034.0, 295.0, -834.0], 600.0).unwrap();
    let mut registry = PassRegistry::new();
    registry.add(&primase_core::state::RESOLVE_STATE);
    primase_gta::register(&mut registry);
    let pipeline = registry.parse("resolve-state{assign=gta.map:online;gta.hour:12}, gta-select-region{x0=95,y0=-1034,x1=295,y1=-834,min_size=0.5,max_priority=3}, gta-leave-out-interiors").unwrap();
    PassManager::new(registry)
        .run(&mut read, &pipeline, &mut RunReport::default())
        .unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    source.add_shader_database(&mut world).unwrap();
    let options = Options::from([
        ("technique".into(), "deferred_draw".into()),
        ("unset-textures".into(), "white".into()),
    ]);
    let mut log = PassLog::default();
    RESOLVE_GTA_SHADERS
        .instance(&options)
        .unwrap()
        .run(&mut world, &mut log)
        .unwrap();
    let mut checked = 0;
    let mut reasons = std::collections::BTreeMap::new();
    for (_, binding) in world.iter::<ShaderBinding>() {
        for stage in &binding.stages {
            let shader = world.get::<Shader>(stage.shader).unwrap();
            let table = &world.get::<GtaShaderTable>(stage.shader).unwrap().0;
            for texture in &stage.textures {
                let TextureSource::Missing { why } = &texture.source else {
                    continue;
                };
                assert!(!why.trim().is_empty());
                let d = shader
                    .interface
                    .resource(texture.resource)
                    .unwrap()
                    .provenance
                    .d3d
                    .unwrap();
                assert!(
                    why.contains(&format!("t{} space {}", d.register, d.space)),
                    "{why}"
                );
                let record = table
                    .resources
                    .iter()
                    .find(|r| {
                        [258, 322, 514, 770].contains(&r.tag)
                            && u32::from(r.register) == d.register
                            && u32::from(r.space) == d.space
                    })
                    .unwrap();
                if let Some(name) = table.name_of(record.name) {
                    assert!(why.contains(name), "{name}: {why}");
                }
                *reasons.entry(why.clone()).or_insert(0usize) += 1;
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "the real region exercised no refusal");
    for (why, n) in reasons {
        println!("{n} material-stage slots: {why}");
    }
}
