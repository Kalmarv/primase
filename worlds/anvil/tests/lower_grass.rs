//! `lower-anvil-grass` on a hand-made world: one terrain node of 64 m (level 8 of the 16,384 m square) with a flat ground
//! at height 7 and every sample in group 0, whose one rule puts one model (a stand-in prototype) at probability 1 at
//! each sample with a scale in 0.5 to 1.5. The plants lie in the rectangle on the ground, within the rule's scale, the
//! same options give the same plants and another seed others, `max-plants` is refused not truncated, and the setup, the
//! models' placements and the rasters are consumed.
use primase_anvil::components::{
    AnvilGrass, AnvilGrassModel, AnvilTerrain, AnvilTerrainGroups, GroupTile, TerrainTile,
};
use primase_anvil::passes::{CellPoint, Density, LowerAnvilGrass};
use primase_anvil::read::entity::{GrassSetup, MeshInstance};
use primase_anvil::read::grass::{
    GrassModelSettings, StoredGrassGroup, StoredGrassLibrary, StoredGrassModel, StoredGrassRule,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, Name, Transform, World};

/// The terrain node's row and column: 64 m nodes from -8192, so its corner is (-3648, 1088).
const ROW: u32 = 145;
const COLUMN: u32 = 71;

fn library() -> StoredGrassLibrary {
    let settings = GrassModelSettings {
        z_offset: 0.0,
        height_scale_for_occlusion: 0.9,
        terrain_normal_follower_ratio: 1.0,
        root_height: 0.0,
        softness: 0.5,
        normal_bend_factor: 10.0,
        visible_virtual_lod_count: 4,
        texture_atlas_element_count: 1,
        raycast_translucency: 1,
        imposter_shader_start_model_lod_index: [2, 3, 4],
        imposter_lookat_ratio: 1.0,
        unnamed: [0; 23],
    };
    let rule = StoredGrassRule {
        id: 4,
        model: 0,
        tiling_log_count: 0,
        density: 1.0,
        tilt_factor: 0.0,
        scaling_enabled: true,
        scale_a: 1.5,
        scale_b: 0.5,
        height_scaling_enabled: false,
        height_scale_a: 1.0,
        height_scale_b: 1.0,
        unnamed_f32: 0.0,
        unnamed: [0; 49],
    };
    StoredGrassLibrary {
        models: vec![StoredGrassModel {
            id: 1,
            settings,
            bounds_id: 2,
            bounds: [0.0; 6],
            bounds_rest: [0; 24],
        }],
        groups: vec![StoredGrassGroup {
            id: 3,
            rules: vec![rule],
        }],
        table: [0xFF; 256],
        rest: Vec::new(),
    }
}

fn setup() -> GrassSetup {
    let bounds = primase_anvil::read::entity::BoundingVolume {
        id: 0,
        min: [0.0; 3],
        max: [0.0; 3],
        word: 0,
    };
    GrassSetup {
        head: [0; 4],
        object: None,
        id: 0,
        instance: MeshInstance {
            id: 0,
            instances: Vec::new(),
            bounds,
            reserved: [0; 16],
            compiled: [0; 17],
            material_infos: Vec::new(),
            instance_bounds: Vec::new(),
        },
        instance_extra: Vec::new(),
        tail: Vec::new(),
        library: None,
        settings_id: 0,
        settings: Vec::new(),
        entities: Vec::new(),
    }
}

/// The world: the setup, one model placement of a prototype, and the terrain with its group and weight rasters.
fn world() -> (World, Entity, Entity) {
    let mut w = World::new();
    let g = w.spawn();
    w.insert(
        g,
        AnvilGrass {
            setup: setup(),
            library: library(),
        },
    );
    let prototype = w.spawn();
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype,
            variant: 0,
        },
    );
    w.insert(placement, Name("GrassEntity_Model 0".to_owned()));
    w.insert(
        placement,
        AnvilGrassModel {
            model: 0,
            ranges: vec![0, 1, 2],
        },
    );
    let terrain = w.spawn();
    // a value of 2^28 / 2^(8 - 28) ... the height unit is range x 2^(level - 28): range 1 gives 2^-20 a value
    let value = 7 << 20;
    w.insert(
        terrain,
        AnvilTerrain {
            frame: SourceFrame::WORLD,
            level: 8,
            origin: [-8192.0, -8192.0],
            side: 16_384.0,
            tiles: vec![TerrainTile {
                row: ROW,
                column: COLUMN,
                heights: vec![value; 132 * 132],
                colour: Vec::new(),
                material: Vec::new(),
                block_8: Vec::new(),
            }],
            materials: Vec::new(),
            details: Vec::new(),
            noise: None,
        },
    );
    w.insert(
        terrain,
        AnvilTerrainGroups {
            frame: SourceFrame::WORLD,
            level: 8,
            origin: [-8192.0, -8192.0],
            side: 16_384.0,
            tiles: vec![GroupTile {
                row: ROW,
                column: COLUMN,
                group: vec![0; 132 * 132],
                weight: vec![96; 132 * 132],
            }],
        },
    );
    (w, g, placement)
}

fn options(seed: u64, max: u64) -> Options {
    Options::from(
        [
            ("rect", "-3640,1096,-3632,1104"),
            ("range", "1"),
            ("offset", "0"),
            ("sample", "corner"),
            ("seed", &seed.to_string()),
            ("density", "per-sample"),
            ("tiling", "one"),
            ("scale", "rule"),
            ("height-scale", "none"),
            ("tilt", "none"),
            ("normal", "terrain"),
            ("max-plants", &max.to_string()),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    )
}

fn run(seed: u64, max: u64) -> Result<(World, PassLog, Entity, Entity), String> {
    let (mut w, setup, placement) = world();
    let pass = LowerAnvilGrass::from_options(&options(seed, max))?;
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log)?;
    Ok((w, log, setup, placement))
}

fn plants(w: &World) -> Vec<[f32; 3]> {
    let mut at: Vec<[f32; 3]> = w
        .iter::<Instance>()
        .filter_map(|(e, _)| w.get::<Transform>(e).map(|t| t.cols[3]))
        .collect();
    at.sort_by(|a, b| a.partial_cmp(b).expect("finite positions"));
    at
}

#[test]
fn the_plants_stand_on_the_ground_inside_the_rectangle_and_the_inputs_are_consumed() {
    let (w, log, setup, placement) = run(1, 100_000).expect("pass");
    let at = plants(&w);
    // one plant a sample whose plant lies in the 8 m square: about its 16 x 16 samples
    assert!((200..=300).contains(&at.len()), "{} plants", at.len());
    for p in &at {
        assert!((-3640.0..-3632.0).contains(&f64::from(p[0])), "x {p:?}");
        assert!((1096.0..1104.0).contains(&f64::from(p[1])), "y {p:?}");
        assert!((p[2] - 7.0).abs() < 1e-3, "z {p:?}");
    }
    for (e, _) in w.iter::<Instance>() {
        let t = w.get::<Transform>(e).expect("a transform");
        let s = t.cols[0].iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((0.5..=1.5).contains(&s), "scale {s}");
        // flat ground: the up stays the source's
        assert!((t.cols[2][2] / s - 1.0).abs() < 1e-4);
        assert_eq!(
            w.get::<Name>(e).map(|n| n.0.as_str()),
            Some("GrassEntity_Model 0")
        );
    }
    assert!(!w.is_alive(setup) && !w.is_alive(placement));
    assert!(w.iter::<AnvilGrass>().next().is_none());
    assert!(w.iter::<AnvilGrassModel>().next().is_none());
    assert!(w.iter::<AnvilTerrainGroups>().next().is_none());
    assert!(w.iter::<AnvilTerrain>().next().is_some());
    assert_eq!(
        log.counts["grass plants placed by a stand-in for the engine's run-time scatter"],
        at.len() as u64
    );
}

#[test]
fn the_same_options_give_the_same_plants_and_another_seed_others() {
    let a = plants(&run(1, 100_000).expect("pass").0);
    let b = plants(&run(1, 100_000).expect("pass").0);
    let c = plants(&run(2, 100_000).expect("pass").0);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn a_ceiling_is_refused_not_truncated() {
    let error = run(1, 10).map(|_| ()).expect_err("over the ceiling");
    assert!(error.contains("max-plants 10"), "{error}");
}

#[test]
fn every_option_is_required() {
    let full = options(1, 100);
    for key in full.keys() {
        let mut some = full.clone();
        some.remove(key);
        let error = LowerAnvilGrass::from_options(&some).map(|_| ()).expect_err(key);
        assert!(error.contains(key.as_str()), "{key}: {error}");
    }
    let pass = LowerAnvilGrass::from_options(&full).expect("options");
    assert_eq!(pass.options(), full);
    assert_eq!(
        (pass.sample, pass.density),
        (CellPoint::Corner, Density::PerSample)
    );
    assert!(pass.approximates().starts_with("grass placed by a stand-in"));
}

/// A rule whose `TilingLogCount` makes more candidates than the ceiling allows is refused naming the rule, not run
/// with a candidate: 4^20 over a ceiling of 100.
#[test]
fn a_tiling_count_over_the_ceiling_is_refused_naming_the_rule() {
    let (mut w, _, _) = world();
    let mut lib = w
        .iter::<AnvilGrass>()
        .next()
        .map(|(_, g)| g.library.clone())
        .expect("library");
    lib.groups[0].rules[0].tiling_log_count = 20;
    let (e, _) = w
        .iter::<AnvilGrass>()
        .map(|(e, g)| (e, g.setup.clone()))
        .next()
        .expect("setup");
    let setup = w.get::<AnvilGrass>(e).map(|g| g.setup.clone()).expect("setup");
    w.insert(e, AnvilGrass { setup, library: lib });
    let mut o = options(1, 100);
    o.insert("tiling".to_owned(), "rule".to_owned());
    let pass = LowerAnvilGrass::from_options(&o).expect("options");
    let error = pass
        .run(&mut w, &mut PassLog::default())
        .expect_err("over the ceiling");
    assert!(
        error.contains("TilingLogCount 20") && error.contains("0x4"),
        "{error}"
    );
}
