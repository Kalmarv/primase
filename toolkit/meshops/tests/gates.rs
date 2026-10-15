//! C# primitive tessellation oracle, with its reference vector arithmetic library.
use primase_meshops::*;
#[test]
fn primitives_exact() {
    let data = include_bytes!("fixtures/primitives.bin");
    let mut words = data.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    for case in 0..count {
        let kind = words.next().unwrap();
        let r = f32::from_bits(words.next().unwrap());
        let nv = words.next().unwrap() as usize;
        let ni = words.next().unwrap() as usize;
        let a = [1.25, -2.5, 3.75];
        let b = [3.1, 4.2, -0.7];
        let m = match kind {
            0 => sphere(a, r),
            1 => cylinder(a, b, r),
            2 => capsule(a, b, r),
            3 => box_mesh(a, [[2., 0., 0.], [0.1, 3., 0.], [0., 0., r * 4.]]),
            _ => panic!(),
        }
        .unwrap();
        assert_eq!(m.positions.len(), nv);
        assert_eq!(m.indices.len(), ni);
        for (i, p) in m.positions.iter().enumerate() {
            for (k, x) in p.iter().enumerate() {
                assert_eq!(
                    x.to_bits(),
                    words.next().unwrap(),
                    "case {case} kind {kind} r {r} vertex {i} channel {k}"
                );
            }
        }
        for i in m.indices {
            assert_eq!(i, words.next().unwrap());
        }
    }
    assert!(words.next().is_none());
}

#[test]
fn reference_operations_exact() {
    fn mesh(words: &mut impl Iterator<Item = u32>) -> Mesh {
        fn array<const N: usize>(words: &mut impl Iterator<Item = u32>) -> Vec<[f32; N]> {
            let len = words.next().unwrap() as usize;
            assert!(len.is_multiple_of(N));
            (0..len / N)
                .map(|_| std::array::from_fn(|_| f32::from_bits(words.next().unwrap())))
                .collect()
        }
        let positions = array(words);
        let normals = array(words);
        let uv = array(words);
        let len = words.next().unwrap();
        let indices = (0..len).map(|_| words.next().unwrap()).collect();
        Mesh {
            positions,
            normals,
            uv,
            indices,
        }
    }
    let mut words = include_bytes!("fixtures/operations.bin")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    let input = mesh(&mut words);
    for _ in 0..count {
        let op = words.next().unwrap();
        let expected = mesh(&mut words);
        let actual = match op {
            0 => flat_normals(&input).unwrap(),
            1 => split_long(&input, 1.2, 10000).unwrap().0,
            2 => transform(&input, [[2., 0., 0.], [0., 3., 0.], [0., 0., 4.]], [1., 2., 3.]).unwrap(),
            3 => transform(
                &input,
                [[-2., 0., 0.], [0.3, 3., 0.], [0.1, 0., 4.]],
                [1., 2., 3.],
            )
            .unwrap(),
            4 => transform(&input, [[-2., 0., 0.], [0., 3., 0.], [0., 0., 4.]], [1., 2., 3.]).unwrap(),
            _ => panic!(),
        };
        assert_eq!(actual.indices, expected.indices, "op {op} indices");
        for (name, a, b) in [
            ("positions", actual.positions, expected.positions),
            ("normals", actual.normals, expected.normals),
        ] {
            assert_eq!(a.len(), b.len());
            for (i, (a, b)) in a.iter().zip(&b).enumerate() {
                assert_eq!(
                    a.map(f32::to_bits),
                    b.map(f32::to_bits),
                    "op {op} {name} vertex {i}"
                );
            }
        }
        assert_eq!(
            actual.uv.iter().map(|a| a.map(f32::to_bits)).collect::<Vec<_>>(),
            expected
                .uv
                .iter()
                .map(|a| a.map(f32::to_bits))
                .collect::<Vec<_>>()
        );
    }
    assert!(words.next().is_none());
}

#[test]
fn reference_weld_and_uv_exact() {
    let mut words = include_bytes!("fixtures/weld-uv.bin")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b));
    let count = words.next().unwrap();
    for _ in 0..count {
        let uv = std::array::from_fn(|_| std::array::from_fn(|_| f32::from_bits(words.next().unwrap())));
        assert_eq!(uv_tiles(uv).unwrap().to_bits(), words.next().unwrap());
    }
    fn read(words: &mut impl Iterator<Item = u32>) -> Mesh {
        let n = words.next().unwrap();
        let positions = (0..n / 3)
            .map(|_| std::array::from_fn(|_| f32::from_bits(words.next().unwrap())))
            .collect();
        let n = words.next().unwrap();
        let indices = (0..n).map(|_| words.next().unwrap()).collect();
        Mesh {
            positions,
            indices,
            ..Mesh::default()
        }
    }
    let input = read(&mut words);
    let expected = read(&mut words);
    let actual = weld_millimetres(&input, [0., 0.]).unwrap();
    assert_eq!(actual.indices, expected.indices);
    assert_eq!(
        actual
            .positions
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect::<Vec<_>>(),
        expected
            .positions
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect::<Vec<_>>()
    );
    assert!(words.next().is_none());
}
#[test]
fn validation_compaction_and_limits() {
    let mesh = Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [7., 7., 7.]],
        indices: vec![2, 0, 1],
        ..Mesh::default()
    };
    let compacted = compact(&mesh).unwrap();
    assert_eq!(
        compacted.positions,
        vec![mesh.positions[2], mesh.positions[0], mesh.positions[1]]
    );
    assert_eq!(compacted.indices, vec![0, 1, 2]);
    assert!(split_long(&mesh, 0.2, 0).is_err());
    assert_eq!(mesh.positions.len(), 4);
    assert!(transform(&mesh, [[0.; 3]; 3], [0.; 3]).is_err());
    assert!(sphere([f32::NAN, 0., 0.], 1.).is_err());
    assert!(uv_tiles([[f32::INFINITY; 2]; 3]).is_err());
    let invalid = Mesh {
        indices: vec![0, 1, 4],
        ..mesh
    };
    assert!(compact(&invalid).is_err());
    assert!(split_long(&invalid, 1., 10).is_err());
}
