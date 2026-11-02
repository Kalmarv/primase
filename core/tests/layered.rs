use primase_core::layered::{Channel, Composition, Source, mask_value, over_as_sum};
use primase_core::pbr::TextureRef;
use primase_core::{Component, Layer, LayerWeights, LayeredMaterial, Semantic, WeightMapping, World};

#[test]
fn pixel_mapping_keeps_darkening_and_interpolates_channels_before_products() {
    let map = WeightMapping::RgbProduct4;
    assert_eq!(map.evaluate([0.5, 0.5, 0.5, 1.]), [0.125; 4]);
    assert_eq!(map.evaluate([1., 0.5, 0.5, 0.]), [0.; 4]);
    let ends = [map.evaluate([0., 0., 0., 1.]), map.evaluate([0., 1., 1., 1.])];
    let interpolated_products: [f32; 4] = std::array::from_fn(|i| (ends[0][i] + ends[1][i]) / 2.);
    assert_ne!(map.evaluate([0., 0.5, 0.5, 1.]), interpolated_products);
    assert_eq!(
        WeightMapping::Identity.evaluate([0.2, 0.3, 0.1, 0.]),
        [0.2, 0.3, 0.1, 0.]
    );
}

#[test]
fn verifies_null_slots_references_and_nonrecursive_materials() {
    let mut world = World::default();
    let ordinary = world.spawn();
    let entity = world.spawn();
    let mut material = LayeredMaterial {
        layers: vec![
            Some(Layer {
                material: ordinary,
                texcoord: Some(1),
                transform: None,
            }),
            None,
            None,
            None,
        ],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
    };
    assert!(material.verify(entity, &world).is_ok());
    world.insert(entity, material.clone());
    assert!(world.closure([entity]).contains(&ordinary));
    world.insert(ordinary, material.clone());
    assert!(material.verify(entity, &world).is_err());
    world.despawn(ordinary);
    // a layer whose material is gone: the world's reference check, not the material's own
    assert!(material.verify(entity, &world).is_ok());
    assert_eq!(
        world.verify_components(),
        [(
            "core.layered",
            entity,
            format!("refers to {ordinary:?}, which is not alive")
        )]
    );
    material.layers[0] = None;
    assert!(material.verify(entity, &world).is_ok());
    material.layers.pop();
    assert!(material.verify(entity, &world).is_err());
    material.weights.mapping = WeightMapping::Identity;
    assert!(material.verify(entity, &world).is_ok());
    material.layers.clear();
    assert!(material.verify(entity, &world).is_err());
}

/// Worked by hand: Sekiro's threshold k = (0, s, -s, s) with s = 2 at v = 0.5, m = 0.25 is 1 - 0.5 + 0.25 = 0.75;
/// past 1 it saturates; a lerp of 0.2 to 0.6 at 0.25 is 0.3; a mask of rgba (1, 0.5, 0, 0) by (0.5, 0.5, 0, 0) and
/// bias -0.1 is 0.65; over with slot weights (_, 0.5, 0.25) sums as (0.375, 0.375, 0.25).
#[test]
fn channels_evaluate_by_hand() {
    let v = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 0,
    };
    let t = Channel::Threshold {
        v: v.clone(),
        m: v.clone(),
        k: [0., 2., -2., 2.],
    };
    assert_eq!(t.evaluate(&[0.5, 0.25]), 0.75);
    assert_eq!(t.evaluate(&[1.0, 0.0]), 1.0);
    let l = Channel::Lerp {
        a: v.clone(),
        b: v.clone(),
        t: v,
    };
    assert!((l.evaluate(&[0.2, 0.6, 0.25]) - 0.3).abs() < 1e-7);
    assert!((mask_value([0.5, 0.5, 0., 0.], -0.1, [1., 0.5, 0., 0.]) - 0.65).abs() < 1e-7);
    assert_eq!(over_as_sum([0., 0.5, 0.25, 0.], 3), [0.375, 0.375, 0.25, 0.]);
    // no more slots than the four there are
    assert_eq!(over_as_sum([0.5; 4], 9), over_as_sum([0.5; 4], 4));
}

#[test]
fn channels_match_the_mapping_and_their_masks_are_references() {
    let mut world = World::default();
    let (a, b, mask, e) = (world.spawn(), world.spawn(), world.spawn(), world.spawn());
    let layer = |m| {
        Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        })
    };
    let masked = Channel::Source(Source::Mask {
        texture: TextureRef::new(mask),
        channels: [1., 0., 0., 0.],
        bias: 0.,
    });
    let mut material = LayeredMaterial {
        layers: vec![layer(a), layer(b)],
        weights: LayerWeights {
            channels: Some(vec![masked.clone(), masked.clone()]),
            composition: Composition::Over,
            ..LayerWeights::new(Semantic::Color(1), WeightMapping::Identity)
        },
    };
    assert!(material.weights.per_pixel());
    assert_eq!(material.verify(e, &world), Ok(()));
    world.insert(e, material.clone());
    assert!(
        world.closure([e]).contains(&mask),
        "a mask is part of its material"
    );
    // one channel for two slots
    material.weights.channels = Some(vec![masked.clone()]);
    assert_eq!(
        material.verify(e, &world),
        Err("1 channels for a mapping of 2".into())
    );
    // a vertex channel past alpha, a threshold that is not finite
    let past = Channel::Source(Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 4,
    });
    material.weights.channels = Some(vec![masked.clone(), past]);
    assert_eq!(
        material.verify(e, &world),
        Err("channel 1: vertex channel 4 past 3".into())
    );
    let v = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 0,
    };
    let nan = Channel::Threshold {
        v: v.clone(),
        m: v,
        k: [f32::NAN, 0., 0., 0.],
    };
    material.weights.channels = Some(vec![masked, nan]);
    assert!(material.verify(e, &world).unwrap_err().contains("not finite"));
    // a mask that is gone
    world.despawn(mask);
    assert!(
        world
            .get::<LayeredMaterial>(e)
            .unwrap()
            .verify(e, &world)
            .unwrap_err()
            .contains("not alive")
    );
}

/// The height blend worked by hand from the formula (MAD_Tex2's, riders-republic docs/material-spec.md §4.3): weights
/// (0.5, 0.5), heights (0.8, 0.4), so p = (0.4, 0.2) and m = 0.4.
#[test]
fn height_blend_weights_worked_by_hand() {
    use primase_core::layered::height_blend;
    let close = |a: [f32; 4], b: [f32; 4]| (0..4).all(|i| (a[i] - b[i]).abs() < 1e-6);
    // contrast 0.5: u = (0.4 + 0.1, 0.2 + 0.1) = (0.5, 0.3), normalized (0.625, 0.375)
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 0.5);
    assert!(close(got, [0.625, 0.375, 0.0, 0.0]), "{got:?}");
    // contrast 0.9: u = (0.4 - 0.3, max(0.2 - 0.3, 0)) = (0.1, 0): the lower layer drops out
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 0.9);
    assert!(close(got, [1.0, 0.0, 0.0, 0.0]), "{got:?}");
    // contrast 1: every u is 0, no weight
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 1.0);
    assert!(close(got, [0.0; 4]), "{got:?}");
}

/// Hand-worked: vertex weight 1/2 times mask 1/4 times sampled alpha 3/4 is 3/32. Products retain signs and values
/// outside 0..1; each factor, including the last, participates in reference and source validation.
#[test]
fn products_are_bounded_ordered_and_keep_all_sources() {
    use primase_core::layered::PRODUCT_LIMIT;
    let mut w = World::new();
    let material = w.spawn();
    let host = w.spawn();
    let mask = w.spawn();
    let vertex = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 1,
    };
    let texture = Source::Mask {
        texture: TextureRef::new(mask),
        channels: [0., 0., 0., 1.],
        bias: 0.,
    };
    let channel = Channel::Product {
        sources: vec![vertex.clone(), texture.clone(), texture.clone()],
    };
    assert_eq!(channel.evaluate(&[0.5, 0.25, 0.75]), 0.09375);
    assert_eq!(channel.evaluate(&[-2., 3., 0.5]), -3.);
    assert_eq!(channel.sources(), vec![&vertex, &texture, &texture]);
    for n in [0, 1, PRODUCT_LIMIT + 1] {
        assert!(
            Channel::Product {
                sources: vec![vertex.clone(); n]
            }
            .check()
            .unwrap_err()
            .contains("product")
        );
    }
    assert!(
        Channel::Product {
            sources: vec![vertex.clone(); PRODUCT_LIMIT]
        }
        .check()
        .is_ok()
    );
    let mut layered = LayeredMaterial {
        layers: vec![Some(Layer {
            material,
            texcoord: None,
            transform: None,
        })],
        weights: LayerWeights {
            channels: Some(vec![channel]),
            ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
        },
    };
    w.insert(host, layered.clone());
    assert!(w.closure([host]).contains(&mask));
    assert!(layered.reads(Semantic::Color(0)));
    assert_eq!(layered.mask_textures().count(), 2);
    assert!(layered.verify(host, &w).is_ok());
    if let Some(channels) = &mut layered.weights.channels {
        channels[0] = Channel::Product {
            sources: vec![
                vertex.clone(),
                texture,
                Source::Vertex {
                    semantic: Semantic::Color(0),
                    channel: 4,
                },
            ],
        };
    }
    assert!(layered.verify(host, &w).unwrap_err().contains("past 3"));
    // The order is specified even when reassociating floating-point multiplication would change the result.
    let three = Channel::Product {
        sources: vec![vertex; 3],
    };
    assert_eq!(three.evaluate(&[1e20, 1e20, 1e-20]), f32::INFINITY);
}

fn colour(channel: u8) -> Source {
    Source::Vertex {
        semantic: Semantic::Color(0),
        channel,
    }
}

/// Whether two results are the same value: equal, or both NaN.
fn same_value(a: f32, b: f32) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

/// Each named form is its expression: the expression spells it back, and the two evaluate alike (to the bit, NaN
/// aside) on values that round differently, are out of range, infinite or NaN.
#[test]
fn named_forms_are_their_expressions() {
    let (a, b, c) = (colour(0), colour(1), colour(2));
    let named = [
        Channel::Source(a.clone()),
        Channel::Lerp {
            a: a.clone(),
            b: b.clone(),
            t: c.clone(),
        },
        Channel::Lerp {
            a: a.clone(),
            b: b.clone(),
            t: a.clone(),
        },
        Channel::Threshold {
            v: a.clone(),
            m: b.clone(),
            k: [0., 2., -2., 2.],
        },
        Channel::Threshold {
            v: a.clone(),
            m: b.clone(),
            k: [0.25, 1., -1., 0.],
        },
        Channel::Threshold {
            v: a.clone(),
            m: a.clone(),
            k: [0.5, 0.5, 0.5, 0.5],
        },
        Channel::Product {
            sources: vec![a.clone(), b.clone(), c.clone()],
        },
        Channel::Product {
            sources: vec![a.clone(), a.clone(), a.clone(), a.clone()],
        },
        Channel::Power {
            v: a.clone(),
            exponent: 8,
        },
        Channel::Power {
            v: a.clone(),
            exponent: 5,
        },
    ];
    let distinct = [a, b, c];
    // the values of each of `sources` from one value per distinct source (`at`), in `sources`' order
    let values_of = |sources: &[&Source], at: &[f32]| -> Vec<f32> {
        sources
            .iter()
            .map(|s| at[distinct.iter().position(|d| d == *s).unwrap()])
            .collect()
    };
    let samples = [
        [0.5, 0.25, 0.75],
        [0.1, 0.7, 0.3],
        [1.0 / 3.0, 2.0 / 3.0, 0.9],
        [-2.0, 3.0, 1e20],
        [f32::INFINITY, 0.0, 0.5],
        [f32::NAN, 0.5, 0.5],
        [1e-20, 1e20, -0.0],
    ];
    for channel in named {
        let e = channel.expression().unwrap();
        let back = Channel::from_expression(&e).unwrap();
        assert_eq!(back, channel, "{e:?}");
        assert_eq!(back.expression().unwrap(), e);
        for at in &samples {
            let named = channel.evaluate(&values_of(&channel.sources(), at));
            let spelled = e.evaluate(&values_of(&e.sources.iter().collect::<Vec<_>>(), at));
            assert!(
                same_value(named, spelled),
                "{channel:?} at {at:?}: {named} and {spelled}"
            );
        }
    }
    // a product of two of one source is its square, and power 1 is the source
    let square = Channel::Product {
        sources: vec![colour(0); 2],
    };
    assert_eq!(
        Channel::from_expression(&square.expression().unwrap()).unwrap(),
        Channel::Power {
            v: colour(0),
            exponent: 2
        }
    );
    let one = Channel::Power {
        v: colour(0),
        exponent: 1,
    };
    assert_eq!(
        Channel::from_expression(&one.expression().unwrap()).unwrap(),
        Channel::Source(colour(0))
    );
}

/// Spellings of one computation are one expression: operands of add, multiply, min and max in either order, a value
/// spelled twice, terms the value does not read, sources given twice; a named form spelled by hand is that form.
#[test]
fn expressions_have_one_spelling() {
    use primase_core::layered::{ExpressionBuilder, Term};
    let (v, m) = (colour(0), colour(1));
    // saturate(1 - 1.4 (2 (1 - v) - m)), spelled two ways
    let spell = |swap: bool, waste: bool| {
        let mut b = ExpressionBuilder::new();
        let one = b.constant(1.0);
        let vv = b.source(v.clone());
        if waste {
            let unused = b.constant(7.0);
            b.multiply(unused, vv);
        }
        let inv = b.subtract(one, vv);
        let two = b.constant(2.0);
        let twice = if swap {
            b.multiply(inv, two)
        } else {
            b.multiply(two, inv)
        };
        let mm = b.source(m.clone());
        let d = b.subtract(twice, mm);
        let k = b.constant(1.4);
        let kd = if swap { b.multiply(d, k) } else { b.multiply(k, d) };
        let one_again = b.constant(1.0);
        let x = b.subtract(if swap { one_again } else { one }, kd);
        let w = b.saturate(x);
        b.channel(w).unwrap()
    };
    let plain = spell(false, false);
    assert_eq!(spell(true, true), plain);
    let Channel::Expression(e) = &plain else {
        panic!("not an expression: {plain:?}");
    };
    assert_eq!(e.sources, [v.clone(), m.clone()]);
    assert_eq!(
        e.terms.iter().filter(|t| matches!(t, Term::Constant(_))).count(),
        3
    );
    assert_eq!(plain.check(), Ok(()));
    // worked by hand at v = 0.25, m = 0.5: 1 - 1.4 (1.5 - 0.5) = -0.4, saturated 0
    assert_eq!(plain.evaluate(&[0.25, 0.5]), 0.0);
    // at v = 0.75, m = 0.25: 1 - 1.4 (0.5 - 0.25) = 0.65
    assert!((plain.evaluate(&[0.75, 0.25]) - 0.65).abs() < 1e-6);
    // a lerp and a square spelled by hand are the named forms
    let mut b = ExpressionBuilder::new();
    let (x, y, t) = (b.source(v.clone()), b.source(m.clone()), b.source(colour(2)));
    let d = b.subtract(y, x);
    let s = b.multiply(t, d);
    let l = b.add(s, x);
    assert_eq!(
        b.channel(l).unwrap(),
        Channel::Lerp {
            a: v.clone(),
            b: m.clone(),
            t: colour(2)
        }
    );
    let sq = b.multiply(x, x);
    assert_eq!(
        b.channel(sq).unwrap(),
        Channel::Power {
            v: v.clone(),
            exponent: 2
        }
    );
    // other operations are another computation: a + t b - t a is not the lerp
    let tb = b.multiply(t, y);
    let ta = b.multiply(t, x);
    let sum = b.add(x, tb);
    let other = b.subtract(sum, ta);
    assert!(matches!(b.channel(other).unwrap(), Channel::Expression(_)));
}

/// What [`Channel::check`] holds an expression to: its canonical spelling, no named form, finite constants, operands
/// before their term, sources that are there, at most `TERM_LIMIT` terms.
#[test]
fn expressions_are_checked() {
    use primase_core::layered::{Expression, ExpressionBuilder, TERM_LIMIT, Term};
    let (v, m) = (colour(0), colour(1));
    let difference = Expression {
        sources: vec![v.clone(), m.clone()],
        terms: vec![
            Term::Source(0),
            Term::Source(1),
            Term::Subtract([0, 1]),
            Term::Saturate(2),
        ],
    };
    assert_eq!(Channel::Expression(difference.clone()).check(), Ok(()));
    let reordered = Expression {
        sources: vec![m.clone(), v.clone()],
        terms: vec![
            Term::Source(1),
            Term::Source(0),
            Term::Subtract([0, 1]),
            Term::Saturate(2),
        ],
    };
    assert!(reordered.check().unwrap_err().contains("not canonical"));
    assert_eq!(reordered.canonical().unwrap(), difference);
    let lerp = Channel::Lerp {
        a: v.clone(),
        b: m.clone(),
        t: v.clone(),
    };
    let spelled = Channel::Expression(lerp.expression().unwrap());
    assert!(spelled.check().unwrap_err().contains("named form"));
    let bad = |terms: Vec<Term>| {
        Expression {
            sources: vec![v.clone()],
            terms,
        }
        .check()
        .unwrap_err()
    };
    assert!(bad(vec![Term::Source(0), Term::Constant(f32::NAN), Term::Add([0, 1])]).contains("not finite"));
    assert!(bad(vec![Term::Source(0), Term::Add([0, 1])]).contains("not before it"));
    assert!(bad(vec![Term::Source(1)]).contains("source 1 of 1"));
    assert!(bad(vec![]).contains("without terms"));
    let mut b = ExpressionBuilder::new();
    let mut x = b.source(v.clone());
    for i in 0..TERM_LIMIT {
        let k = b.constant(i as f32 + 2.0);
        x = b.multiply(x, k);
    }
    assert!(b.channel(x).unwrap_err().contains("past"));
}

/// The numeric rules: saturate of NaN is 0, in a threshold too; min and max of NaN give the other operand.
#[test]
fn expressions_follow_the_numeric_rules() {
    use primase_core::layered::{ExpressionBuilder, saturate};
    assert_eq!(saturate(f32::NAN), 0.0);
    assert_eq!(saturate(-1.0), 0.0);
    assert_eq!(saturate(f32::INFINITY), 1.0);
    let threshold = Channel::Threshold {
        v: colour(0),
        m: colour(1),
        k: [0., 1., 1., 0.],
    };
    assert_eq!(threshold.evaluate(&[f32::NAN, 0.5]), 0.0);
    let mut b = ExpressionBuilder::new();
    let (x, y) = (b.source(colour(0)), b.source(colour(1)));
    let lo = b.min(x, y);
    let hi = b.max(x, y);
    let both = b.add(lo, hi);
    let channel = b.channel(both).unwrap();
    assert_eq!(channel.evaluate(&[f32::NAN, 0.25]), 0.5);
    assert_eq!(channel.evaluate(&[0.75, 0.25]), 1.0);
}

/// An expression's sources are the material's: the vertex streams it reads and the masks it samples.
#[test]
fn expression_sources_are_read_and_referenced() {
    use primase_core::layered::ExpressionBuilder;
    let mut w = World::new();
    let (layer, host, mask) = (w.spawn(), w.spawn(), w.spawn());
    let mut b = ExpressionBuilder::new();
    let v = b.source(Source::Vertex {
        semantic: Semantic::Color(1),
        channel: 3,
    });
    let t = b.source(Source::Mask {
        texture: TextureRef::new(mask),
        channels: [1., 0., 0., 0.],
        bias: 0.,
    });
    let d = b.subtract(v, t);
    let s = b.saturate(d);
    let layered = LayeredMaterial {
        layers: vec![Some(Layer {
            material: layer,
            texcoord: None,
            transform: None,
        })],
        weights: LayerWeights {
            channels: Some(vec![b.channel(s).unwrap()]),
            ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
        },
    };
    assert!(layered.reads(Semantic::Color(1)));
    assert!(!layered.reads(Semantic::Color(0)));
    assert_eq!(layered.mask_textures().count(), 1);
    assert_eq!(layered.verify(host, &w), Ok(()));
    w.insert(host, layered);
    assert!(w.closure([host]).contains(&mask));
}
