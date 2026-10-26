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
        Mesh::new(vec![Primitive {
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
        }]),
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
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives()[0]
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
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives()[0]
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

/// A mask whose reference states a nearest magnification filter is read at the nearest texel: u 0.5 is texel
/// floor(0.5 * 2) = 1, the right one (red 255), where bilinear gives the mean.
#[test]
fn a_stated_nearest_filter_samples_the_nearest_texel() {
    use primase_core::pbr::{Filter, Filters};
    let (mut w, host, proto) = world(Composition::Sum);
    let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
    if let Some(Channel::Source(Source::Mask { texture, .. })) =
        l.weights.channels.as_mut().unwrap().get_mut(1)
    {
        texture.filters = Filters {
            mag: Some(Filter::Nearest),
            ..Filters::default()
        };
    }
    w.insert(host, l);
    bake(&mut w);
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives()[0]
        .streams
        .get(&Semantic::Color(1))
    else {
        panic!("no baked stream")
    };
    assert_eq!(
        baked,
        &vec![[0.2, 0., 0., 0.], [0.4, 1., 0., 0.], [0.6, 1., 0., 0.]]
    );
}

/// A texture coordinate past the integer range (finite, so the world verifies) samples a texel by the wrap mode; the
/// bilinear neighbour of the last integer used to overflow.
#[test]
fn a_texture_coordinate_past_the_integer_range_samples_a_texel() {
    for uv in [[1e30f32, 0.5], [0.5, -1e30], [f32::MAX, f32::MAX]] {
        let (mut w, host, proto) = world(Composition::Sum);
        let m = w.get_mut::<Mesh>(proto).unwrap();
        if let Some(Stream::F32x2(coordinates)) = m
            .primitive_mut(0)
            .unwrap()
            .streams_mut()
            .get_mut(&Semantic::TexCoord(0))
        {
            coordinates[1] = uv;
        }
        bake(&mut w);
        let l = w.get::<LayeredMaterial>(host).unwrap();
        assert!(!l.weights.per_pixel(), "{uv:?}");
    }
}

/// At the three vertices: 0.2*0*0.5 = 0, 0.4*1*0.5 = 0.2, 0.6*0.5*0.5 = 0.15. The third factor samples alpha
/// independently; all factors are evaluated before the shared baker writes its explicitly approximate vertex weights.
#[test]
fn a_three_source_product_is_baked_without_dropping_its_last_mask() {
    let (mut w, host, proto) = world(Composition::Sum);
    let weights = &mut w.get_mut::<LayeredMaterial>(host).unwrap().weights;
    let channels = weights.channels.as_mut().unwrap();
    let Channel::Source(mask) = channels[1].clone() else {
        panic!("mask fixture")
    };
    let Source::Mask { texture, .. } = &mask else {
        panic!("mask fixture")
    };
    channels[1] = Channel::Product {
        sources: vec![
            Source::Vertex {
                semantic: Semantic::Color(0),
                channel: 0,
            },
            mask.clone(),
            Source::Mask {
                texture: *texture,
                channels: [0., 0., 0., 0.5],
                bias: 0.,
            },
        ],
    };
    bake(&mut w);
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives()[0]
        .streams
        .get(&Semantic::Color(1))
    else {
        panic!("baked stream")
    };
    assert_eq!(
        baked,
        &vec![[0.2, 0., 0., 0.], [0.4, 0.2, 0., 0.], [0.6, 0.15, 0., 0.]]
    );
}
