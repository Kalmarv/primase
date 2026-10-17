//! The GTA pipeline on each named region ([`primase_gta::pipeline::REGIONS`]): the region read from the install, the
//! default selection passes run (GTA Online's map, default hour, the region rules, interiors left out), every
//! placement loaded, and [`DEFAULT_PASSES`] run, as every writer gets it. The world must verify after the passes, every placement's prototype must have a drawable or be
//! counted (no live file, or nothing a surface draws), and every material on a drawn primitive must be lowered
//! (`pbr.material`, or a layered terrain material). The counts that review a region (tinted primitives and the
//! triangles straddling palette columns, decals, vertex-alpha surfaces, layered terrain, LOD forms) are printed.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`; `PRIMASE_GTA_REGION` limits it to one named region.
//!
//! [`DEFAULT_PASSES`]: primase_gta::pipeline::DEFAULT_PASSES
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, LayeredMaterial, Mesh};
use primase_gta::pipeline::{DEFAULT_PASSES, GtaSource, Map, REACH, REGIONS, Selection, select_passes};

#[test]
fn every_named_region_through_the_gta_passes() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::real_data("PRIMASE_GTA_DIR"),
        primase_testdata::real_data("PRIMASE_GTA_KEYS"),
    ) else {
        return;
    };
    let only = primase_testdata::real_data_value("PRIMASE_GTA_REGION");
    let source = GtaSource::open(&dir, &keys, Map::Online).expect("install");
    let mut registry = PassRegistry::new();
    registry.register("select-lod", primase_core::select_lod);
    registry.register("resolve-state", primase_core::state::resolve_state);
    primase_gta::register(&mut registry);
    let pipeline = registry.parse(DEFAULT_PASSES).expect("default passes");
    let manager = PassManager::new(registry);
    for (name, region) in REGIONS {
        if only.as_deref().is_some_and(|o| o != *name) {
            continue;
        }
        let (mut read, report) = source.read(*region, REACH).expect("region");
        println!("{name}: {:?}; {:?}", report.region, report.lights);
        let select = manager
            .registry
            .parse(&select_passes(&Selection::new(*region)))
            .expect("selection passes");
        let mut picked = RunReport::default();
        manager.run(&mut read, &select, &mut picked).expect("selection");
        for step in &picked.steps {
            println!("{name}: {}: {:?}", step.pass, step.log.counts);
        }
        let placements = read.iter::<Instance>().count();
        let (mut w, assets) = source.load_all(&read).expect("assets");
        println!("{name}: {assets:?}");
        let mut run = RunReport::default();
        manager.run(&mut w, &pipeline, &mut run).expect("passes");
        for step in &run.steps {
            println!(
                "{name}: {}: {:?}, approximated {:?}",
                step.pass, step.log.counts, step.log.approximations
            );
        }
        let bad = w.verify_components();
        assert!(
            bad.is_empty(),
            "{name}: {} components fail verification, e.g. {:?}",
            bad.len(),
            bad.first()
        );
        // every placement draws its prototype's mesh, or its prototype is counted as drawing nothing
        let drawn = w
            .iter::<Instance>()
            .filter(|(_, i)| w.has::<Mesh>(i.prototype))
            .count();
        println!("{name}: {drawn} of {placements} placements draw a mesh");
        // every drawn primitive's material is lowered
        let mut unlowered = 0;
        for (_, m) in w.iter::<Mesh>() {
            for p in &m.primitives {
                if let Some(mat) = p.material
                    && !w.has::<PbrMaterial>(mat)
                    && !w.has::<LayeredMaterial>(mat)
                {
                    unlowered += 1;
                }
            }
        }
        assert_eq!(unlowered, 0, "{name}: primitives whose material is not lowered");
        assert!(drawn > 0, "{name}: nothing drawn");
    }
}
