//! Shaun White's lights laid out by hand: a point, a spot and the sky with its inline sun read to the end, and their
//! malformed variants refused.
use anvil_formats::shaunwhite::light::LightShape;
use anvil_formats::shaunwhite::light::{self, OMNI_LIGHT, SKY_LIGHT, SPOT_LIGHT, SUN_LIGHT};

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
