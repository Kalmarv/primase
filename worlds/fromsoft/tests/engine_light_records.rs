//! The light records of a running game against the lowering. Sekiro 1.06 (Steam build 5794815), offline, in Ashina Castle
//! (`m11_01_00_00`): the structured buffer the tile-based lighting pass reads (`g_LightBuffer`, 208-byte `PointLight`
//! records: `PositionAndRadius`, `DiffColor`, `SpecColor`, ...) was read back from a frame, and each record that stands
//! on a BTL light of the map's banks (view-space position moved to the map frame by the frame's inverse view matrix,
//! within 0.01 m) or on the one steady effect light was compared with that light's stored values. The expected values
//! below are the records' `DiffColor` red, green and blue as the game uploaded them (float bit patterns printed in full).
//!
//! The lowering of a stored light is a `Light` whose colour times intensity is the radiance the game's shader receives,
//! at a `candela-per-power` of 1: `colour^2 * power`, the same for a BTL light's bytes and power and an effect light's
//! stored colour and `alpha * diffuse multiplier`. A relative difference above 1e-6 fails.
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Light, Transform, World};
use primase_fromsoft::sekiro::passes;
use primase_fromsoft::sekiro::source::{
    EffectLight, EffectPlace, EffectShape, EffectValue, LightKind, LightSource,
};

const IDENTITY: Transform = Transform {
    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
};

/// A BTL record of the given bytes, power and radius.
fn btl(diffuse: [u8; 3], power: f32, radius: f32) -> LightSource {
    LightSource {
        name: "live".into(),
        kind: LightKind::Point,
        diffuse,
        diffuse_power: power,
        specular: [0; 3],
        specular_power: 0.,
        cast_shadows: false,
        cone_angle: 0.,
        radius,
        tangent: [1., 0., 0.],
    }
}

fn constant(values: &[f32]) -> EffectValue {
    EffectValue {
        initial: values.to_vec(),
        settled: Some(values.to_vec()),
        keyframes: 0,
        modifiers: 0,
    }
}

fn lower(world: &mut World) -> Vec<Light> {
    let mut registry = PassRegistry::new();
    passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            world,
            &manager
                .registry
                .parse("lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15}")
                .expect("pipeline"),
            &mut report,
        )
        .expect("lowering");
    world.iter::<Light>().map(|(_, l)| l.clone()).collect()
}

fn assert_record(light: &Light, record: [f32; 3], what: &str) {
    for (i, (colour, record)) in light.color.iter().zip(record).enumerate() {
        let got = f64::from(*colour) * f64::from(light.intensity);
        let want = f64::from(record);
        assert!(
            (got - want).abs() <= 1e-6 * want.max(1e-9),
            "{what} channel {i}: lowered {got}, the game's record {want}"
        );
    }
}

/// BTL banks `m11_01_00_00_0000` lights 18, 23, 8, 14 and 15: the bank's bytes and power, the record's `DiffColor`.
/// `PositionAndRadius.w` of each is `1 / radius^2` and `DiffColor.w` is `1 / (the record's float at +164)`: not lowered.
#[test]
fn btl_lights_lower_to_the_records_the_game_uploaded() {
    let cases: [([u8; 3], f32, f32, [f32; 3]); 5] = [
        ([255, 158, 9], 0.75, 12., [0.75, 0.287_935_44, 0.000_934_256_2]),
        ([200, 225, 255], 0.1, 6., [0.061_514_806, 0.077_854_68, 0.1]),
        ([255, 185, 147], 0.75, 8., [0.75, 0.394_752_05, 0.249_238_77]),
        ([159, 240, 255], 1.0, 7., [0.388_788_97, 0.885_813_2, 1.0]),
        ([255, 158, 9], 1.0, 4., [1.0, 0.383_913_9, 0.001_245_674_9]),
    ];
    for (bytes, power, radius, record) in cases {
        let mut world = World::default();
        let e = world.spawn();
        world.insert(e, IDENTITY);
        world.insert(e, btl(bytes, power, radius));
        let lights = lower(&mut world);
        let [light] = &lights[..] else { panic!() };
        assert_record(light, record, &format!("{bytes:?} x {power}"));
        assert_eq!(light.range, Some(radius));
    }
}

/// Effect 300 (`f000000300.fxr` of `sfxbnd_commoneffects`): a steady point light, no flicker, colour
/// (200, 230, 255) / 255, alpha 1, diffuse multiplier 0.05, radius 1.5; the record read next to the player at
/// (-202.83, -25.02, 176.76) has `DiffColor` (0.030757403, 0.040676665, 0.05) and `PositionAndRadius.w` = 1 / 1.5^2.
/// Its multiplier is a BTL power: the same lowering as a BTL light, with no unit between them.
#[test]
fn a_steady_effect_light_lowers_to_the_record_the_game_uploaded() {
    let mut world = World::default();
    let e = world.spawn();
    world.insert(e, IDENTITY);
    let colour = [0.784_313_74, 0.901_960_8, 1., 1.];
    world.insert(
        e,
        EffectLight {
            effect: 300,
            place: EffectPlace::Region { region: "R".into() },
            shape: EffectShape::Point {
                radius: constant(&[1.5]),
            },
            diffuse: constant(&colour),
            diffuse_multiplier: Some(constant(&[0.05])),
            specular: constant(&colour),
            specular_multiplier: Some(constant(&[0.05])),
            separate_specular: true,
            shadows: false,
            flickers: false,
            fade_out_frames: 30,
            starts_disabled: false,
            chosen_in_state_0: true,
            static_transforms: 1,
            via: Vec::new(),
        },
    );
    let lights = lower(&mut world);
    let [light] = &lights[..] else { panic!() };
    assert_record(light, [0.030_757_403, 0.040_676_665, 0.05], "effect 300");
    assert_eq!(light.range, Some(1.5));
}
