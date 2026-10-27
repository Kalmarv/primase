#![allow(clippy::unwrap_used, reason = "hand-worked native enum fixture")]
use iridia_interface::{AddressMode, SamplerState};
use iridia_interface::{Filter, MipFilter};
#[test]
fn native_sampler_pairs_keep_addresses_filters_and_unknowns() {
    let s = iridia_d3d9::sampler_states([
        (1, 2),
        (2, 3),
        (3, 5),
        (5, 1),
        (6, 2),
        (7, 2),
        (8, 0.5f32.to_bits()),
    ])
    .unwrap();
    assert_eq!(
        s.wrap,
        [
            Some(AddressMode::Mirror),
            Some(AddressMode::Clamp),
            Some(AddressMode::MirrorOnce)
        ]
    );
    assert_eq!(s.filters.mag, Some(Filter::Nearest));
    assert_eq!(s.filters.min, Some(Filter::Linear));
    assert_eq!(s.filters.mip, Some(MipFilter::Linear));
    assert_eq!(s.mip_lod_bias, Some(0.5));
    assert_eq!(iridia_d3d9::sampler_states([]).unwrap(), SamplerState::default());
    for pair in [
        (4, 0),
        (6, 3),
        (6, 7),
        (7, 3),
        (8, f32::NAN.to_bits()),
        (10, 0),
        (10, 8),
        (11, 1),
        (1, 99),
    ] {
        assert!(iridia_d3d9::sampler_states([pair]).is_err(), "{pair:?}");
    }
}
