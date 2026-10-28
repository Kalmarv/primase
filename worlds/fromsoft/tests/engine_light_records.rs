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
use fromsoft_formats::btl::{Kind, Light as BtlLight};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Light, Transform, World};
use primase_fromsoft::sekiro::light_record;
use primase_fromsoft::sekiro::passes::{self, lights::spot_half_angles};
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
        cone_width: 0.,
        exposure_weight: 0.,
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

/// A BTL spot as its bank stores it: the full vertical angle `cone`, the horizontal one (record offset 184), the
/// rotation in radians.
fn btl_spot(position: [f32; 3], rotation: [f32; 3], cone: f32, width: f32) -> BtlLight {
    BtlLight {
        name: "live".into(),
        kind: Kind::Spot,
        diffuse: [200, 225, 255],
        diffuse_power: 0.5,
        specular: [200, 225, 255],
        specular_power: 1.,
        cast_shadows: false,
        cone_angle: cone,
        position,
        rotation,
        radius: 30.,
        flicker_interval: [0.; 2],
        flicker_brightness: 1.,
        event_id: -1,
        near_clip: 1.,
        shadow_model_cull_flip: false,
        sharpness: 1.,
        horizontal_angle: width,
        near_distance: 0.,
        enable_distance: 0.,
        volume_density: 0.,
        exposure_weight: 0.,
        auxiliary: Default::default(),
    }
}

/// Spot lights of Ashina Castle (`m11_01_00_00_0000` lights 2 and 3, `m11_02_00_00_0000` lights 0, 4 and 5), their
/// records read back from the spot buffer: the two scale rows of the `ViewToLightSpaceMatrix` (rows 0 and 1, the
/// horizontal and the vertical axis of the cone) have the lengths below, `cot` of half the stored angle of that
/// axis: the horizontal axis's is the stored word at offset 184, the vertical axis's the cone angle at offset 44.
/// (`cone`, `width`) -> (horizontal scale, vertical scale).
#[test]
fn btl_spot_angles_are_full_angles_per_axis() {
    let cases: [(f32, f32, f64, f64); 5] = [
        (30., 30., 3.732_050_3, 3.732_050_5),
        (8., 90., 1.000_000_2, 14.300_667),
        (30., 20., 5.671_281_9, 3.732_050_8),
        (50., 10., 11.430_053_4, 2.144_507_0),
        (20., 25., 4.510_708_8, 5.671_282_1),
    ];
    for (cone, width, horizontal, vertical) in cases {
        let [h, v] = spot_half_angles(cone, width).expect("a cone");
        for (half, scale, axis) in [(h, horizontal, "horizontal"), (v, vertical, "vertical")] {
            let got = 1. / half.tan();
            assert!(
                (got - scale).abs() <= 1e-6 * scale,
                "cone {cone} width {width} {axis}: cot(half angle) {got}, the game's scale {scale}"
            );
        }
    }
}

/// What the game's loader code does with a stored width of 0 (the width is 0 on most banks' lights; no live spot of the
/// readbacks has one): the horizontal axis takes the cone angle, and a spot with neither angle is switched off. Read from
/// the executable's code, not from a record.
#[test]
fn a_missing_spot_width_takes_the_cone_angle() {
    assert_eq!(spot_half_angles(30., 0.), spot_half_angles(30., 30.));
    assert_eq!(spot_half_angles(0., 20.), spot_half_angles(20., 20.));
    assert_eq!(spot_half_angles(0., 0.), None);
}

/// The same two lights' axes: the records' `forward` (row 3 of the matrix), `right` and `up` (rows 0 and 1) in the map
/// frame, from the stored rotation in radians. The world frame mirrors z, so a record's z component is negated: the
/// beam is the transform's local -Z, and its local X and Y are the cone's horizontal and vertical axes.
#[test]
fn btl_spot_axes_are_the_records() {
    let cases = [
        (
            btl_spot(
                [-196.624, -10.8612, 219.919],
                [1.220_838_2, -1.447_076_4, 0.],
                30.,
                30.,
            ),
            [-0.340_237_8, -0.939_387_1, 0.042_310_3],
            [0.123_404_5, 0., 0.992_356_4],
            [-0.932_206_8, 0.342_858_5, 0.115_924_6],
        ),
        (
            btl_spot(
                [-353.334, -22.1065, 235.501],
                [0.296_149_9, 0.727_452_3, 0.],
                8.,
                90.,
            ),
            [0.636_021, -0.291_839_9, 0.714_357_6],
            [0.746_871_1, 0., -0.664_969],
            [0.194_064_5, 0.956_467_2, 0.217_966_8],
        ),
    ];
    for (light, forward, right, up) in cases {
        let (t, _) = light_record(&light, &IDENTITY).unwrap();
        let mirrored = |v: [f32; 3]| [v[0], v[1], -v[2]];
        let beam = t.cols[2].map(|x| -x);
        for (got, want, what) in [
            (beam, mirrored(forward), "beam"),
            (t.cols[0], mirrored(right), "horizontal axis"),
            (t.cols[1], mirrored(up), "vertical axis"),
        ] {
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 2e-6,
                    "{what}: {got:?}, the game's record {want:?}"
                );
            }
        }
    }
}
