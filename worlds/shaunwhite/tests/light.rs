mod inputs;

use anvil_formats::shaunwhite::light::{self, OMNI_LIGHT, SKY_LIGHT, SPOT_LIGHT, SUN_LIGHT};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Ambient, AmbientLight, Environment, Light, LightKind, Linear, Transform, World};
use primase_shaunwhite::components::{SkySettings, Stored};

fn words(b: &mut Vec<u8>, values: &[u32]) {
    for n in values {
        b.extend(n.to_le_bytes());
    }
}
fn floats(b: &mut Vec<u8>, values: &[f32]) {
    for n in values {
        b.extend(n.to_le_bytes());
    }
}
/// Independently hand-laid AC2 field order plus the game's five flags, resolution and ranges.
fn fixture(class: u32) -> Vec<u8> {
    let mut b = vec![1];
    words(&mut b, &[42, class, 0]);
    b.push(1);
    words(&mut b, &[0, 0xA797_67ED]);
    floats(&mut b, &[0.5, 0.25, 0.125, 1.]);
    b.extend([1, 1, 1, 1, 0]);
    floats(&mut b, &[2.]);
    words(&mut b, &[43, 0xAA8F_96B6]);
    b.push(0);
    words(&mut b, &[0]);
    floats(&mut b, &[1., 1., 0.1, 0.1, 1.]);
    b.extend([0, 1]);
    floats(&mut b, &[0.2]);
    words(&mut b, &[1, 2]);
    if class == SPOT_LIGHT {
        floats(&mut b, &[0.2, 0.7]);
        words(&mut b, &[0, 0]);
    }
    if class != SUN_LIGHT {
        floats(&mut b, &[0., 30.]);
    }
    b
}
fn sky() -> Vec<u8> {
    let mut b = vec![1];
    words(&mut b, &[9, SKY_LIGHT]);
    floats(&mut b, &[0., 0., 1.]);
    words(&mut b, &[0, 0xF820_6AF7, 0xFF80_4020]);
    floats(&mut b, &[2., 0., 0.1]);
    b.push(0);
    words(&mut b, &[0, 0xF820_6AF7, 0xFFFF_FFFF]);
    floats(&mut b, &[1., 0., 0., 0.]);
    floats(&mut b, &[0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]);
    assert_eq!(b.len(), 106);
    b.extend(fixture(SUN_LIGHT));
    b
}

#[test]
fn explicit_light_units_and_axes_lower_through_the_manager() {
    let mut w = World::new();
    let point = w.spawn();
    let spot = w.spawn();
    let sun = w.spawn();
    for (e, class) in [(point, OMNI_LIGHT), (spot, SPOT_LIGHT)] {
        w.insert(e, Stored(light::read_light(&fixture(class)).unwrap()));
        w.insert(e, Transform::IDENTITY);
    }
    let (settings, l) = light::read_sky(&sky()).unwrap();
    w.insert(sun, Stored(settings));
    w.insert(sun, Stored(l));
    w.insert(sun, Transform::IDENTITY);
    let mut reg = PassRegistry::new();
    primase_shaunwhite::register(&mut reg);
    assert!(reg.parse("lower-shaunwhite-lighting").is_err());
    let pipeline=reg.parse("lower-shaunwhite-lighting{point-scale=100,sun-scale=2,spot-axis=y,sun-vector=to-sun,colour=linear}").unwrap();
    let manager = PassManager::new(reg).with_pass_checks(true);
    manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    assert_eq!(w.get::<Light>(point).unwrap().intensity, 200.);
    assert_eq!(w.get::<Light>(point).unwrap().range, Some(30.));
    assert_eq!(w.get::<Light>(sun).unwrap().intensity, 4.);
    assert_eq!(
        w.get::<Light>(spot).unwrap().kind,
        LightKind::Spot {
            inner_cone: 0.2,
            outer_cone: 0.7
        }
    );
    assert_eq!(
        Linear::of(w.get::<Transform>(spot).unwrap()).point([0., 0., -1.]),
        [0., 0., -1.]
    );
    assert_eq!(
        Linear::of(w.get::<Transform>(sun).unwrap()).point([0., 0., -1.]),
        [0., -1., 0.]
    );
    assert_eq!(w.resource::<Environment>().unwrap().sun, Some(sun));
    // the sky's ambient colour (0.2, 0.3, 0.4) in the sun's unit, which sun-scale states here
    assert_eq!(
        w.resource::<Environment>().unwrap().ambient,
        Some(Ambient {
            intensity: 2.,
            light: AmbientLight::Flat([0.2, 0.3, 0.4])
        })
    );
    assert!(w.has::<SkySettings>(sun));
}

/// `point-scale=sun:LUX`: the level's own sun fixes one unit for the local lights and the sun alike. The fixture's
/// sun is intensity 2 in colour (0.5, 0.25, 0.125), luminance 0.2126 x 0.5 + 0.7152 x 0.25 + 0.0722 x 0.125.
#[test]
fn the_sun_fixes_the_light_unit_when_asked() {
    let mut w = World::new();
    let point = w.spawn();
    w.insert(point, Stored(light::read_light(&fixture(OMNI_LIGHT)).unwrap()));
    w.insert(point, Transform::IDENTITY);
    let mut reg = PassRegistry::new();
    primase_shaunwhite::register(&mut reg);
    let text = "lower-shaunwhite-lighting{point-scale=sun:100000,sun-scale=7,spot-axis=y,sun-vector=to-sun,colour=linear}";
    let pipeline = reg.parse(text).unwrap();
    let manager = PassManager::new(reg).with_pass_checks(true);
    // a level without its sky has no sun to take the unit from
    assert!(
        manager
            .run(&mut w.clone(), &pipeline, &mut RunReport::default())
            .unwrap_err()
            .to_string()
            .contains("needs the level's SkyLight")
    );
    let sun = w.spawn();
    let (settings, l) = light::read_sky(&sky()).unwrap();
    w.insert(sun, Stored(settings));
    w.insert(sun, Stored(l));
    w.insert(sun, Transform::IDENTITY);
    manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    let luminance = 0.2126_f64 * 0.5 + 0.7152 * 0.25 + 0.0722 * 0.125;
    let unit = 100_000. / (2. * luminance);
    let near = |a: f32, b: f64| (f64::from(a) - b).abs() < b * 1e-5;
    // the sun is the stated lux in luminance, whatever sun-scale says; a light of intensity 2 is twice the unit
    assert!(near(w.get::<Light>(sun).unwrap().intensity, 2. * unit));
    assert!(near(w.get::<Light>(point).unwrap().intensity, 2. * unit));
    assert_eq!(w.get::<Light>(point).unwrap().range, Some(30.));
    // the ambient is in the same unit as the sun: its colour times the lux per stored unit
    let ambient = w.resource::<Environment>().unwrap().ambient.clone().unwrap();
    assert!(near(ambient.intensity, unit));
    assert_eq!(ambient.light, AmbientLight::Flat([0.2, 0.3, 0.4]));
}

/// The ambient follows the colour encoding of the lights, is reported (counted, approximated, and the bake named as
/// not carried), and does not replace an ambient the world already states.
#[test]
fn the_ambient_is_reported_and_follows_the_colour_encoding() {
    let mut w = World::new();
    let sun = w.spawn();
    let (settings, l) = light::read_sky(&sky()).unwrap();
    w.insert(sun, Stored(settings));
    w.insert(sun, Stored(l));
    w.insert(sun, Transform::IDENTITY);
    let mut reg = PassRegistry::new();
    primase_shaunwhite::register(&mut reg);
    let pipeline = reg
        .parse("lower-shaunwhite-lighting{point-scale=100,sun-scale=3,spot-axis=y,sun-vector=to-sun,colour=srgb}")
        .unwrap();
    let manager = PassManager::new(reg).with_pass_checks(true);
    let mut taken = w.clone();
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).unwrap();
    let linear = |v: f32| ((v + 0.055) / 1.055).powf(2.4);
    let ambient = w.resource::<Environment>().unwrap().ambient.clone().unwrap();
    assert_eq!(ambient.intensity, 3.);
    assert_eq!(
        ambient.light,
        AmbientLight::Flat([linear(0.2), linear(0.3), linear(0.4)])
    );
    let log = &report.steps[0].log;
    assert_eq!(log.counts["sky ambient colours lowered as core ambient"], 1);
    assert!(
        log.omitted.keys().any(|k| k.starts_with("baked lighting")),
        "{:?}",
        log.omitted
    );
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("sky ambient as one flat light")),
        "{:?}",
        log.approximations
    );
    taken.insert_resource(Environment {
        sun: None,
        ambient: Some(Ambient {
            intensity: 1.,
            light: AmbientLight::Flat([1.; 3]),
        }),
    });
    assert!(
        manager
            .run(&mut taken, &pipeline, &mut RunReport::default())
            .unwrap_err()
            .to_string()
            .contains("already states an ambient")
    );
}

#[test]
fn point_scale_is_a_number_or_sun_lux() {
    let mut reg = PassRegistry::new();
    primase_shaunwhite::register(&mut reg);
    let make = |scale: &str| {
        reg.parse(&format!(
            "lower-shaunwhite-lighting{{point-scale={scale},sun-scale=1,spot-axis=y,sun-vector=to-sun,colour=linear}}"
        ))
        .map(|_| ())
    };
    for good in ["100", "0.5", "sun:100000"] {
        assert!(make(good).is_ok(), "{good}");
    }
    for bad in ["sun:0", "sun:", "sun:-3", "sun:x", "-1", "nan", "sky:5"] {
        assert!(make(bad).is_err(), "{bad}");
    }
}

/// The game's levels: each sky's ambient colour is lowered in the sun's unit (the sun taken as 100,000 lux), and is
/// the lux the level's baked lighting leaves a dark surface (the colour times lux per stored unit).
#[test]
fn levels_state_their_sky_ambient_in_the_suns_unit() {
    use inputs::{Game, install_file};
    let files = [
        install_file(Game::ShaunWhite, "DataPC_01.forge"),
        install_file(Game::ShaunWhite, "DataPC_02.forge"),
        install_file(Game::ShaunWhite, "DataPC_03.forge"),
        install_file(Game::ShaunWhite, "DataPC_04.forge"),
        install_file(Game::ShaunWhite, "DataPC_05.forge"),
        install_file(Game::ShaunWhite, "DataPC_06.forge"),
    ];
    let Some(files) = files.into_iter().collect::<Option<Vec<_>>>() else {
        return;
    };
    // (grey level of the ambient colour in 1/255, the ambient's lux in thousands) per level
    let expected = [
        (70., 11.6),
        (70., 11.6),
        (70., 11.6),
        (90., 14.9),
        (70., 11.9),
        (50., 10.4),
    ];
    for (level, path) in files.iter().enumerate() {
        let shared: Vec<_> = files.iter().filter(|p| *p != path).cloned().collect();
        let (mut w, _) = primase_shaunwhite::read::level::read_level(path, &shared).unwrap();
        let mut reg = PassRegistry::new();
        primase_shaunwhite::register(&mut reg);
        let pipeline = reg
            .parse(
                "lower-shaunwhite-lighting{point-scale=sun:100000,sun-scale=1,spot-axis=y,sun-vector=to-sun,colour=linear}",
            )
            .unwrap();
        PassManager::new(reg)
            .with_pass_checks(true)
            .run(&mut w, &pipeline, &mut RunReport::default())
            .unwrap();
        let ambient = w.resource::<Environment>().unwrap().ambient.clone().unwrap();
        let AmbientLight::Flat(c) = ambient.light else {
            panic!("level {}: not flat", level + 1)
        };
        assert!(
            c.iter().all(|v| (v * 255. - expected[level].0).abs() < 0.01),
            "level {}: {c:?}",
            level + 1
        );
        let lux = f64::from(c[0] * ambient.intensity) / 1000.;
        assert!(
            (lux - expected[level].1).abs() < 0.06,
            "level {}: {lux} klux",
            level + 1
        );
    }
}
