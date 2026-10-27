use primase_core::shader::AlphaComparison;
use iridia::output_uses::{AlphaDither, AlphaExpr, Known, alpha_tests, discard_compares};

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
        // a test of texture alpha the dither role does not explain, or whose value reaches the target only as a
        // coordinate or a branch: the discard compares the word all the same, and its value is the program's expression
        // ([`alpha_tests`]), not an output alpha
        fixture!("same_source", Some(AlphaComparison::Unwritten), "base_map"),
        fixture!("shared_scale", Some(AlphaComparison::Unwritten), "dither_map"),
        fixture!("control_only", Some(AlphaComparison::Unwritten), "dither_map"),
        fixture!("coordinate_only", Some(AlphaComparison::Unwritten), "dither_map"),
        // a vertex colour's alpha is no texture's: nothing here is an alpha test
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
                // without the producer's dither role the perturbation is not recognized as one, but the test is still
                // the program's exact expression
                assert_eq!(
                    discard_compares(&g, buffer, 0, None),
                    Some(AlphaComparison::Unwritten),
                    "dither needs its producer's role"
                );
            }
        }
    }
}

/// A fixture compiled from `fixtures/alpha_reference.hlsl` (the header says how), read at run time: these two entry
/// points were added with `alpha_tests` and are compiled by the same commands.
fn compiled(entry: &str) -> Vec<(&'static str, iridia::Program)> {
    let read = |extension: &str| {
        let path = format!(
            "{}/tests/fixtures/alpha_reference_{entry}.{extension}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{path}: {e} (compile alpha_reference.hlsl's `{entry}`)"))
    };
    vec![
        (
            "fxc",
            primase_dxbc::lift(&read("dxbc")).unwrap_or_else(|e| panic!("{entry}.dxbc: {e}")),
        ),
        (
            "dxc",
            primase_dxil::lift(&read("dxil")).unwrap_or_else(|e| panic!("{entry}.dxil: {e}")),
        ),
    ]
}

/// The program's own test: `base.a * detail.a < reference` is `tex.w * tex.w` against the word, nothing written.
/// Two real textures leave two varying samples, which no mask cutoff states; one bound to a known value leaves one.
#[test]
fn a_test_of_two_texture_alphas_is_read_exactly_and_solved_only_where_one_varies() {
    for (compiler, p) in compiled("unwritten") {
        let g = p.graph().unwrap();
        let buffer = g.reads.buffer_at(0, 0).unwrap();
        assert_eq!(
            discard_compares(&g, buffer, 0, None),
            Some(AlphaComparison::Unwritten),
            "{compiler}"
        );
        let tests: Vec<_> = alpha_tests(&g).into_iter().filter(|t| t.word == 0).collect();
        assert_eq!(tests.len(), 1, "{compiler}: {tests:?}");
        let tested = &tests[0].tested;
        assert!(
            tested.is_exact() && tested.has_alpha_texel(),
            "{compiler}: {tested}"
        );
        let samples: Vec<_> = tested
            .leaves()
            .into_iter()
            .filter(|l| matches!(l, AlphaExpr::Texel { .. }))
            .collect();
        assert_eq!(samples.len(), 2, "{compiler}: {tested}");
        let name_of = |resource| {
            g.interface
                .resources
                .iter()
                .find(|r| r.id == resource)
                .map(|r| r.provenance.name.clone())
                .unwrap_or_default()
        };
        // both real: refused, naming why
        let both = tested.linear(&mut |leaf| match leaf {
            AlphaExpr::Texel { .. } => Known::Varies,
            _ => Known::Unknown,
        });
        let why = both.expect_err(compiler);
        assert!(why.contains("two varying samples"), "{compiler}: {why}");
        // the detail map bound to a texture of alpha 0.5 everywhere: one varies, the cutoff is the reference over 0.5
        let mut half_detail = |leaf: &AlphaExpr| match leaf {
            AlphaExpr::Texel { resource, .. } if name_of(*resource) == "detail_map" => Known::Value(0.5),
            AlphaExpr::Texel { .. } => Known::Varies,
            _ => Known::Unknown,
        };
        let linear = tested.linear(&mut half_detail).expect(compiler);
        assert_eq!((linear.scale, linear.offset), (0.5, 0.), "{compiler}");
        assert_eq!(
            linear.sample.map(|(_, r, lane)| (name_of(r), lane)),
            Some(("base_map".to_owned(), 3)),
            "{compiler}"
        );
        assert_eq!(linear.cutoff(0.25).unwrap(), Some(0.5), "{compiler}");
        // a default of alpha 1 drops out: the cutoff is the reference itself
        let one = tested
            .linear(&mut |leaf| match leaf {
                AlphaExpr::Texel { resource, .. } if name_of(*resource) == "detail_map" => Known::Value(1.),
                AlphaExpr::Texel { .. } => Known::Varies,
                _ => Known::Unknown,
            })
            .expect(compiler);
        assert_eq!(one.cutoff(0.3).unwrap(), Some(0.3), "{compiler}");
        // a default of alpha 0 discards everything: refused, not cut at the reference
        let zero = tested
            .linear(&mut |leaf| match leaf {
                AlphaExpr::Texel { resource, .. } if name_of(*resource) == "detail_map" => Known::Value(0.),
                AlphaExpr::Texel { .. } => Known::Varies,
                _ => Known::Unknown,
            })
            .expect(compiler);
        assert!(zero.cutoff(0.3).is_err(), "{compiler}");
        assert_eq!(
            zero.cutoff(-100.).unwrap(),
            None,
            "{compiler}: a reference below the range never discards"
        );
    }
}

/// `base.a + (dither.a - 0.5) * scale < reference` with the dither bound to its default 0.5 is `base.a < reference`
/// whatever the scale (a finite word), and with the dither bound to another value the term is that value's.
#[test]
fn a_dither_bound_to_its_default_adds_exactly_its_value() {
    for (compiler, p) in compiled("unwritten_dither") {
        let g = p.graph().unwrap();
        let buffer = g.reads.buffer_at(0, 0).unwrap();
        let tests: Vec<_> = alpha_tests(&g).into_iter().filter(|t| t.word == 0).collect();
        assert_eq!(tests.len(), 1, "{compiler}: {tests:?}");
        let tested = &tests[0].tested;
        let name_of = |resource| {
            g.interface
                .resources
                .iter()
                .find(|r| r.id == resource)
                .map(|r| r.provenance.name.clone())
                .unwrap_or_default()
        };
        let solve = |dither: f64, scale: Option<f64>| {
            tested.linear(&mut |leaf| match leaf {
                AlphaExpr::Texel { resource, .. } if name_of(*resource) == "dither_map" => {
                    Known::Value(dither)
                }
                AlphaExpr::Texel { .. } => Known::Varies,
                AlphaExpr::Word { buffer: b, word: 1 } if *b == buffer => {
                    scale.map_or(Known::Unknown, Known::Value)
                }
                _ => Known::Unknown,
            })
        };
        let neutral = solve(0.5, Some(0.25)).expect(compiler);
        assert_eq!((neutral.scale, neutral.offset), (1., 0.), "{compiler}");
        assert_eq!(neutral.cutoff(0.4).unwrap(), Some(0.4), "{compiler}");
        let bright = solve(1., Some(0.25)).expect(compiler);
        assert!(
            (bright.cutoff(0.4).unwrap().unwrap() - 0.275).abs() < 1e-12,
            "{compiler}"
        );
        // whether the test moves with the noise is whether another answer changes the folded expression: scale 0 it does
        // not (exact, uncounted), scale 0.5 it does (a hard cutoff replaces a stochastic test)
        let moves = |scale: f64| {
            let (a, b) = (solve(0.5, Some(scale)).unwrap(), solve(1.5, Some(scale)).unwrap());
            (a.scale, a.offset) != (b.scale, b.offset)
        };
        assert!(!moves(0.), "{compiler}");
        assert!(moves(0.5), "{compiler}");
        let why = solve(0.5, None).expect_err(compiler);
        assert!(why.contains("is not stated"), "{compiler}: {why}");
    }
}
