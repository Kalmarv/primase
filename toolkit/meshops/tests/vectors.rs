mod support;
use primase_meshops::vector::Vec3;
#[test]
fn independent_vector_values() {
    let mut words = include_bytes!("fixtures/vectors.bin")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    let mut stats = support::Floats::default();
    let mut small_deviations = 0;
    for _ in 0..count {
        let a = Vec3::from(std::array::from_fn(|_| f32::from_bits(words.next().unwrap())));
        let b = Vec3::from(std::array::from_fn(|_| f32::from_bits(words.next().unwrap())));
        let t = f32::from_bits(words.next().unwrap());
        let n = a.normalize();
        let d = if t != 0. { a / t } else { a };
        let cross = a.cross(b);
        let actual = [
            a.length(),
            a.length_squared(),
            n.x,
            n.y,
            n.z,
            d.x,
            d.y,
            d.z,
            a.dot(b),
            cross.x,
            cross.y,
            cross.z,
        ];
        for (channel, value) in actual.into_iter().enumerate() {
            let expected = f32::from_bits(words.next().unwrap());
            if (2..5).contains(&channel) && a.length() > 0. && a.length() < 1e-6 {
                // The historical oracle leaves these non-unit vectors unchanged.
                // Our contract normalizes every finite nonzero vector instead.
                if value != expected {
                    small_deviations += 1;
                }
                assert!((n.length() - 1.).abs() <= f32::EPSILON);
            } else {
                stats.compare(value, expected);
            }
        }
    }
    assert!(words.next().is_none());
    stats.report("independent vector oracle");
    eprintln!("named tiny-vector passthrough deviations={small_deviations}");
}
#[test]
fn normalization_defined_for_zero_tiny_and_large() {
    assert_eq!(Vec3::ZERO.normalize(), Vec3::ZERO);
    for size in [f32::MIN_POSITIVE, f32::MAX, 1e-8, 1.] {
        let n = Vec3::new(size, 0., 0.).normalize();
        assert_eq!(n, Vec3::UNIT_X);
    }
    assert_eq!(Vec3::ZERO.normalize_or(Vec3::UNIT_Y), Vec3::UNIT_Y);
    assert_eq!(
        Vec3::new(f32::NAN, 0., 0.).normalize_or(Vec3::UNIT_Z),
        Vec3::UNIT_Z
    );
}

#[test]
fn small_face_has_a_unit_normal() {
    let mesh = primase_meshops::Mesh {
        positions: vec![[0.; 3], [1e-4, 0., 0.], [0., 1e-4, 0.]],
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let flat = primase_meshops::flat_normals(&mesh).unwrap();
    assert_eq!(flat.positions, mesh.positions);
    assert_eq!(flat.normals, vec![[0., 0., 1.]; 3]);
}
