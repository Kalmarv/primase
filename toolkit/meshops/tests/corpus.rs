//! Optional independent C# value oracle corpus; the case bundle contains source meshes
//! and five expected outputs. See README for its binary contract.
use primase_meshops::*;
mod support;
#[test]
fn real_mesh_reference_cases() {
    let Some(path) = std::env::var_os("PRIMASE_MESH_CASES") else {
        eprintln!("SKIPPED: PRIMASE_MESH_CASES unset");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.len().is_multiple_of(4));
    let mut words = bytes.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b));
    let cases = words.next().unwrap();
    assert!(cases > 0);
    fn mesh(w: &mut impl Iterator<Item = u32>) -> Mesh {
        fn floats<const N: usize>(w: &mut impl Iterator<Item = u32>) -> Vec<[f32; N]> {
            let n = w.next().unwrap() as usize;
            assert!(n.is_multiple_of(N));
            (0..n / N)
                .map(|_| std::array::from_fn(|_| f32::from_bits(w.next().unwrap())))
                .collect()
        }
        let positions = floats(w);
        let normals = floats(w);
        let uv = floats(w);
        let n = w.next().unwrap();
        let indices = (0..n).map(|_| w.next().unwrap()).collect();
        Mesh {
            positions,
            normals,
            uv,
            indices,
        }
    }
    let mut stats = support::Floats::default();
    let (mut checked, mut vertices, mut triangles) = (0, 0, 0);
    for case in 0..cases {
        let input = mesh(&mut words);
        vertices += input.positions.len();
        triangles += input.indices.len() / 3;
        let n = words.next().unwrap();
        assert_eq!(n, 5);
        for _ in 0..n {
            let op = words.next().unwrap();
            let expected = mesh(&mut words);
            let actual = match op {
                0 => flat_normals(&input).unwrap(),
                1 => split_long(&input, 16., 1_000_000).unwrap().0,
                2 => transform(
                    &input,
                    [[-2., 0., 0.], [0.3, 3., 0.], [0.1, 0., 4.]],
                    [1., 2., 3.],
                )
                .unwrap(),
                3 => weld_millimetres(&input, [0., 0.]).unwrap(),
                4 => compact(&input).unwrap(),
                _ => panic!("unknown op"),
            };
            assert_eq!(actual.indices, expected.indices, "case {case} op {op} indices");
            if op == 1 {
                // Subdivision appends derived midpoints; existing attributes are
                // stored data and must not be covered by a float tolerance.
                assert_eq!(
                    actual.positions[..input.positions.len()]
                        .iter()
                        .map(|v| v.map(f32::to_bits))
                        .collect::<Vec<_>>(),
                    input
                        .positions
                        .iter()
                        .map(|v| v.map(f32::to_bits))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    actual.normals[..input.normals.len()]
                        .iter()
                        .map(|v| v.map(f32::to_bits))
                        .collect::<Vec<_>>(),
                    input
                        .normals
                        .iter()
                        .map(|v| v.map(f32::to_bits))
                        .collect::<Vec<_>>()
                );
            }
            for (kind, (a, b)) in [
                (&actual.positions, &expected.positions),
                (&actual.normals, &expected.normals),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(a.len(), b.len());
                for (a, b) in a.iter().zip(b) {
                    if op >= 3 || (op == 0 && kind == 0) {
                        assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
                    } else {
                        for (x, y) in a.iter().zip(b) {
                            stats.compare(*x, *y);
                        }
                    }
                }
            }
            assert_eq!(
                actual.uv.iter().map(|v| v.map(f32::to_bits)).collect::<Vec<_>>(),
                expected
                    .uv
                    .iter()
                    .map(|v| v.map(f32::to_bits))
                    .collect::<Vec<_>>()
            );
            checked += 1;
        }
    }
    stats.report("real operation derived values");
    assert!(words.next().is_none());
    eprintln!(
        "C# real mesh cases={cases} source_vertices={vertices} source_triangles={triangles} checked={checked} topology_exact={checked} failed=0"
    );
}
