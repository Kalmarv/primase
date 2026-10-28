//! `lower-fromsoft-lights` on effect lights (`fromsoft.effect_light`) made by hand: the colour (squared, as the game's
//! shader takes it), the brightness (alpha times the stored multiplier, in BTL power, the settled or the first
//! keyframe), the range and cone, the lights left out by reason, and the options. The readings of a running game that
//! fix the unit are in `engine_light_records.rs`; the reader and the real-data counts are in `effect_lights_census.rs`.
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Light, LightKind, Transform, World};
use primase_fromsoft::sekiro::passes;
use primase_fromsoft::sekiro::source::{EffectLight, EffectPlace, EffectShape, EffectValue, LightSource};

const IDENTITY: Transform = Transform {
    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
};

fn constant(values: &[f32]) -> EffectValue {
    EffectValue {
        initial: values.to_vec(),
        settled: Some(values.to_vec()),
        keyframes: 0,
        modifiers: 0,
    }
}
/// A fade-in: 0 at the first keyframe, `to` where it settles.
fn fade_in(to: f32) -> EffectValue {
    EffectValue {
        initial: vec![0.],
        settled: Some(vec![to]),
        keyframes: 2,
        modifiers: 0,
    }
}

fn point(colour: [f32; 4], multiplier: EffectValue, radius: f32) -> EffectLight {
    EffectLight {
        effect: 800030,
        place: EffectPlace::Region { region: "R".into() },
        shape: EffectShape::Point {
            radius: constant(&[radius]),
        },
        diffuse: constant(&colour),
        diffuse_multiplier: Some(multiplier),
        specular: constant(&colour),
        specular_multiplier: None,
        separate_specular: false,
        shadows: true,
        flickers: false,
        fade_out_frames: 6,
        starts_disabled: false,
        chosen_in_state_0: true,
        static_transforms: 1,
        via: Vec::new(),
    }
}

fn spot(multiplier: f32, far: f32, rx: f32, ry: f32) -> EffectLight {
    EffectLight {
        shape: EffectShape::Spot {
            near: constant(&[0.01]),
            far: constant(&[far]),
            radius_x: constant(&[rx]),
            radius_y: constant(&[ry]),
        },
        diffuse_multiplier: Some(constant(&[multiplier])),
        ..point([1.; 4], constant(&[1.]), 1.)
    }
}

fn lowered(lights: Vec<EffectLight>, pipeline: &str) -> Result<(World, RunReport), String> {
    let mut world = World::default();
    for l in lights {
        let e = world.spawn();
        world.insert(e, IDENTITY);
        world.insert(e, l);
    }
    let mut registry = PassRegistry::new();
    passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            &mut world,
            &manager.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok((world, report))
}

const BASE: &str = "candela-per-power=2,spot-blend=0.25";

fn lights(world: &World) -> Vec<Light> {
    world.iter::<Light>().map(|(_, l)| l.clone()).collect()
}

#[test]
fn an_effect_light_is_lowered_in_btl_power() {
    // colour 128/255 squared, alpha 0.5, multiplier 6.4 settled (a fade-in from 0): 0.5 * 6.4 * 2
    let l = point([1., 128. / 255., 0., 0.5], fade_in(6.4), 3.);
    let (world, report) = lowered(
        vec![l],
        &format!("lower-fromsoft-lights{{{BASE},brightness=settled}}"),
    )
    .unwrap();
    let all = lights(&world);
    let [light] = &all[..] else { panic!() };
    assert_eq!(light.kind, LightKind::Point);
    assert!(
        (f64::from(light.intensity) - 0.5 * 6.4 * 2.).abs() < 1e-5,
        "{}",
        light.intensity
    );
    assert_eq!(light.range, Some(3.));
    assert_eq!(light.cast_shadows, Some(true));
    assert_eq!(light.color[0], 1.);
    assert_eq!(light.color[2], 0.);
    // the square, as the BTL lights' bytes
    assert!((light.color[1] - (128. / 255. * 128. / 255.)).abs() < 1e-7);
    // the record stays
    assert_eq!(world.iter::<EffectLight>().count(), 1);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["effect lights lowered"], 1);
    assert_eq!(
        log.approximations["effect light colour squared, the game's transfer"],
        1
    );
}

#[test]
fn the_first_keyframe_is_lowered_with_brightness_initial_and_a_dark_light_is_left_out() {
    let fading = point([1.; 4], fade_in(6.4), 3.);
    let flash = point(
        [1.; 4],
        EffectValue {
            initial: vec![8.],
            settled: Some(vec![0.]),
            keyframes: 2,
            modifiers: 0,
        },
        3.,
    );
    let pipeline = |extra: &str| format!("lower-fromsoft-lights{{{BASE}{extra}}}");
    // settled: the fade-in is lit, the flash is dark
    let (world, report) = lowered(vec![fading.clone(), flash.clone()], &pipeline("")).unwrap();
    assert_eq!(lights(&world).len(), 1);
    assert_eq!(
        report.steps[0].log.counts["effect lights dark at the lowered value, left out"],
        1
    );
    assert!((f64::from(lights(&world)[0].intensity) - 6.4 * 2.).abs() < 1e-5);
    // initial: the other way round
    let (world, report) = lowered(vec![fading, flash], &pipeline(",brightness=initial")).unwrap();
    assert_eq!(lights(&world).len(), 1);
    assert!((f64::from(lights(&world)[0].intensity) - 8. * 2.).abs() < 1e-5);
    assert_eq!(
        report.steps[0].log.counts["effect lights dark at the lowered value, left out"],
        1
    );
}

#[test]
fn a_looping_property_is_lowered_at_its_first_keyframe_and_counted() {
    let looping = point(
        [1.; 4],
        EffectValue {
            initial: vec![2.],
            settled: None,
            keyframes: 3,
            modifiers: 0,
        },
        3.,
    );
    let (world, report) = lowered(
        vec![looping],
        &format!("lower-fromsoft-lights{{{BASE},brightness=settled}}"),
    )
    .unwrap();
    assert!((f64::from(lights(&world)[0].intensity) - 2. * 2.).abs() < 1e-5);
    assert_eq!(
        report.steps[0].log.counts["looping colour or multiplier properties, lowered at their first keyframe"],
        1
    );
}

#[test]
fn a_spot_takes_its_multiplier_as_stored_and_the_wider_half_angle() {
    // far 10 m, base half-extents 10 and 5 m: half-angles 45 and 26.57 degrees; the wider is lowered
    let (world, report) = lowered(
        vec![spot(40., 10., 10., 5.)],
        &format!("lower-fromsoft-lights{{{BASE},brightness=settled}}"),
    )
    .unwrap();
    let all = lights(&world);
    let [light] = &all[..] else { panic!() };
    let outer = std::f32::consts::FRAC_PI_4;
    let LightKind::Spot {
        inner_cone,
        outer_cone,
    } = light.kind
    else {
        panic!("{:?}", light.kind)
    };
    assert!((outer_cone - outer).abs() < 1e-6, "{outer_cone}");
    assert!((inner_cone - outer * 0.75).abs() < 1e-6);
    assert!((f64::from(light.intensity) - 40. * 2.).abs() < 1e-5);
    assert_eq!(light.range, Some(10.));
    let log = &report.steps[0].log;
    assert_eq!(
        log.approximations["elliptic spot cones taken at their wider half-angle"],
        1
    );
    assert_eq!(
        log.approximations["effect spot axis along the effect's local +Z"],
        1
    );
    assert_eq!(
        log.approximations["effect spot multiplier as stored, no tenth (the game's code; not checked on a live spot)"],
        1
    );
}

#[test]
fn disabled_and_unchosen_lights_are_left_out_by_reason_unless_included() {
    let mut off = point([1.; 4], constant(&[1.]), 2.);
    off.starts_disabled = true;
    let mut unchosen = point([1.; 4], constant(&[1.]), 2.);
    unchosen.chosen_in_state_0 = false;
    let on = point([1.; 4], constant(&[1.]), 2.);
    let (world, report) = lowered(
        vec![off.clone(), unchosen.clone(), on.clone()],
        &format!("lower-fromsoft-lights{{{BASE},brightness=settled}}"),
    )
    .unwrap();
    assert_eq!(lights(&world).len(), 1);
    let log = &report.steps[0].log;
    assert_eq!(
        log.counts["effect lights of regions that start disabled, left out"],
        1
    );
    assert_eq!(
        log.counts["effect lights of a config the effect's first state does not choose, left out"],
        1
    );
    let (world, _) = lowered(
        vec![off, unchosen, on],
        &format!("lower-fromsoft-lights{{{BASE},include-disabled=on}}"),
    )
    .unwrap();
    assert_eq!(lights(&world).len(), 3);
}

#[test]
fn an_effect_light_and_a_btl_light_of_one_brightness_are_one_light() {
    // the game's shader takes a BTL light's bytes^2 * power and an effect light's colour^2 * alpha * multiplier as one
    // field: the same colour and the same number lower to the same light at any candela-per-power
    let mut world = World::default();
    let btl = world.spawn();
    world.insert(btl, IDENTITY);
    world.insert(
        btl,
        LightSource {
            name: "b".into(),
            kind: primase_fromsoft::sekiro::source::LightKind::Point,
            diffuse: [255, 128, 0],
            diffuse_power: 2.5,
            specular: [0; 3],
            specular_power: 0.,
            cast_shadows: false,
            cone_angle: 0.,
            radius: 4.,
            tangent: [1., 0., 0.],
        },
    );
    let e = world.spawn();
    world.insert(e, IDENTITY);
    world.insert(e, point([1., 128. / 255., 0., 1.], constant(&[2.5]), 4.));
    let mut registry = PassRegistry::new();
    passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            &mut world,
            &manager
                .registry
                .parse(&format!("lower-fromsoft-lights{{{BASE}}}"))
                .unwrap(),
            &mut report,
        )
        .unwrap();
    let all = lights(&world);
    let [a, b] = &all[..] else { panic!("{}", all.len()) };
    // the effect's colour is stored as an f32 (128/255 rounded), the BTL's as a byte: equal to f32 rounding
    for i in 0..3 {
        assert!(
            (a.color[i] - b.color[i]).abs() < 1e-7,
            "{:?} {:?}",
            a.color,
            b.color
        );
    }
    assert_eq!(a.intensity, b.intensity);
    assert_eq!(a.range, b.range);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["lights lowered"], 1);
    assert_eq!(log.counts["effect lights lowered"], 1);
}

#[test]
fn the_effect_options_refuse_bad_values() {
    let one = || vec![point([1.; 4], constant(&[1.]), 2.)];
    for pipeline in [
        format!("lower-fromsoft-lights{{{BASE},brightness=middle}}"),
        format!("lower-fromsoft-lights{{{BASE},include-disabled=maybe}}"),
        format!("lower-fromsoft-lights{{{BASE},effect-power-per-unit=1}}"),
    ] {
        assert!(lowered(one(), &pipeline).is_err(), "{pipeline}");
    }
    // the effect options alone are complete pipeline steps, and so are the two required options
    for pipeline in [
        format!("lower-fromsoft-lights{{{BASE}}}"),
        format!("lower-fromsoft-lights{{{BASE},brightness=initial}}"),
        format!("lower-fromsoft-lights{{{BASE},include-disabled=on}}"),
    ] {
        assert!(lowered(one(), &pipeline).is_ok(), "{pipeline}");
    }
}
