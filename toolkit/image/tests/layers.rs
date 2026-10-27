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
    use iridia_interface::{Filter, Filters};
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

/// A decal-style layered material: two layers, the second drawing `alpha_texture` (a 2 x 1 texture whose alpha is 0 on
/// the left texel and 1 on the right), weighted by the vertex red times that texture's alpha. COLOR_0 red is 0.2, 0.4,
/// 0.5 at u 0.25, 0.75, 0.5, where the alpha is 0, 1, 0.5.
fn decal_world(own_alpha: bool, scale: f32) -> (World, primase_core::Entity, primase_core::Entity) {
    let (mut w, host, proto) = world(Composition::Over);
    let alpha_texture = w.spawn();
    w.insert(
        alpha_texture,
        Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255, 255, 255, 0, 255, 255, 255, 255]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
    // the second layer's material draws the alpha texture, or (not its own alpha) another texture
    let drawn = if own_alpha { alpha_texture } else { w.spawn() };
    let layer_material = l.layers[1].as_ref().unwrap().material;
    w.insert(
        layer_material,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(drawn)),
            ..PbrMaterial::default()
        },
    );
    l.weights.channels = Some(vec![
        Channel::Source(Source::Mask {
            texture: TextureRef::new(alpha_texture),
            channels: [0.; 4],
            bias: 1.0,
        }),
        Channel::Product {
            sources: vec![
                Source::Vertex {
                    semantic: Semantic::Color(0),
                    channel: 0,
                },
                Source::Mask {
                    texture: TextureRef::new(alpha_texture),
                    channels: [0., 0., 0., scale],
                    bias: 0.,
                },
            ],
        },
    ]);
    w.insert(host, l);
    let m = w.get_mut::<Mesh>(proto).unwrap();
    if let Some(Stream::F32x4(c)) = m
        .primitive_mut(0)
        .unwrap()
        .streams_mut()
        .get_mut(&Semantic::Color(0))
    {
        c[2] = [0.5, 0., 0., 1.];
    }
    (w, host, proto)
}

fn bake_with(w: &mut World, text: &str) -> Vec<[f32; 4]> {
    let mut reg = PassRegistry::new();
    primase_image::register(&mut reg);
    let pipeline = reg.parse(text).unwrap();
    PassManager::new(reg)
        .run(w, &pipeline, &mut RunReport::default())
        .unwrap();
    let proto = w.iter::<Mesh>().next().unwrap().0;
    let Some(Stream::F32x4(baked)) = w.get::<Mesh>(proto).unwrap().primitives()[0]
        .streams
        .get(&Semantic::Color(1))
    else {
        panic!("no baked stream")
    };
    baked.clone()
}

fn close(a: &[[f32; 4]], b: &[[f32; 4]]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        for k in 0..4 {
            assert!((x[k] - y[k]).abs() < 1e-6, "{a:?} vs {b:?}");
        }
    }
}

/// The overlay's own texture alpha is applied per pixel by the consumer, so the vertex weight is the red alone: 0.5 at
/// the vertex where the red is 0.5 and the alpha 0.5, not 0.25. Over composition: slot 0 keeps one minus it.
#[test]
fn a_layers_own_texture_alpha_is_left_out_of_its_baked_weight_with_layer_alpha_pixel() {
    let (mut w, _, _) = decal_world(true, 1.0);
    close(
        &bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}"),
        &[[0.8, 0.2, 0., 0.], [0.6, 0.4, 0., 0.], [0.5, 0.5, 0., 0.]],
    );
}

/// The default is the original behaviour: the alpha is sampled at the vertex and multiplied in (0.5 x 0.5 = 0.25).
#[test]
fn a_layers_own_texture_alpha_is_baked_at_the_vertices_by_default() {
    let (mut w, _, _) = decal_world(true, 1.0);
    close(
        &bake_with(&mut w, "bake-layer-channels"),
        &[[1., 0., 0., 0.], [0.6, 0.4, 0., 0.], [0.75, 0.25, 0., 0.]],
    );
    let (mut w, _, _) = decal_world(true, 1.0);
    close(
        &bake_with(&mut w, "bake-layer-channels{layer-alpha=baked}"),
        &[[1., 0., 0., 0.], [0.6, 0.4, 0., 0.], [0.75, 0.25, 0., 0.]],
    );
}

/// What the factor carries besides the texture's alpha stays: a modulation alpha of 0.5 halves the weight.
#[test]
fn the_factors_scalar_stays_in_the_weight() {
    let (mut w, _, _) = decal_world(true, 0.5);
    close(
        &bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}"),
        &[[0.9, 0.1, 0., 0.], [0.8, 0.2, 0., 0.], [0.75, 0.25, 0., 0.]],
    );
}

/// An alpha that is not the layer's own (another texture's, or one on a layer that draws none) is a mask, whatever the
/// option: the baked weights are those of the default.
#[test]
fn an_alpha_that_is_not_the_layers_own_is_baked_with_layer_alpha_pixel() {
    let (mut w, _, _) = decal_world(false, 1.0);
    close(
        &bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}"),
        &[[1., 0., 0., 0.], [0.6, 0.4, 0., 0.], [0.75, 0.25, 0., 0.]],
    );
}

/// Channels that are not a product with the layer's own alpha (a plain mask, a vertex weight) are untouched.
#[test]
fn other_channels_are_unchanged_with_layer_alpha_pixel() {
    let (mut w, _, _) = world(Composition::Sum);
    let pixel = bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}");
    close(
        &pixel,
        &[[0.2, 0., 0., 0.], [0.4, 1., 0., 0.], [0.6, 0.5, 0., 0.]],
    );
}

/// A layer with no vertex weight or mask has the alpha alone as its channel (Shaun White's material without vertex
/// colour): the weight is then 1 (held one 512th short, see the next test) times the scalar, whatever the texture
/// holds; baked, it is the alpha at the vertex.
#[test]
fn a_channel_that_is_the_layers_own_alpha_alone_becomes_its_scalar() {
    for (scale, pixel, baked) in [
        (
            1.0,
            [1.0 / 512.0, 1.0 - 1.0 / 512.0],
            [[1., 0., 0., 0.], [0., 1., 0., 0.], [0.5, 0.5, 0., 0.]],
        ),
        (
            0.5,
            [0.5, 0.5],
            [[1., 0., 0., 0.], [0.5, 0.5, 0., 0.], [0.75, 0.25, 0., 0.]],
        ),
    ] {
        let alone = |w: &mut World, host| {
            let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
            let Some(Channel::Product { sources }) =
                l.weights.channels.as_ref().and_then(|c| c.get(1)).cloned()
            else {
                panic!("decal fixture")
            };
            l.weights.channels.as_mut().unwrap()[1] = Channel::Source(sources[1].clone());
            w.insert(host, l);
        };
        let (mut w, host, _) = decal_world(true, scale);
        alone(&mut w, host);
        let out = bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}");
        close(&out, &[[pixel[0], pixel[1], 0., 0.]; 3]);
        let (mut w, host, _) = decal_world(true, scale);
        alone(&mut w, host);
        close(&bake_with(&mut w, "bake-layer-channels"), &baked);
    }
}

/// A later layer whose alpha is left out is a coverage of (nearly) one that the consumer's pixel alpha thins: the layer
/// under it keeps its weight (its overlay opacity is its own coverage, the vertex red) instead of summing to zero.
#[test]
fn a_later_layer_with_its_alpha_left_out_does_not_zero_the_layers_under_it() {
    let (mut w, host, proto) = decal_world(true, 1.0);
    let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
    let alpha_texture = l.layers[1]
        .as_ref()
        .and_then(|y| w.get::<PbrMaterial>(y.material))
        .and_then(|m| m.base_color_texture)
        .unwrap()
        .texture;
    let third = w.spawn();
    w.insert(
        third,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(alpha_texture)),
            ..PbrMaterial::default()
        },
    );
    l.layers.push(Some(Layer {
        material: third,
        texcoord: None,
        transform: None,
    }));
    l.weights
        .channels
        .as_mut()
        .unwrap()
        .push(Channel::Source(Source::Mask {
            texture: TextureRef::new(alpha_texture),
            channels: [0., 0., 0., 1.],
            bias: 0.,
        }));
    w.insert(host, l);
    let _ = proto;
    let baked = bake_with(&mut w, "bake-layer-channels{layer-alpha=pixel}");
    let red = [0.2, 0.4, 0.5];
    for (v, w) in baked.iter().enumerate() {
        // overlay opacities as the lowering reads them: weight over the weights up to it
        let under_one = w[0] + w[1];
        assert!((w[1] / under_one - red[v]).abs() < 1e-3, "vertex {v}: {w:?}");
        assert!(w[2] > 0.99, "vertex {v}: {w:?}");
    }
}
