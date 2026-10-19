//! `weld_exact`: only bit-identical corners merge, and every face survives.

use primase_meshops::{weld_exact, weld_quantized};

#[test]
fn only_bit_identical_corners_merge_and_every_face_is_kept() {
    let a = [0.0, 0.0, 0.0];
    let near = [0.0001, 0.0, 0.0];
    let corners = [
        a,
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        near,
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        a,
        a,
        [2.0, 0.0, 0.0],
    ];
    let m = weld_exact(&corners).unwrap();
    // 0.1 mm apart stays apart; the third face's coincident corners are kept as one vertex, the face too
    assert_eq!(
        m.positions,
        vec![a, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], near, [2.0, 0.0, 0.0]]
    );
    assert_eq!(m.indices, vec![0, 1, 2, 3, 1, 2, 0, 0, 4]);
    // the 1 mm grid merges what exact keeps apart, and drops the face that collapses
    let q = weld_quantized(&corners, 1000.0).unwrap();
    assert_eq!(q.indices, vec![0, 1, 2, 0, 1, 2]);
}

#[test]
fn partial_triangles_and_non_finite_corners_are_errors() {
    assert!(weld_exact(&[[0.0; 3]; 2]).is_err());
    assert!(weld_exact(&[[0.0; 3], [f32::NAN, 0.0, 0.0], [1.0; 3]]).is_err());
}
