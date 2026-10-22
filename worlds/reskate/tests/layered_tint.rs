//! `lower-layered-reskate` then `lower-tint-reskate`: the materials the first makes from a blend take `COLOR_0`
//! as their layers do, so the tint never reads a blend's weights (or a ramp the source ignores) as a colour.
//!
//! Hand-worked fixture, the shape of a Tex2 wall strip (two triangles over four vertices): `COLOR_0` is a
//! dark-to-white ramp whose first vertex of the first triangle is black, its alpha is the weight of layer 1 (baked
//! to `COLOR_1` as the pipeline's bake-layer-channels leaves it).
//!
//! | vertex | COLOR_0 (u8)       | COLOR_1 (slot weights) |
//! |--------|--------------------|------------------------|
//! | 0      | 0, 0, 0, 255       | 0, 1                   |
//! | 1      | 128, 128, 128, 255 | 0, 1                   |
//! | 2      | 255, 255, 255, 255 | 0, 1                   |
//! | 3      | 64, 64, 64, 255    | 0.5, 0.5               |
//!
//! Triangle 0 = (0, 1, 2) starts at the black vertex, triangle 1 = (3, 2, 1) at the 64 one.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{
    Entity, Layer, LayerWeights, LayeredMaterial, Mesh, Primitive, Semantic, Stream, Topology, VertexColor,
    WeightMapping, World, uses_color0,
};
use primase_reskate::components::{MaterialClass, SkateMaterial, SkateScene};
use primase_reskate::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::tint::LowerTintReskate;

fn material(key: &str) -> SkateMaterial {
    SkateMaterial {
        key: key.into(),
        class: MaterialClass::Opaque,
        base: None,
        normal: None,
        mask: None,
        base_color: Some([200, 200, 200, 255]),
        tint: None,
        alpha: None,
        cutoff: None,
        two_sided: None,
        domain: None,
        metallic: None,
        roughness: None,
        wrap: None,
        ocean: None,
        cast_shadows: None,
        emissive: None,
        vertex_alpha: None,
        other: Default::default(),
    }
}

/// The strip: a blend of two layers (`layer1_ignores`: the second layer's `core.vertex_color`; the first is
/// Ignored, as lower-anvil-material states every Tex2 layer) with the weights in `weights` (`COLOR_1` as baked, or
/// `COLOR_0` itself).
fn strip(layer1_ignores: bool, weights: Semantic) -> (World, Entity) {
    strip_of([true, layer1_ignores], weights)
}

/// [`strip`] with each layer's mark given.
fn strip_of(ignores: [bool; 2], weights: Semantic) -> (World, Entity) {
    let mut w = World::new();
    let layers: Vec<Option<Layer>> = [("a", ignores[0]), ("b", ignores[1])]
        .into_iter()
        .map(|(key, ignores)| {
            let e = w.spawn();
            w.insert(e, material(key));
            if ignores {
                w.insert(e, VertexColor::Ignored);
            }
            Some(Layer {
                material: e,
                texcoord: None,
                transform: None,
            })
        })
        .collect();
    let blend = w.spawn();
    w.insert(
        blend,
        LayeredMaterial {
            layers,
            weights: LayerWeights::new(weights, WeightMapping::Identity),
        },
    );
    w.insert(blend, material("blend"));
    w.insert(blend, VertexColor::Ignored);
    let colour0 = Stream::U8x4(vec![
        [0, 0, 0, 255],
        [128, 128, 128, 255],
        [255, 255, 255, 255],
        [64, 64, 64, 255],
    ]);
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]]),
        ),
        (Semantic::Color(0), colour0),
    ]);
    if weights == Semantic::Color(1) {
        streams.insert(
            Semantic::Color(1),
            Stream::F32x4(vec![
                [0., 1., 0., 0.],
                [0., 1., 0., 0.],
                [0., 1., 0., 0.],
                [0.5, 0.5, 0., 0.],
            ]),
        );
    }
    let mesh = w.spawn();
    w.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2, 3, 2, 1]),
            material: Some(blend),
        }]),
    );
    (w, mesh)
}

/// The materials the primitives draw, as (key, takes COLOR_0, tint).
fn drawn(w: &World, mesh: Entity) -> Vec<(String, bool, Option<[u8; 3]>)> {
    w.get::<Mesh>(mesh)
        .unwrap()
        .primitives()
        .iter()
        .map(|p| {
            let m = p.material.unwrap();
            let s = w.get::<SkateMaterial>(m).unwrap();
            (s.key.clone(), uses_color0(w, m), s.tint)
        })
        .collect()
}

/// Both passes through the pass manager, so what each adds, changes and removes is held to what it declares.
fn run(w: &mut World) -> (PassLog, PassLog) {
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerLayeredReskate::new(OverlayLimit::All)),
            Box::new(LowerTintReskate::uncapped()),
        ],
    };
    // the scene a read map has (the passes require it)
    w.insert_resource(SkateScene::default());
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .unwrap();
    let [blend, tint] = [0, 1].map(|i| report.steps[i].log.clone());
    (blend, tint)
}

/// Ignored layers (a Tex2 Material, Vertex Color Intensity 0 or not lowered): the overlay copy is Ignored too, so
/// the black-first-vertex triangle is not tinted black and the ramp does not split the overlay: the same two
/// primitives (base, overlay) before and after the tint, no tinted copy, no tint set.
#[test]
fn an_overlay_of_an_ignored_layer_is_not_tinted_by_the_ramp() {
    let (mut w, mesh) = strip(true, Semantic::Color(1));
    let (blend, tint) = run(&mut w);
    assert_eq!(blend.counts["overlay primitives"], 1);
    assert_eq!(
        blend.counts["overlay materials that do not take COLOR_0 as a colour (their layer's, black, or the weights are in it)"],
        1
    );
    // the base is layer 0's own material
    assert_eq!(
        drawn(&w, mesh),
        [
            ("a".to_owned(), false, None),
            ("blend~overlay1".to_owned(), false, None)
        ]
    );
    assert!(
        !tint.counts.contains_key("tinted material copies"),
        "{:?}",
        tint.counts
    );
    assert!(!tint.counts.contains_key("tinted primitives split by colour"));
    assert_eq!(
        tint.counts["tinted primitives whose materials ignore COLOR_0 (core.vertex_color), not tinted"],
        2
    );
}

/// The control: the layer states nothing (it takes `COLOR_0` as a colour), so its overlay copy does and the tint
/// reads the ramp: triangle 0 (first vertex black) is drawn with a black tint, triangle 1 (first vertex 64/255,
/// linear 0.251, sRGB byte 137: 1.055 x 0.251^(1/2.4) - 0.055 = 0.538, x 255 = 137.2) with that one.
#[test]
fn an_overlay_of_a_colour_layer_is_tinted_by_the_ramp() {
    let (mut w, mesh) = strip(false, Semantic::Color(1));
    let (_, tint) = run(&mut w);
    assert_eq!(tint.counts["tinted primitives split by colour"], 1);
    let tints: Vec<Option<[u8; 3]>> = drawn(&w, mesh)
        .into_iter()
        .filter(|(k, ..)| k.starts_with("blend~overlay1"))
        .map(|(.., t)| t)
        .collect();
    assert_eq!(tints, [Some([0, 0, 0]), Some([137, 137, 137])]);
}

/// Weights in `COLOR_0` itself (the alpha of a colour stream): it is not a colour then, so every material made
/// from the blend ignores it, even a layer that states nothing.
#[test]
fn weights_in_color0_are_never_a_colour() {
    let (mut w, mesh) = strip(false, Semantic::Color(0));
    let (_, tint) = run(&mut w);
    for (key, takes, tint) in drawn(&w, mesh) {
        assert!(!takes, "{key} takes COLOR_0 as a colour");
        assert_eq!(tint, None, "{key}");
    }
    assert!(
        !tint.counts.contains_key("tinted material copies"),
        "{:?}",
        tint.counts
    );
}

/// A null first slot is a black base, a weight sum below one a black darkening overlay: neither has a colour to
/// tint, so both are Ignored.
#[test]
fn black_layers_ignore_color0() {
    let (mut w, mesh) = strip(false, Semantic::Color(1));
    let blend = w.get::<Mesh>(mesh).unwrap().primitives()[0].material.unwrap();
    // the weights sum to 0.4 at every vertex: the darkening overlay draws (0.6 of black); slot 0 is null
    w.get_mut::<LayeredMaterial>(blend).unwrap().layers[0] = None;
    let weights = Stream::F32x4(vec![[0., 0.4, 0., 0.]; 4]);
    w.get_mut::<Mesh>(mesh)
        .unwrap()
        .primitive_mut(0)
        .unwrap()
        .streams_mut()
        .insert(Semantic::Color(1), weights);
    let (blend, _) = run(&mut w);
    assert_eq!(
        blend.counts["black base materials that do not take COLOR_0 as a colour"],
        1
    );
    let keys = drawn(&w, mesh);
    assert!(keys.iter().any(|(k, ..)| k == "blend~base"), "{keys:?}");
    assert!(keys.iter().any(|(k, ..)| k.ends_with("~darkening")), "{keys:?}");
    for (key, takes, tint) in keys.iter().filter(|(k, ..)| !k.starts_with("blend~overlay1")) {
        assert!(!takes && tint.is_none(), "{key}");
    }
}

/// A base layer that takes `COLOR_0` as a colour, with the weights in `COLOR_0`: the blend draws a copy of it that
/// does not (`a~base`), made once; the original is not changed, so an ordinary primitive that draws it and another
/// blend (weights in `COLOR_1`) that has it as its base still take `COLOR_0`: the ordinary one is split by the
/// ramp as before (black first vertex: tint [0, 0, 0]), the other blend's base is the original.
#[test]
fn a_shared_base_layer_keeps_its_colour_where_it_is_drawn_otherwise() {
    let (mut w, mesh) = strip_of([false, true], Semantic::Color(0));
    let blend = w.get::<Mesh>(mesh).unwrap().primitives()[0].material.unwrap();
    let a = w.get::<LayeredMaterial>(blend).unwrap().layers[0]
        .as_ref()
        .unwrap()
        .material;
    let b = w.get::<LayeredMaterial>(blend).unwrap().layers[1]
        .as_ref()
        .unwrap()
        .material;
    // the same quad drawn by `a` itself
    let mut plain = w.get::<Mesh>(mesh).unwrap().clone();
    *plain.primitive_mut(0).unwrap().material_mut() = Some(a);
    let ordinary = w.spawn();
    w.insert(ordinary, plain);
    // another blend, `a` over `b`, weights in COLOR_1
    let (other_w, other_mesh) = strip_of([false, true], Semantic::Color(1));
    let other_prim = other_w.get::<Mesh>(other_mesh).unwrap().primitives()[0].clone();
    let other_blend = w.spawn();
    let layers = vec![
        Some(Layer {
            material: a,
            texcoord: None,
            transform: None,
        }),
        Some(Layer {
            material: b,
            texcoord: None,
            transform: None,
        }),
    ];
    w.insert(
        other_blend,
        LayeredMaterial {
            layers,
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    w.insert(other_blend, material("other"));
    w.insert(other_blend, VertexColor::Ignored);
    let mut p = other_prim;
    p.material = Some(other_blend);
    let other = w.spawn();
    w.insert(other, Mesh::new(vec![p]));

    let (blend_log, _) = run(&mut w);
    assert_eq!(
        blend_log.counts["base material copies that do not take COLOR_0 as a colour (the original takes it, the weights are in it)"],
        1
    );
    // the original is untouched
    assert!(uses_color0(&w, a));
    assert!(w.get::<VertexColor>(a).is_none());
    // the blend draws the copy
    let drawn_keys: Vec<(String, bool)> = drawn(&w, mesh).into_iter().map(|(k, c, _)| (k, c)).collect();
    assert_eq!(drawn_keys[0], ("a~base".to_owned(), false), "{drawn_keys:?}");
    // the ordinary primitive was split by the ramp: a tinted copy of `a` per colour, black among them
    let tints: Vec<Option<[u8; 3]>> = drawn(&w, ordinary).into_iter().map(|(.., t)| t).collect();
    assert!(tints.contains(&Some([0, 0, 0])), "{tints:?}");
    // the other blend's base is `a` itself (or its tinted copies, as it takes COLOR_0), never the ignoring copy
    let keys: Vec<String> = drawn(&w, other).into_iter().map(|(k, ..)| k).collect();
    assert!(keys.iter().all(|k| k != "a~base"), "{keys:?}");
    assert!(keys.iter().any(|k| k.starts_with('a')), "{keys:?}");
}
