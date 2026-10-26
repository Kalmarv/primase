use std::collections::BTreeMap;

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{MaterialUsage, Surface, SurfaceClass, Surfaces, World};
use primase_shaunwhite::components::{MaterialRef, MaterialState};

#[test]
fn stored_water_flag_lowers_without_matching_names_and_keeps_death_surfaces_distinct() {
    let mut w = World::new();
    let water = w.spawn();
    let dry = w.spawn();
    let state = MaterialState {
        blend: 0,
        blended: false,
        alpha_test: false,
        alpha_ref: 128,
        water: true,
        two_sided: false,
        invert_culling: false,
        color: [0.16, 0.24, 0.28, 1.],
        illumination: 0.0,
        specular_level: 1.0,
        specular_power: 2.0,
        normal_scale: 1.,
        layering: primase_shaunwhite::components::LayerState::default(),
    };
    for (e, name, state) in [
        (water, "renamed source material", state),
        (
            dry,
            "WaterPort",
            MaterialState {
                water: false,
                two_sided: false,
                invert_culling: false,
                ..state
            },
        ),
    ] {
        w.insert(
            e,
            MaterialRef {
                id: e.index(),
                name: name.into(),
                state: Some(state),
                slots: BTreeMap::new(),
                unread: Vec::new(),
            },
        );
    }
    w.insert_resource(Surfaces(
        ["Water", "Bail", "RobotBail", "ConcreteUnsafe", "InvisibleWall"]
            .into_iter()
            .map(Surface::new)
            .collect(),
    ));
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    assert!(registry.parse("lower-shaunwhite-water").is_err());
    assert!(registry.parse("lower-shaunwhite-water{roughness=NaN}").is_err());
    let pipeline = registry
        .parse("lower-shaunwhite-water{roughness=0.05},lower-shaunwhite-material")
        .unwrap();
    let manager = PassManager::new(registry).with_pass_checks(true);
    manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    assert_eq!(w.get::<MaterialUsage>(water), Some(&MaterialUsage::Water));
    assert!(w.get::<MaterialUsage>(dry).is_none());
    let wet = w.get::<PbrMaterial>(water).unwrap();
    assert_eq!(
        (wet.base_color, wet.roughness, wet.alpha_mode),
        (state.color, 0.05, AlphaMode::Opaque)
    );
    let surfaces = &w.resource::<Surfaces>().unwrap().0;
    assert_eq!(surfaces[0].class, Some(SurfaceClass::Water));
    assert!(surfaces[1..].iter().all(|s| s.class.is_none()));
    assert_eq!(surfaces[1].name, "Bail");
}
