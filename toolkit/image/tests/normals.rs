use primase_image::{Image, Samples, normal_mip_chain, rebuild_normal_z, resize_normals};

#[test]
fn opposing_quantized_normals_become_flat_and_preserve_mean_alpha() {
    let image = Image {
        width: 2,
        height: 1,
        samples: Samples::U8(vec![255, 128, 128, 0, 0, 127, 127, 200]),
    };
    let filtered = resize_normals(&image, 1, 1).expect("valid normals");
    assert_eq!(filtered.near_zero_to_flat, 1);
    assert_eq!(filtered.image.samples, Samples::U8(vec![128, 128, 255, 100]));
    let (chain, count) = normal_mip_chain(&image).expect("chain");
    assert_eq!(count, 1);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1], filtered.image);
}

#[test]
fn odd_edges_contribute_and_sources_are_normalized_before_averaging() {
    // Two shorter +X vectors and one full-length +Y: unit-vector mean is (2,1,0).
    let image = Image {
        width: 3,
        height: 1,
        samples: Samples::U8(vec![191, 128, 128, 255, 191, 128, 128, 255, 128, 255, 128, 255]),
    };
    let filtered = resize_normals(&image, 1, 1).expect("valid normals");
    let Samples::U8(p) = filtered.image.samples else {
        panic!("RGBA8")
    };
    assert!((p[0] as i32 - 241).abs() <= 1);
    assert!((p[1] as i32 - 185).abs() <= 1);
    assert_eq!(filtered.near_zero_to_flat, 0);
}

#[test]
fn invalid_shapes_and_non_rgba8_are_errors() {
    let image = Image {
        width: 1,
        height: 1,
        samples: Samples::U8(vec![128, 128, 255, 255]),
    };
    assert!(resize_normals(&image, 0, 1).is_err());
    let bad = Image {
        samples: Samples::U16(vec![0; 4]),
        ..image
    };
    assert!(resize_normals(&bad, 1, 1).is_err());
    assert!(normal_mip_chain(&bad).is_err());
}

#[test]
fn quantization_residue_is_flat_but_resolved_direction_survives() {
    for (offset, expected) in [(128, 1), (129, 0)] {
        let image = Image {
            width: 2,
            height: 1,
            samples: Samples::U8(vec![255, offset, offset, 255, 0, offset, offset, 255]),
        };
        let filtered = resize_normals(&image, 1, 1).expect("valid normals");
        assert_eq!(filtered.near_zero_to_flat, expected);
        if expected == 1 {
            assert_eq!(filtered.image.samples, Samples::U8(vec![128, 128, 255, 255]));
        }
    }
}

/// Worked by hand from x = r / 127.5 - 1, y = g / 127.5 - 1 and blue = round_ties_even((z / 2 + 1 / 2) * 255).
#[test]
fn z_is_rebuilt_inside_the_disk_and_xy_projected_outside_it() {
    let rebuilt = |rg: [u8; 2]| {
        let mut p = [rg[0], rg[1], 7, 200];
        let projected = rebuild_normal_z(&mut p);
        (p, projected)
    };
    // x = y = 1/255: z = 0.99998..., blue 254.998 -> 255
    assert_eq!(rebuilt([128, 128]), ([128, 128, 255, 200], false));
    // x = -0.49804, y = 0.50588: r^2 = 0.50396, z = 0.70430, blue 217.298 -> 217
    assert_eq!(rebuilt([64, 192]), ([64, 192, 217, 200], false));
    // x = 1, y = 1/255: outside the disk; xy / |xy| = (0.999992, 0.0039215) -> 255, 128; z = 0, blue 127.5 -> 128 (ties
    // to even)
    assert_eq!(rebuilt([255, 128]), ([255, 128, 128, 200], true));
    assert_eq!(rebuilt([128, 255]), ([128, 255, 128, 200], true));
    // x = y = -1: projected to -1/sqrt(2) each, 37.344 -> 37
    assert_eq!(rebuilt([0, 0]), ([37, 37, 128, 200], true));
}
