use primase_fromsoft::layer_bake::BaseMipBilinearWrap;

#[test]
fn centers_wrap_seams_and_alpha() {
    let image = BaseMipBilinearWrap::new(
        2,
        2,
        &[255, 0, 0, 0, 0, 255, 0, 64, 0, 0, 255, 128, 255, 255, 255, 255],
        false,
    )
    .unwrap();
    assert_eq!(image.sample([0.25, 0.25]).unwrap(), [1., 0., 0., 0.]);
    assert_eq!(image.sample([1.25, -0.75]).unwrap(), [1., 0., 0., 0.]);
    let midpoint = image.sample([0.5, 0.5]).unwrap();
    assert_eq!(&midpoint[..3], &[0.5; 3]);
    assert!((midpoint[3] - 447.0 / 1020.0).abs() < 1e-7);
    // Wrapping permutes the four summands. Bound the resulting f32 rounding,
    // while periodic coordinates with the same ordering remain bit-identical.
    let seam = image.sample([0., 0.]).unwrap();
    for (a, b) in seam.into_iter().zip(midpoint) {
        assert!(a.to_bits().abs_diff(b.to_bits()) <= 1);
    }
    assert_eq!(image.sample([1., 1.]).unwrap(), seam);
    assert!(image.sample([f32::NAN, 0.]).is_err());
}

#[test]
fn dimensions_and_coordinate_domain_are_checked() {
    assert!(BaseMipBilinearWrap::new(0, 1, &[], false).is_err());
    assert!(BaseMipBilinearWrap::new(2, 1, &[0; 4], false).is_err());
    let image = BaseMipBilinearWrap::new(1, 1, &[64; 4], false).unwrap();
    for uv in [[f32::INFINITY, 0.], [0., f32::NEG_INFINITY], [f32::MAX, 0.]] {
        assert!(image.sample(uv).is_err());
    }
    assert_eq!(image.sample([-16., 32.]).unwrap(), [64. / 255.; 4]);
}
#[test]
fn srgb_is_decoded_before_filtering() {
    let image = BaseMipBilinearWrap::new(2, 1, &[0, 0, 0, 0, 255, 255, 255, 255], true).unwrap();
    assert_eq!(image.sample([0.5, 0.5]).unwrap(), [0.5; 4]);
    // Averaging encoded bytes then decoding would yield about 0.214 instead.
    let middle = BaseMipBilinearWrap::new(1, 1, &[128; 4], true)
        .unwrap()
        .sample([0., 0.])
        .unwrap();
    assert!((middle[0] - 0.2158605).abs() < 1e-7);
    assert_eq!(middle[3], 128.0 / 255.0);
}
