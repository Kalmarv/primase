use primase_image::{linear_to_srgb8, linear_to_srgb8_f32, srgb_to_linear, srgb_to_linear_f32};

/// The exact curve leaves the f32 writer, which rounds every intermediate, at most 6 ULP away over all 256 bytes;
/// the f64 writer rounds once and stays within 1. Bounds cover libm's last-bit variation (host `powf` is not
/// correctly rounded everywhere).
const F32_STANDARD_ULPS: u32 = 8;
const F64_STANDARD_ULPS: u32 = 1;

/// IEC 61966-2-1 decoded by hand for bytes around the 0.04045 knee and across the range. `c = byte / 255`;
/// `linear = c / 12.92` below the knee, else `((c + 0.055) / 1.055)^2.4`. The expected value is the exact result
/// rounded once to f32.
const STANDARD_DECODES: &[(u8, f32)] = &[
    // 0: c = 0; 0 / 12.92 = 0
    (0, 0.0),
    // 1: c = 0.003921569 (below the knee); 0.003921569 / 12.92 = 0.0003035270
    (1, 0.000303527),
    // 10: c = 0.039215686 (below the knee); 0.039215686 / 12.92 = 0.0030352698
    (10, 0.00303527),
    // 11: c = 0.043137255 (above the knee); ((0.043137255 + 0.055) / 1.055)^2.4 = 0.0033465358
    (11, 0.0033465358),
    // 12: c = 0.047058824; ((0.047058824 + 0.055) / 1.055)^2.4 = 0.0036765073
    (12, 0.0036765074),
    // 20: c = 0.078431373; ((c + 0.055) / 1.055)^2.4 = 0.0069954102
    (20, 0.00699541),
    // 64: c = 0.250980392; ((c + 0.055) / 1.055)^2.4 = 0.0512694584
    (64, 0.051269457),
    // 100: c = 0.392156863; ((c + 0.055) / 1.055)^2.4 = 0.1274376804
    (100, 0.12743768),
    // 128: c = 0.501960784; ((c + 0.055) / 1.055)^2.4 = 0.2158605001
    (128, 0.2158605),
    // 192: c = 0.752941176; ((c + 0.055) / 1.055)^2.4 = 0.5271151257
    (192, 0.5271151),
    // 200: c = 0.784313725; ((c + 0.055) / 1.055)^2.4 = 0.5775804404
    (200, 0.57758045),
    // 255: c = 1; ((1 + 0.055) / 1.055)^2.4 = 1
    (255, 1.0),
];

/// The same curve encoded by hand for f32 inputs: `linear * 12.92` below 0.0031308, else
/// `1.055 * linear^(1/2.4) - 0.055`; times 255, rounded to the nearest byte. Expected bytes are the two writers'
/// quantized results (the f32 rounding is far from every half-byte boundary here).
const STANDARD_ENCODES: &[(f32, u8)] = &[
    // 0: 12.92 * 0 = 0; 0 * 255 = 0
    (0.0, 0),
    // 0.001: 12.92 * 0.001 = 0.012920001; * 255 = 3.2946
    (0.001, 3),
    // 0.0031307 (below the knee): 12.92 * c = 0.040448644; * 255 = 10.3144
    (0.0031307, 10),
    // 0.0031309 (above the knee): 1.055 * c^(1/2.4) - 0.055 = 0.040451177; * 255 = 10.3151
    (0.0031309, 10),
    // 0.01: 1.055 * 0.01^(1/2.4) - 0.055 = 0.099852821; * 255 = 25.4625
    (0.01, 25),
    // 0.1: 1.055 * 0.1^(1/2.4) - 0.055 = 0.349190215; * 255 = 89.0435
    (0.1, 89),
    // 0.25: 1.055 * 0.25^(1/2.4) - 0.055 = 0.537098730; * 255 = 136.9602
    (0.25, 137),
    // 0.5: 1.055 * 0.5^(1/2.4) - 0.055 = 0.735356983; * 255 = 187.5160
    (0.5, 188),
    // 0.75: 1.055 * 0.75^(1/2.4) - 0.055 = 0.880825021; * 255 = 224.6104
    (0.75, 225),
    // 1: 1.055 * 1 - 0.055 = 1; * 255 = 255
    (1.0, 255),
];

#[test]
fn byte_transfers_match_hand_worked_iec_61966_2_1_values() {
    for &(i, expected) in STANDARD_DECODES {
        let scene = srgb_to_linear(f64::from(i) / 255.) as f32;
        assert!(
            scene.to_bits().abs_diff(expected.to_bits()) <= F64_STANDARD_ULPS,
            "scene decode of {i}: {scene:?} vs {expected:?}"
        );
        let map = srgb_to_linear_f32(f32::from(i) / 255.);
        assert!(
            map.to_bits().abs_diff(expected.to_bits()) <= F32_STANDARD_ULPS,
            "map decode of {i}: {map:?} vs {expected:?}"
        );
    }
    for &(linear, expected) in STANDARD_ENCODES {
        assert_eq!(
            linear_to_srgb8(f64::from(linear)),
            expected,
            "scene encode of {linear:?}"
        );
        assert_eq!(linear_to_srgb8_f32(linear), expected, "map encode of {linear:?}");
    }
}

// The writers' pre-extraction formulas, recomputed here. BASELINE only: it pins the exact bits their f32/f64
// rounding policy produces for every byte and at every half-byte boundary, so a change in our own output shows.
// It is not a correctness oracle (a shared misreading would pass); the hand-worked values above are that check.
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
fn baseline_writer_rounding_matches_its_pre_extraction_formulas() {
    for i in 0..=255u8 {
        assert_eq!(
            (srgb_to_linear(f64::from(i) / 255.) as f32).to_bits(),
            scene_decode(i).to_bits(),
            "scene decode of {i}"
        );
        assert_eq!(
            srgb_to_linear_f32(f32::from(i) / 255.).to_bits(),
            map_decode(i).to_bits(),
            "map decode of {i}"
        );
    }
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

/// [`scene_encode`] on an f64 input.
fn scene_encode_f64(linear: f64) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// The inputs bracketing every half-byte boundary of the curve, `spread` ulps each way, in f64.
fn near_boundaries_f64(spread: u64) -> Vec<f64> {
    let mut out = Vec::new();
    for i in 0..255 {
        let s = (f64::from(i) + 0.5) / 255.;
        let c = if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        };
        let bits = c.to_bits();
        out.extend((bits - spread..=bits + spread).map(f64::from_bits));
    }
    out
}

/// The tables give the curves' own results: every decoded byte bit for bit, and the encoded byte of every input tried:
/// 2^22 inputs spread evenly over [0, 1], 4001 ulps around each of the 255 half-byte boundaries (where a rounding
/// threshold lies), and the values outside (0, 1) (negative, above 1, infinite, NaN), in f64 and f32. The oracle is the
/// curve as written before the tables ([`scene_encode`], [`map_encode`]).
#[test]
fn byte_tables_give_the_curves_own_results() {
    use primase_image::{srgb8_to_linear, srgb8_to_linear_f32};
    for i in 0..=255u8 {
        assert_eq!(
            srgb8_to_linear(i).to_bits(),
            srgb_to_linear(f64::from(i) / 255.).to_bits(),
            "decode {i}"
        );
        assert_eq!(
            srgb8_to_linear_f32(i).to_bits(),
            map_decode(i).to_bits(),
            "f32 decode {i}"
        );
    }
    let specials = [
        -1.0,
        -0.0,
        0.0,
        1.0,
        1.5,
        f64::MIN,
        f64::MAX,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ];
    let even = (0..=1u32 << 22).map(|k| f64::from(k) / f64::from(1u32 << 22));
    for c in specials.into_iter().chain(even).chain(near_boundaries_f64(2000)) {
        assert_eq!(
            linear_to_srgb8(c),
            scene_encode_f64(c),
            "f64 input {:016x}",
            c.to_bits()
        );
        let v = c as f32;
        assert_eq!(
            linear_to_srgb8_f32(v),
            map_encode(v),
            "f32 input {:08x}",
            v.to_bits()
        );
    }
    for c in near_boundaries_f64(0) {
        let bits = (c as f32).to_bits();
        for v in (bits - 2000..=bits + 2000).map(f32::from_bits) {
            assert_eq!(
                linear_to_srgb8_f32(v),
                map_encode(v),
                "f32 input {:08x}",
                v.to_bits()
            );
        }
    }
}

/// [`linear_to_srgb8_f32`] against its curve on every f32 from 0 to 1 (1,065,353,217 inputs). Slow: run with
/// `--ignored` in release.
#[test]
#[ignore = "every f32 in [0, 1]: run with --ignored in release"]
fn f32_byte_table_gives_the_curves_result_on_every_input() {
    let ones = 1.0f32.to_bits();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u32;
    let wrong: u64 = std::thread::scope(|s| {
        (0..threads)
            .map(|t| {
                s.spawn(move || {
                    (t..=ones)
                        .step_by(threads as usize)
                        .filter(|&b| {
                            let v = f32::from_bits(b);
                            linear_to_srgb8_f32(v) != map_encode(v)
                                || linear_to_srgb8(f64::from(v)) != scene_encode(v)
                        })
                        .count() as u64
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().expect("worker"))
            .sum()
    });
    assert_eq!(wrong, 0);
}

/// A stated colour (`R,G,B,A`) prints as it parses, refuses anything but four bytes, and decodes its colour bytes as
/// `srgb_to_linear(byte / 255)` rounded to f32 and its alpha as `byte / 255`: the bits the lowerings that held a
/// flat colour of their own computed.
#[test]
fn a_stated_colour_parses_prints_and_decodes() {
    let c = primase_image::Srgba8::parse("48,98,122,190").unwrap();
    assert_eq!(c.0, [48, 98, 122, 190]);
    assert_eq!(c.to_string(), "48,98,122,190");
    for bad in [
        "",
        "48,98,122",
        "48,98,122,190,1",
        "48,98,122,256",
        "48,98,-1,190",
        "a,b,c,d",
    ] {
        assert!(primase_image::Srgba8::parse(bad).is_err(), "{bad}");
    }
    for byte in 0..=255u8 {
        let [r, _, _, a] = primase_image::Srgba8([byte, 0, 0, byte]).linear();
        assert_eq!(
            r.to_bits(),
            (srgb_to_linear(f64::from(byte) / 255.0) as f32).to_bits()
        );
        assert_eq!(a.to_bits(), ((f64::from(byte) / 255.0) as f32).to_bits());
    }
}
