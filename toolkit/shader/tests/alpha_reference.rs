use primase_core::shader::AlphaComparison;
use primase_shader::output_uses::{AlphaDither, discard_compares};

#[test]
fn alpha_reference_requires_structural_data_and_a_disjoint_dither_source() {
    macro_rules! fixture {
        ($entry:literal, $expected:expr, $texture:literal) => {
            (
                $entry,
                include_bytes!(concat!("fixtures/alpha_reference_", $entry, ".dxbc")).as_slice(),
                include_bytes!(concat!("fixtures/alpha_reference_", $entry, ".dxil")).as_slice(),
                $expected,
                $texture,
            )
        };
    }
    let fixtures = [
        fixture!("direct", Some(AlphaComparison::Output), "dither_map"),
        fixture!("transformed", Some(AlphaComparison::Transformed), "dither_map"),
        fixture!(
            "dithered",
            Some(AlphaComparison::Dithered { scale_word: 1 }),
            "dither_map"
        ),
        fixture!(
            "product",
            Some(AlphaComparison::Dithered { scale_word: 1 }),
            "dither_map"
        ),
        fixture!("same_source", None, "base_map"),
        fixture!("shared_scale", None, "dither_map"),
        fixture!("control_only", None, "dither_map"),
        fixture!("coordinate_only", None, "dither_map"),
        fixture!("unrelated", None, "dither_map"),
    ];
    for (name, dxbc, dxil, expected, dither_name) in fixtures {
        for (compiler, p) in [
            ("fxc", primase_dxbc::lift(dxbc).unwrap()),
            ("dxc", primase_dxil::lift(dxil).unwrap()),
        ] {
            let g = p.graph().unwrap();
            let buffer = g.reads.buffer_at(0, 0).unwrap();
            let dither = g
                .interface
                .resources
                .iter()
                .find(|r| r.provenance.name == dither_name)
                .map(|r| AlphaDither {
                    texture: r.id,
                    scale_word: 1,
                });
            assert_eq!(
                discard_compares(&g, buffer, 0, dither),
                expected,
                "{name} ({compiler})"
            );
            if matches!(expected, Some(AlphaComparison::Dithered { .. })) {
                assert_eq!(
                    discard_compares(&g, buffer, 0, None),
                    None,
                    "dither needs its producer's role"
                );
            }
        }
    }
}
