//! Per-pixel layer weights baked at the vertices for targets that blend by vertex weights, against hand-worked values.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::layered::{Channel, Composition, Source};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Layer, LayerWeights, LayeredMaterial, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole,
    Texture, Topology, WeightMapping, World,
};

/// A 2 x 1 mask (red 0 on the left texel, 255 on the right) and a three-vertex primitive whose UVs sit on the left
/// texel's centre (u 0.25), the right one's (u 0.75) and between them (u 0.5, the bilinear mean), with COLOR_0 red
/// 0.2, 0.4, 0.6.
fn world(composition: Composition) -> (World, primase_core::Entity, primase_core::Entity) {
    let mut w = World::new();
    let mask = w.spawn();
    w.insert(
        mask,
        Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![0, 0, 0, 255, 255, 0, 0, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    );
    let (a, b) = (w.spawn(), w.spawn());
    w.insert(a, PbrMaterial::default());
    w.insert(b, PbrMaterial::default());
    let host = w.spawn();
    let layer = |m| {
        Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        })
    };
    w.insert(
        host,
        LayeredMaterial {
            layers: vec![layer(a), layer(b)],
            weights: LayerWeights {
                channels: Some(vec![
                    Channel::Source(Source::Vertex {
                        semantic: Semantic::Color(0),
                        channel: 0,
                    }),
                    Channel::Source(Source::Mask {
                        texture: TextureRef::new(mask),
                        channels: [1.0, 0.0, 0.0, 0.0],
                        bias: 0.0,
                    }),
                ]),
                composition,
                ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
            },
        },
    );
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh {
            primitives: vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.25, 0.5], [0.75, 0.5], [0.5, 0.5]]),
                    ),
                    (
                        Semantic::Color(0),
                        Stream::F32x4(vec![[0.2, 0., 0., 1.], [0.4, 0., 0., 1.], [0.6, 0., 0., 1.]]),
                    ),
                ]),
                indices: Some(vec![0, 1, 2]),
                material: Some(host),
            }],
        },
    );
    (w, host, proto)
}

fn bake(w: &mut World) -> RunReport {
    let mut reg = PassRegistry::new();
    primase_image::register(&mut reg);
    let pipeline = reg.parse("bake-layer-channels").unwrap();
    let mut report = RunReport::default();
    PassManager::new(reg).run(w, &pipeline, &mut report).unwrap();
    report
}

#[test]
fn channels_are_evaluated_at_each_vertex_into_a_new_colour_stream() {
    let (mut w, host, proto) = world(Composition::Sum);
    let report = bake(&mut w);
    let l = w.get::<LayeredMaterial>(host).unwrap();
    assert!(!l.weights.per_pixel());
    assert_eq!(l.weights.attribute, Semantic::Color(1), "COLOR_0 is taken");
    assert_eq!(l.weights.mapping, WeightMapping::Identity);
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives[0]
        .streams
        .get(&Semantic::Color(1))
    else {
        panic!("no baked stream")
    };
    assert_eq!(
        baked,
        &vec![[0.2, 0., 0., 0.], [0.4, 1., 0., 0.], [0.6, 0.5, 0., 0.]]
    );
    assert_eq!(
        report.steps[0].log.approximations["primitives with layer weights baked at their vertices"],
        1
    );
}

#[test]
fn over_is_baked_to_summed_slot_weights() {
    let (mut w, host, proto) = world(Composition::Over);
    bake(&mut w);
    let l = w.get::<LayeredMaterial>(host).unwrap();
    assert_eq!(
        (l.weights.composition, l.weights.mapping),
        (Composition::Sum, WeightMapping::Identity)
    );
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives[0]
        .streams
        .get(&Semantic::Color(1))
    else {
        panic!("no baked stream")
    };
    // slot 1's weight is the mask: slot 0 keeps 1 - mask
    assert_eq!(
        baked,
        &vec![[1., 0., 0., 0.], [0., 1., 0., 0.], [0.5, 0.5, 0., 0.]]
    );
}
