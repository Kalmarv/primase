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

/// Hand-worked at scale 1 (one grid cell per unit): keys are the coordinates rounded half to even, so 0.5 and -0.5 share cell 0
/// and 1.5 and 2.5 share cell 2, 3.5 is cell 4. The first face collapses (0.5, -0.5, 1.5 -> cells 0, 0, 2) and is dropped, its two
/// vertices stay; the second keeps the first position of each cell.
#[test]
fn the_grid_rounds_half_to_even_keeps_first_positions_and_drops_collapsed_faces() {
    let corners = [
        [0.5, 0.0, 0.0],
        [-0.5, 0.0, 0.0],
        [1.5, 0.0, 0.0],
        [2.5, 0.0, 0.0],
        [3.5, 0.0, 0.0],
        [0.5, 1.0, 0.0],
    ];
    let m = weld_quantized(&corners, 1.0).unwrap();
    assert_eq!(
        m.positions,
        vec![[0.5, 0.0, 0.0], [1.5, 0.0, 0.0], [3.5, 0.0, 0.0], [0.5, 1.0, 0.0]]
    );
    assert_eq!(m.indices, vec![1, 2, 3]);
}

#[test]
fn a_bad_scale_or_a_key_out_of_range_is_an_error() {
    let tri = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(weld_quantized(&tri, scale).is_err(), "scale {scale}");
    }
    // 3e9 cells does not fit a signed 32-bit key
    assert!(weld_quantized(&[[3.0e9, 0.0, 0.0], tri[1], tri[2]], 1.0).is_err());
    assert!(weld_quantized(&tri, 1.0).is_ok());
}
