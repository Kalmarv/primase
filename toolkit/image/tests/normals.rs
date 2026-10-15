use primase_image::{Image, Samples, normal_mip_chain, resize_normals};

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
