//! Real reader/load/pass integration, not an independent shaded-render oracle.
mod inputs;

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Texture, World, convert::Reader, frame::SourceFrame};
use primase_shred::{
    components::{ShredMaterial, SourceTexture},
    read::{
        convert::ShredReader,
        material_join::{MaterialInputs, TexturePreference},
    },
};
use inputs::{Game, install};
use rme_formats::pak::Pak;
use std::collections::{BTreeMap, BTreeSet};

fn lower(world: &mut World, normals: &str) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    let pipeline = registry
        .parse(&format!("lower-shred-materials{{normals={normals},alpha=named}}"))
        .expect("registered SHRED material pipeline");
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(world, &pipeline, &mut report)
        .expect("SHRED lowering with declared effects and verified values");
    report
}

#[test]
fn charlie_oscar_mike_materials_lower_after_load_and_keep_source_pixels_and_copy_graph() {
    let Some(root) = install(Game::Shred) else { return };
    let dir = root.join("PS3_GAME/USRDIR");
    let banks = Pak::open(dir.join("database/materials.pak")).unwrap();
    for name in ["charlie", "oscar", "mike"] {
        let base = Pak::open(dir.join(format!("levels/{name}.pak"))).unwrap();
        let layer = if name == "charlie" {
            Some(Pak::open(dir.join("levels/layers/00000004.pak")).unwrap())
        } else {
            None
        };
        let mut paks = vec![(name, &base)];
        if let Some(layer) = &layer {
            paks.push(("training", layer));
        }
        let mut reader = ShredReader::new(paks, SourceFrame::WORLD).with_materials(MaterialInputs {
            banks: &banks,
            assets: vec![],
            textures: TexturePreference::FullChainThenPrefetch,
        });
        let (plan, _) = reader.read().unwrap();
        let roots: BTreeSet<_> = plan.iter::<Instance>().take(20).map(|(e, _)| e).collect();
        let (loaded, _) = reader.load(&plan, &roots).unwrap();
        for mode in ["omit", "ag-down"] {
            let mut world = loaded.clone();
            let source: BTreeMap<_, _> = world.iter::<Texture>().map(|(e, t)| (e, t.clone())).collect();
            let graphs: BTreeMap<_, _> = world
                .iter::<SourceTexture>()
                .map(|(e, t)| (e, t.clone()))
                .collect();
            let report = lower(&mut world, mode);
            assert_eq!(
                world.iter::<PbrMaterial>().count(),
                world.iter::<ShredMaterial>().count()
            );
            for (e, t) in &source {
                assert_eq!(world.get::<Texture>(*e), Some(t));
            }
            for (e, t) in &graphs {
                assert_eq!(world.get::<SourceTexture>(*e), Some(t));
            }
            assert!(world.verify_components().is_empty());
            let mut colours = 0;
            let mut normals = 0;
            let mut specular = 0;
            let mut masks = 0;
            for (_, p) in world.iter::<PbrMaterial>() {
                colours += usize::from(p.base_color_texture.is_some());
                normals += usize::from(p.normal_texture.is_some());
                specular += usize::from(p.specular.is_some());
                masks += usize::from(matches!(p.alpha_mode, primase_core::pbr::AlphaMode::Mask { .. }));
                for texture in p.textures() {
                    assert!(world.get::<Texture>(texture.texture).is_some());
                }
            }
            assert!(colours > 0);
            if mode == "omit" {
                assert_eq!(normals, 0);
            }
            let source_materials = world.iter::<ShredMaterial>().count();
            let log = &report.steps[0].log;
            assert_eq!(log.counts["SHRED materials lowered"], source_materials as u64);
            assert!(!log.approximations.is_empty());
            assert!(!log.approximated_on.is_empty());
            eprintln!(
                "SHRED PBR {name} normals={mode}: {source_materials} materials; colour={colours} normal={normals} specular={specular} mask={masks}; original_pixels={} copy_graphs={} prepared_views={}; approximation_kinds={}",
                source.len(),
                graphs.len(),
                world.iter::<Texture>().count() - source.len(),
                log.approximations.len()
            );
        }
    }
}
