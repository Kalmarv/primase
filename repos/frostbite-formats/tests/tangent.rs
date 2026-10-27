//! Gate of the tangent frame codec (`frostbite_formats::tangent`): frames encode and decode back within the codec's
//! steps (normal 0.2 degrees, tangent 0.4); a frame on an axis, a tangent along the normal, both signs.

#![allow(clippy::unwrap_used)]

use frostbite_formats::tangent;

fn angle(a: [f32; 3], b: [f32; 3]) -> f64 {
    let d: f64 = (0..3).map(|k| f64::from(a[k]) * f64::from(b[k])).sum();
    let l = |v: [f32; 3]| (0..3).map(|k| f64::from(v[k]).powi(2)).sum::<f64>().sqrt();
    (d / (l(a) * l(b))).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn frames_round_trip_within_their_steps() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let (mut worst_n, mut worst_t) = (0.0f64, 0.0f64);
    for i in 0..20_000 {
        let n = [next(), next(), next()].map(|v| v as f32);
        let r = [next(), next(), next()].map(|v| v as f32);
        if angle(n, r) < 1.0 || angle(n, r) > 179.0 {
            continue;
        }
        // the tangent: r made perpendicular to n
        let nn: f32 = n.iter().map(|v| v * v).sum::<f32>().sqrt();
        let u = n.map(|v| v / nn);
        let d: f32 = (0..3).map(|k| u[k] * r[k]).sum();
        let t = [0, 1, 2].map(|k| r[k] - u[k] * d);
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        let f = tangent::decode(tangent::encode(n, t, sign).unwrap());
        worst_n = worst_n.max(angle(f.normal, n));
        worst_t = worst_t.max(angle(f.tangent, t));
        assert_eq!(f.sign, sign);
    }
    assert!(
        worst_n < 0.2 && worst_t < 0.4,
        "worst normal {worst_n} deg, tangent {worst_t} deg"
    );
    // on an axis, every axis and direction
    for k in 0..3 {
        for s in [1.0f32, -1.0] {
            let mut n = [0.0f32; 3];
            n[k] = s;
            let mut t = [0.0f32; 3];
            t[(k + 1) % 3] = 1.0;
            let f = tangent::decode(tangent::encode(n, t, 1.0).unwrap());
            assert!(
                angle(f.normal, n) < 0.01 && angle(f.tangent, t) < 0.2,
                "{n:?} {t:?}: {f:?}"
            );
        }
    }
    // a tangent along the normal has no direction to keep: angle 0; a normal without length is refused
    assert!(tangent::encode([0.0, 1.0, 0.0], [0.0, 2.0, 0.0], 1.0).is_some());
    assert!(tangent::encode([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0).is_none());
}
