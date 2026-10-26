use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Environment, Light, LightKind, Linear, Transform, World};
use primase_shaunwhite::components::{LightShape, SkySettings};
use primase_shaunwhite::read::light::{self, OMNI_LIGHT, SKY_LIGHT, SPOT_LIGHT, SUN_LIGHT};

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
fn hand_worked_point_spot_and_sky_walk_to_the_end() {
    let p = light::read_light(&fixture(OMNI_LIGHT)).unwrap();
    assert_eq!(p.colour, [0.5, 0.25, 0.125, 1.]);
    assert_eq!(p.intensity, 2.);
    assert_eq!(p.shape, LightShape::Point { near: 0., far: 30. });
    assert_eq!(p.flare.values, [1., 1., 0.1, 0.1, 1.]);
    let s = light::read_light(&fixture(SPOT_LIGHT)).unwrap();
    assert_eq!(
        s.shape,
        LightShape::Spot {
            inner: 0.2,
            outer: 0.7,
            cookie: 0,
            unknown: 0,
            near: 0.,
            far: 30.
        }
    );
    let (settings, sun) = light::read_sky(&sky()).unwrap();
    assert_eq!(settings.ambient_colour, [0.2, 0.3, 0.4, 0.5]);
    assert_eq!(settings.unknown_colour, [0.6, 0.7, 0.8, 0.9]);
    assert_eq!(sun.class, SUN_LIGHT);
    for b in [fixture(OMNI_LIGHT), fixture(SPOT_LIGHT), sky()] {
        for n in 0..b.len() {
            let ok = if b.len() == 200 {
                light::read_sky(&b[..n]).is_ok()
            } else {
                light::read_light(&b[..n]).is_ok()
            };
            assert!(!ok, "prefix {n}");
        }
        let mut extra = b.clone();
        extra.push(0);
        assert!(if b.len() == 200 {
            light::read_sky(&extra).is_err()
        } else {
            light::read_light(&extra).is_err()
        });
    }
    let mut b = fixture(OMNI_LIGHT);
    b[38] = 2;
    assert!(light::read_light(&b).unwrap_err().to_string().contains("bool"));
    let mut b = fixture(OMNI_LIGHT);
    b[43..47].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(
        light::read_light(&b)
            .unwrap_err()
            .to_string()
            .contains("intensity")
    );
    let mut b = fixture(SPOT_LIGHT);
    b[94..98].copy_from_slice(&2.0f32.to_le_bytes());
    assert!(light::read_light(&b).unwrap_err().to_string().contains("angles"));
}

#[test]
fn explicit_light_units_and_axes_lower_through_the_manager() {
    let mut w = World::new();
    let point = w.spawn();
    let spot = w.spawn();
    let sun = w.spawn();
    for (e, class) in [(point, OMNI_LIGHT), (spot, SPOT_LIGHT)] {
        w.insert(e, light::read_light(&fixture(class)).unwrap());
        w.insert(e, Transform::IDENTITY);
    }
    let (settings, l) = light::read_sky(&sky()).unwrap();
    w.insert(sun, settings);
    w.insert(sun, l);
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
    assert!(w.resource::<Environment>().unwrap().ambient.is_none());
    assert!(w.has::<SkySettings>(sun));
}

/// `point-scale=sun:LUX`: the level's own sun fixes one unit for the local lights and the sun alike. The fixture's
/// sun is intensity 2 in colour (0.5, 0.25, 0.125), luminance 0.2126 x 0.5 + 0.7152 x 0.25 + 0.0722 x 0.125.
#[test]
fn the_sun_fixes_the_light_unit_when_asked() {
    let mut w = World::new();
    let point = w.spawn();
    w.insert(point, light::read_light(&fixture(OMNI_LIGHT)).unwrap());
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
    w.insert(sun, settings);
    w.insert(sun, l);
    w.insert(sun, Transform::IDENTITY);
    manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    let luminance = 0.2126_f64 * 0.5 + 0.7152 * 0.25 + 0.0722 * 0.125;
    let unit = 100_000. / (2. * luminance);
    let near = |a: f32, b: f64| (f64::from(a) - b).abs() < b * 1e-5;
    // the sun is the stated lux in luminance, whatever sun-scale says; a light of intensity 2 is twice the unit
    assert!(near(w.get::<Light>(sun).unwrap().intensity, 2. * unit));
    assert!(near(w.get::<Light>(point).unwrap().intensity, 2. * unit));
    assert_eq!(w.get::<Light>(point).unwrap().range, Some(30.));
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
