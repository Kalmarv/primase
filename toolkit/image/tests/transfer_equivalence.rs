use primase_image::{linear_to_srgb8, linear_to_srgb8_f32, srgb_to_linear, srgb_to_linear_f32};

// Independent pre-extraction formulas from the scene and map writers.
fn scene_decode(i: u8) -> f32 {
    let c = f64::from(i) / 255.0;
    (if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }) as f32
}
fn map_decode(i: u8) -> f32 {
    let c = f32::from(i) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn scene_encode(linear: f32) -> u8 {
    let c = f64::from(linear).clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}
fn map_encode(v: f32) -> u8 {
    let v = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    ((v * 255.0 + 0.5) as i32).clamp(0, 255) as u8
}
#[test]
fn all_byte_decodes_keep_each_consumers_precision_bits() {
    for i in 0..=255u8 {
        assert_eq!(
            (srgb_to_linear(f64::from(i) / 255.) as f32).to_bits(),
            scene_decode(i).to_bits(),
            "scene byte {i}"
        );
        assert_eq!(
            srgb_to_linear_f32(f32::from(i) / 255.).to_bits(),
            map_decode(i).to_bits(),
            "map byte {i}"
        );
    }
}
#[test]
fn encode_byte_boundaries_keep_both_quantization_policies() {
    let mut samples = vec![
        -1.,
        -0.,
        0.,
        0.0031308,
        1.,
        2.,
        f32::MIN,
        f32::MAX,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
    ];
    for i in 0..=255 {
        // Linear values bracketing every half-byte rounding boundary.
        let s = (f64::from(i) + 0.5) / 255.;
        let c = if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        };
        samples.push(c as f32);
        samples.push(scene_decode(i as u8));
        samples.push(map_decode(i as u8));
    }
    for value in samples {
        for v in [value.next_down(), value, value.next_up()] {
            assert_eq!(
                linear_to_srgb8(f64::from(v)),
                scene_encode(v),
                "scene input {:08x}",
                v.to_bits()
            );
            assert_eq!(
                linear_to_srgb8_f32(v),
                map_encode(v),
                "map input {:08x}",
                v.to_bits()
            );
        }
    }
}
