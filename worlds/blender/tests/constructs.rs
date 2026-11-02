//! Oracle: Blender 5.2.3's Cycles, rendering what the three stages make of a world of one-metre quads, each drawn with
//! one construct of `pbr.material`, against values worked by hand.
//!
//! The camera is orthographic, eight pixels to the metre, each quad's pixel centres at `(i + 0.5) / 8` of its UVs
//! (`i` 0 to 7 from its left and top), the pixel filter a hundredth of a pixel wide: a pixel is its centre's value, but
//! for the few samples Cycles places up to half a pixel off ([`OFF_CENTRE`]).
//! The texture is 2 x 2 texels, sRGB bytes 0 or 255 so they decode to 0 or 1: `A` (red), `B` (green) on its top row,
//! `C` (blue), `D` (white, alpha 128/255) below. Nearest sampling makes each texel four pixels of a quad (sixteen),
//! two with a scale of 2. What each quad shows is worked out in its case's comment and its `expect`.
//!
//! Read back are the passes every material writes: its base colour, its emission and its coverage, and Cycles' image
//! for the unlit case. Needs `tools.blender`; skips without it.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeMap;

use iridia_interface::{Filter, Filters};
use primase_blender::graph::aov;
use primase_blender::render::{Camera, Device, Projection, RenderOptions, Rendered, render};
use primase_blender::save::{Blender, save};
use primase_blender::{BlenderReport, WriteOptions, write_file};
use primase_core::layered::{Channel, Composition, Source, height_blend, over_as_sum};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Layer, LayerWeights, LayeredMaterial, Mesh, Name, Primitive, Semantic, Stream,
    TexFormat, TexRole, Texture, Topology, Transform, VertexColor, WeightMapping, World,
};

/// Pixels a quad spans; pixels from one quad's corner to the next's; quads in a row.
const PX: usize = 8;
const PITCH: usize = 10;
const COLS: usize = 4;

/// The texels, `[column][row]`, as Blender decodes them (linear RGBA; alpha as stored).
fn texel(c: usize, r: usize) -> [f32; 4] {
    [
        [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]],
        [[0.0, 1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 128.0 / 255.0]],
    ][c][r]
}

/// The texture's bytes, rows from the top.
const TEXELS: [[u8; 4]; 4] = [
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
    [255, 255, 255, 128],
];

/// Samples a pixel.
const SAMPLES: u32 = 256;

/// The most a pixel's value can move from its centre's, per unit of the value's change across a pixel. Cycles 5.2.3
/// places nearly every camera sample within the filter's width of the pixel centre, but some (about 6 in 10,000, with
/// each filter type, width and sampling pattern) up to half a pixel off; a pixel is taken to have at most three of
/// its [`SAMPLES`] there (more than one in 256 samples is rare: about 0.15 a pixel).
const OFF_CENTRE: f32 = 0.5 * 3.0 / SAMPLES as f32;

/// What a pixel of a quad holds: base colour (RGB), coverage, and where stated its emission and Cycles' image.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Want {
    base: [f32; 3],
    coverage: f32,
    emission: Option<[f32; 3]>,
    combined: Option<[f32; 3]>,
    /// How far a value may be from what is worked, where the pixel's samples all read it.
    tolerance: f32,
    /// The most any value changes across a pixel (0 where it is the same across it): its samples off the centre
    /// move it by up to this times [`OFF_CENTRE`].
    slope: f32,
}

impl Want {
    fn base(base: [f32; 3]) -> Self {
        Want {
            base,
            coverage: 1.0,
            emission: None,
            combined: None,
            tolerance: 1e-5,
            slope: 0.0,
        }
    }
    fn uncovered() -> Self {
        Want {
            coverage: 0.0,
            ..Want::base([0.0; 3])
        }
    }
}

fn rgb(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]]
}

/// The texel under pixel `(i, j)` of a quad sampled nearest without a transform.
fn quadrant(i: usize, j: usize) -> [f32; 4] {
    texel(i / 4, j / 4)
}

/// Which texel a coordinate `2 (k + 0.5) / 8` (a scale of 2) falls in along one axis, by its wrap: repeat reads the
/// coordinate's fraction; clamp holds it at the last texel past 1; mirror reads `2 - x` from 1 to 2.
fn wrapped(k: usize, w: Wrap) -> usize {
    let x = 2.0 * (k as f32 + 0.5) / 8.0;
    let x = match w {
        Wrap::Repeat => x.fract(),
        Wrap::Clamp => x.min(0.99),
        Wrap::Mirror if x >= 1.0 => 2.0 - x,
        Wrap::Mirror => x,
    };
    usize::from(x >= 0.5)
}

/// The bilinear value of the texture at pixel `(i, j)` of a quad, repeating: texel centres at `(c + 0.5) / 2`, so the
/// coordinate in texels is `2 u - 0.5`, between texel `floor` and the next by its fraction (the next past 1 being
/// texel 0 again).
fn bilinear(i: usize, j: usize) -> [f32; 3] {
    let axis = |k: usize| {
        let x = 2.0 * (k as f32 + 0.5) / 8.0 - 0.5;
        let c0 = x.floor();
        (c0.rem_euclid(2.0) as usize, x - c0)
    };
    let ((c, fx), (r, fy)) = (axis(i), axis(j));
    let mut out = [0.0; 3];
    for (dc, wx) in [(0, 1.0 - fx), (1, fx)] {
        for (dr, wy) in [(0, 1.0 - fy), (1, fy)] {
            let t = texel((c + dc) % 2, (r + dr) % 2);
            for k in 0..3 {
                out[k] += t[k] * wx * wy;
            }
        }
    }
    out
}

/// sRGB's decoding of an encoded value 0 to 1.
fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// One quad: its material, how it is wound and what it carries, and what each of its pixels holds.
struct Case {
    name: &'static str,
    material: PbrMaterial,
    /// The quad faces away from the camera.
    back: bool,
    /// Streams beyond position, normal and UV set 0, a value per corner: top-left, top-right, bottom-right,
    /// bottom-left.
    streams: Vec<(Semantic, Stream)>,
    ignores_color: bool,
    /// A blend on the material, `material` its host.
    layered: Option<LayeredMaterial>,
    expect: Box<dyn Fn(usize, usize) -> Want>,
}

impl Case {
    fn new(
        name: &'static str,
        material: PbrMaterial,
        expect: impl Fn(usize, usize) -> Want + 'static,
    ) -> Self {
        Case {
            name,
            material,
            back: false,
            streams: Vec::new(),
            ignores_color: false,
            layered: None,
            expect: Box::new(expect),
        }
    }
}

/// A square texture of `size` texels, rows from the top: sRGB colour, or linear data.
fn texture(w: &mut World, name: &str, role: TexRole, pixels: &[[u8; 4]], size: u32) -> Entity {
    let (format, color_space) = match role {
        TexRole::Color => (TexFormat::Rgba8Unorm, ColorSpace::Srgb),
        _ => (TexFormat::Rgba8Unorm, ColorSpace::Linear),
    };
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        Texture {
            width: size,
            height: size,
            format,
            levels: vec![pixels.concat()],
            color_space,
            role,
            normal: None,
        },
    );
    e
}

fn slot(t: Entity, f: impl FnOnce(&mut TextureRef)) -> Option<TextureRef> {
    let mut r = TextureRef::new(t);
    // nearest unless a case says otherwise: each pixel one texel
    r.filters = Filters {
        mag: Some(Filter::Nearest),
        min: Some(Filter::Nearest),
        mip: None,
    };
    f(&mut r);
    Some(r)
}

fn white() -> PbrMaterial {
    PbrMaterial {
        metallic: 0.0,
        double_sided: true,
        ..PbrMaterial::default()
    }
}

fn scaled(wrap: [Wrap; 2]) -> impl FnOnce(&mut TextureRef) {
    move |r| {
        r.wrap = wrap;
        r.transform = Some(UvTransform {
            scale: [2.0, 2.0],
            ..UvTransform::IDENTITY
        });
    }
}

fn cases(w: &mut World) -> Vec<Case> {
    let tex = texture(w, "abcd", TexRole::Color, &TEXELS, 2);
    let data = texture(w, "abcd data", TexRole::Data, &TEXELS, 2);
    let grey = texture(w, "grey 188", TexRole::Color, &[[188, 188, 188, 255]], 1);
    let textured = |f: fn(&mut TextureRef)| PbrMaterial {
        base_color_texture: slot(tex, f),
        ..white()
    };
    let mut out = vec![
        // base colour times the texture: each quadrant its texel times (0.5, 0.25, 1)
        Case::new(
            "factor and texture",
            PbrMaterial {
                base_color: [0.5, 0.25, 1.0, 1.0],
                ..textured(|_| {})
            },
            |i, j| {
                let t = quadrant(i, j);
                Want::base([0.5 * t[0], 0.25 * t[1], t[2]])
            },
        ),
        // the offset (0.5, 0) moves the image half a quad left, repeating: B A over D C
        Case::new(
            "offset",
            textured(|r| {
                r.transform = Some(UvTransform {
                    offset: [0.5, 0.0],
                    ..UvTransform::IDENTITY
                })
            }),
            |i, j| Want::base(rgb(quadrant((i + 4) % 8, j))),
        ),
        // a quarter turn with offset (1, 0) samples (1 - v, u): the top-left quadrant reads (> 0.5, < 0.5) = B, the
        // top-right (> 0.5, > 0.5) = D, the bottom-left (< 0.5, < 0.5) = A, the bottom-right C
        Case::new(
            "rotation",
            textured(|r| {
                r.transform = Some(UvTransform {
                    offset: [1.0, 0.0],
                    scale: [1.0, 1.0],
                    rotation: std::f32::consts::FRAC_PI_2,
                })
            }),
            |i, j| Want::base(rgb(texel(usize::from(j < 4), usize::from(i >= 4)))),
        ),
    ];
    for wrap in [
        [Wrap::Repeat, Wrap::Repeat],
        [Wrap::Clamp, Wrap::Clamp],
        [Wrap::Mirror, Wrap::Mirror],
        [Wrap::Repeat, Wrap::Mirror],
        [Wrap::Clamp, Wrap::Repeat],
        [Wrap::Mirror, Wrap::Clamp],
    ] {
        // a scale of 2 shows the coordinates 0 to 2 along each axis, each axis by its own wrap ([`wrapped`])
        out.push(Case::new(
            "wrap",
            PbrMaterial {
                base_color_texture: slot(tex, scaled(wrap)),
                ..white()
            },
            move |i, j| Want::base(rgb(texel(wrapped(i, wrap[0]), wrapped(j, wrap[1])))),
        ));
    }
    let names = [
        "repeat",
        "clamp",
        "mirror",
        "repeat u, mirror v",
        "clamp u, repeat v",
        "mirror u, clamp v",
    ];
    for (case, name) in out[3..].iter_mut().zip(names) {
        case.name = name;
    }
    let mut colour = |name, stream, ignores: bool, want: [f32; 3]| {
        let mut c = Case::new(
            name,
            PbrMaterial {
                base_color: if ignores { [0.3, 0.6, 0.9, 1.0] } else { [1.0; 4] },
                ..white()
            },
            move |_, _| Want::base(want),
        );
        c.streams = vec![(Semantic::Color(0), stream)];
        c.ignores_color = ignores;
        out.push(c);
    };
    // COLOR_0 multiplies the base colour, at full float precision
    colour(
        "vertex colour f32",
        Stream::F32x4(vec![[0.25, 0.5, 0.75, 1.0]; 4]),
        false,
        [0.25, 0.5, 0.75],
    );
    // a normalized byte colour is its byte over 255
    colour(
        "vertex colour u8",
        Stream::U8x4(vec![[64, 128, 192, 255]; 4]),
        false,
        [64.0 / 255.0, 128.0 / 255.0, 192.0 / 255.0],
    );
    // a material that ignores its vertex colour draws its base colour alone
    colour(
        "vertex colour ignored",
        Stream::F32x4(vec![[0.0, 0.0, 0.0, 1.0]; 4]),
        true,
        [0.3, 0.6, 0.9],
    );
    out.extend([
        // mask at 0.6: D's alpha 128/255 is below it, so D's quadrant is not covered
        Case::new(
            "mask",
            PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: 0.6 },
                ..textured(|_| {})
            },
            |i, j| {
                if i >= 4 && j >= 4 {
                    Want::uncovered()
                } else {
                    Want::base(rgb(quadrant(i, j)))
                }
            },
        ),
        // mask at 0.5: D's alpha 128/255 is at least it, so all is covered
        Case::new(
            "mask at a half",
            PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
                ..textured(|_| {})
            },
            |i, j| Want::base(rgb(quadrant(i, j))),
        ),
        // blended, alpha 0.75 times the texel's: D's 0.376 is below the pass threshold (0.5), the rest 0.75 above
        Case::new(
            "blend",
            PbrMaterial {
                base_color: [1.0, 1.0, 1.0, 0.75],
                alpha_mode: AlphaMode::Blend,
                ..textured(|_| {})
            },
            |i, j| {
                if i >= 4 && j >= 4 {
                    Want::uncovered()
                } else {
                    Want::base(rgb(quadrant(i, j)))
                }
            },
        ),
        // emission (1, 0.5, 0.25) at strength 4 emits (4, 2, 1)
        Case::new(
            "emission",
            PbrMaterial {
                base_color: [0.0, 0.0, 0.0, 1.0],
                emissive: [1.0, 0.5, 0.25],
                emissive_strength: 4.0,
                ..white()
            },
            |_, _| Want {
                emission: Some([4.0, 2.0, 1.0]),
                ..Want::base([0.0; 3])
            },
        ),
        // unlit: the camera sees the base colour itself, and it emits nothing
        Case::new(
            "unlit",
            PbrMaterial {
                base_color: [0.25, 0.5, 0.75, 1.0],
                shading: Shading::Unlit,
                ..white()
            },
            |_, _| Want {
                emission: Some([0.0; 3]),
                combined: Some([0.25, 0.5, 0.75]),
                ..Want::base([0.25, 0.5, 0.75])
            },
        ),
        // a colour texture with no filter stated is filtered: bilinear, repeating ([`bilinear`]). Cycles filters an
        // 8-bit sRGB image's encoded values and decodes the result (the writer counts it): the texels here are 0 or 1
        // encoded and decoded alike, so the pixel is the decoded bilinear value. The encoded value moves by up to 0.25
        // a pixel (a texel is four), the decoded by up to 2.28 times that (sRGB's slope at 1).
        Case::new(
            "bilinear",
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(tex)),
                ..white()
            },
            |i, j| Want {
                slope: 0.25 * 2.28,
                ..Want::base(bilinear(i, j).map(srgb_decode))
            },
        ),
        // a data texture with no filter stated is read exactly: nearest
        Case::new(
            "data nearest",
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(data)),
                ..white()
            },
            |i, j| Want::base(rgb(quadrant(i, j))),
        ),
        // sRGB 188 decodes to ((188 / 255 + 0.055) / 1.055)^2.4 = 0.50289
        Case::new(
            "srgb",
            PbrMaterial {
                base_color_texture: slot(grey, |_| {}),
                ..white()
            },
            |_, _| {
                let v = ((188.0f32 / 255.0 + 0.055) / 1.055).powf(2.4);
                Want {
                    tolerance: 2e-3,
                    ..Want::base([v; 3])
                }
            },
        ),
    ]);
    // the slot's UV set 1 is the quad mirrored left to right: B A over D C
    let mut second = Case::new("texcoord 1", textured(|r| r.texcoord = 1), |i, j| {
        Want::base(rgb(quadrant(7 - i, j)))
    });
    second.streams = vec![(Semantic::TexCoord(1), mirrored())];
    out.push(second);
    // seen from behind, a single-sided quad is not drawn, a double-sided one is
    let mut single = Case::new(
        "back, single-sided",
        PbrMaterial {
            double_sided: false,
            ..white()
        },
        |_, _| Want::uncovered(),
    );
    single.back = true;
    let mut double = Case::new(
        "back, double-sided",
        PbrMaterial {
            base_color: [0.2, 0.4, 0.6, 1.0],
            ..white()
        },
        |_, _| Want::base([0.2, 0.4, 0.6]),
    );
    double.back = true;
    out.extend([single, double]);
    // front-facing and single-sided, drawn
    out.push(Case::new(
        "front, single-sided",
        PbrMaterial {
            base_color: [0.6, 0.4, 0.2, 1.0],
            double_sided: false,
            ..white()
        },
        |_, _| Want::base([0.6, 0.4, 0.2]),
    ));
    out.extend(blends(w, tex, data));
    out
}

/// A pixel's coordinate along one axis of its quad: `(k + 0.5) / 8`.
fn at(k: usize) -> f32 {
    (k as f32 + 0.5) / PX as f32
}

/// A stream of four channels per corner (top-left, top-right, bottom-right, bottom-left) from the corner's `(u, v)`.
fn corners(f: impl Fn(f32, f32) -> [f32; 4]) -> Stream {
    Stream::F32x4(
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .map(|(u, v)| f(u, v))
            .to_vec(),
    )
}

/// `w` times `c`, summed.
fn weighted(ws: &[f32], cs: &[[f32; 3]]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (w, c) in ws.iter().zip(cs) {
        for k in 0..3 {
            out[k] += w * c[k];
        }
    }
    out
}

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];

/// Blends (`core.layered`) of `pbr.material` layers on a host whose own colour, (0.1, 0.2, 0.3), shows only where a
/// blend is not drawn. Each case's weights and colours are worked from core's equations ([`over_as_sum`],
/// [`height_blend`], [`WeightMapping::evaluate`]).
fn blends(w: &mut World, tex: Entity, data: Entity) -> Vec<Case> {
    let mut flat = |c: [f32; 3]| {
        let e = w.spawn();
        w.insert(
            e,
            PbrMaterial {
                base_color: [c[0], c[1], c[2], 1.0],
                ..white()
            },
        );
        e
    };
    let (red, green, blue, white_layer) = (flat(RED), flat(GREEN), flat([0.0, 0.0, 1.0]), flat([1.0; 3]));
    let red_taking = flat(RED);
    let green_ignoring = flat(GREEN);
    let black = |w: &mut World, emissive: [f32; 3], strength: f32| {
        let e = w.spawn();
        w.insert(
            e,
            PbrMaterial {
                base_color: [0.0, 0.0, 0.0, 1.0],
                emissive,
                emissive_strength: strength,
                ..white()
            },
        );
        e
    };
    let glowing_red = black(w, RED, 2.0);
    let glowing_green = black(w, GREEN, 1.0);
    w.insert(green_ignoring, VertexColor::Ignored);
    let textured_layer = w.spawn();
    w.insert(
        textured_layer,
        PbrMaterial {
            base_color_texture: slot(tex, |_| {}),
            ..white()
        },
    );
    let layer = |material| {
        Some(Layer {
            material,
            texcoord: None,
            transform: None,
        })
    };
    let host = || PbrMaterial {
        base_color: [0.1, 0.2, 0.3, 1.0],
        ..white()
    };
    let vertex = |semantic, channel| Source::Vertex { semantic, channel };
    let c1 = |channel| vertex(Semantic::Color(1), channel);
    let mask = |channels| Source::Mask {
        texture: slot(data, |_| {}).unwrap(),
        channels,
        bias: 0.0,
    };
    let case =
        |name, layered: LayeredMaterial, colour1: Stream, expect: Box<dyn Fn(usize, usize) -> Want>| Case {
            streams: vec![(Semantic::Color(1), colour1)],
            layered: Some(layered),
            expect,
            ..Case::new(name, host(), |_, _| Want::uncovered())
        };
    let mut out = vec![
        // COLOR_1's channels weight the layers directly, summed: red at 0.25, green at u
        case(
            "blend sum",
            LayeredMaterial {
                layers: vec![layer(red), layer(green)],
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
            corners(|u, _| [0.25, u, 0.0, 0.0]),
            Box::new(|i, _| Want {
                slope: 1.0 / PX as f32,
                ..Want::base([0.25, at(i), 0.0])
            }),
        ),
        // rgb_product_4 of (0.2, 0.5, 0.25): weights (0.3, 0.1, 0.3, 0.1) of red, green, blue and white
        case(
            "blend rgb_product_4",
            LayeredMaterial {
                layers: vec![layer(red), layer(green), layer(blue), layer(white_layer)],
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
            },
            corners(|_, _| [0.2, 0.5, 0.25, 1.0]),
            Box::new(|_, _| {
                let ws = WeightMapping::RgbProduct4.evaluate([0.2, 0.5, 0.25, 1.0]);
                Want::base(weighted(&ws, &[RED, GREEN, BLUE, [1.0; 3]]))
            }),
        ),
        // over: the texture's layer over red by saturate(0.5 + 0.5 m), m the data texture's red per quadrant (1 0 /
        // 0 1): A and D show the texture, B and C half each
        case(
            "blend threshold over",
            LayeredMaterial {
                layers: vec![layer(red), layer(textured_layer)],
                weights: LayerWeights {
                    channels: Some(vec![
                        Channel::Source(c1(0)),
                        Channel::Threshold {
                            v: c1(1),
                            m: mask([1.0, 0.0, 0.0, 0.0]),
                            k: [0.0, 1.0, 0.5, 0.0],
                        },
                    ]),
                    composition: Composition::Over,
                    ..LayerWeights::new(Semantic::Color(1), WeightMapping::Identity)
                },
            },
            corners(|_, _| [0.3, 0.5, 0.0, 0.0]),
            Box::new(|i, j| {
                let t = quadrant(i, j);
                let w1 = (0.5 + 0.5 * t[0]).clamp(0.0, 1.0);
                Want::base(weighted(&over_as_sum([0.3, w1, 0.0, 0.0], 2), &[RED, rgb(t)]))
            }),
        ),
        // height: weights 0.5 each, heights 0.8 (red) and u (green), contrast 0.75. Green's weight is u1 / (0.25 + u1)
        // with u1 = max(0.5 u - 0.15, 0) to u = 0.8, 0.25 / (0.9 - 0.5 u) beyond: its slope is at most 2 a quad
        case(
            "blend height",
            LayeredMaterial {
                layers: vec![layer(red), layer(green)],
                weights: LayerWeights {
                    composition: Composition::Height { contrast: 0.75 },
                    heights: vec![c1(2), c1(3)],
                    ..LayerWeights::new(Semantic::Color(1), WeightMapping::Identity)
                },
            },
            corners(|u, _| [0.5, 0.5, 0.8, u]),
            Box::new(|i, _| {
                let ws = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, at(i), 0.0, 0.0], 2, 0.75);
                Want {
                    slope: 2.0 / PX as f32,
                    ..Want::base(weighted(&ws, &[RED, GREEN]))
                }
            }),
        ),
        // channels: lerp(0.2, 0.6, v) (v down the quad, read from UV set 0), 0.5^3, and 0.2 * 0.6 * the data texture's
        // alpha (128/255 in D), weighting red, green and blue
        case(
            "blend lerp power product",
            LayeredMaterial {
                layers: vec![layer(red), layer(green), layer(blue)],
                weights: LayerWeights {
                    channels: Some(vec![
                        Channel::Lerp {
                            a: c1(0),
                            b: c1(1),
                            t: vertex(Semantic::TexCoord(0), 1),
                        },
                        Channel::Power {
                            v: c1(2),
                            exponent: 3,
                        },
                        Channel::Product {
                            sources: vec![c1(0), c1(1), mask([0.0, 0.0, 0.0, 1.0])],
                        },
                    ]),
                    ..LayerWeights::new(Semantic::Color(1), WeightMapping::Identity)
                },
            },
            corners(|_, _| [0.2, 0.6, 0.5, 1.0]),
            Box::new(|i, j| Want {
                slope: 0.4 / PX as f32,
                ..Want::base([0.2 + (0.6 - 0.2) * at(j), 0.125, 0.2 * 0.6 * quadrant(i, j)[3]])
            }),
        ),
        // null slot 0 contributes black: red at 0.4, and alpha 0.4, which the host's mask at 0.3 keeps
        case(
            "blend null slot, host mask",
            LayeredMaterial {
                layers: vec![None, layer(red)],
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
            corners(|_, _| [0.7, 0.4, 0.0, 0.0]),
            Box::new(|_, _| Want::base([0.4, 0.0, 0.0])),
        ),
        // the same at a mask of 0.5: alpha 0.4 is below it
        case(
            "blend null slot, host mask above",
            LayeredMaterial {
                layers: vec![None, layer(red)],
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
            corners(|_, _| [0.7, 0.4, 0.0, 0.0]),
            Box::new(|_, _| Want::uncovered()),
        ),
        // emission blends with the rest: (1, 0, 0) at 2 and (0, 1, 0) at 1, weighted 0.5 and 0.25
        case(
            "blend emission",
            LayeredMaterial {
                layers: vec![layer(glowing_red), layer(glowing_green)],
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
            corners(|_, _| [0.5, 0.25, 0.0, 0.0]),
            Box::new(|_, _| Want {
                emission: Some([1.0, 0.25, 0.0]),
                ..Want::base([0.0; 3])
            }),
        ),
        // weights COLOR_3, which the quad does not have: the host is drawn
        case(
            "blend not drawn",
            LayeredMaterial {
                layers: vec![layer(red)],
                weights: LayerWeights::new(Semantic::Color(3), WeightMapping::Identity),
            },
            corners(|_, _| [1.0; 4]),
            Box::new(|_, _| Want::base([0.1, 0.2, 0.3])),
        ),
    ];
    out[5].material.alpha_mode = AlphaMode::Mask { cutoff: 0.3 };
    out[6].material.alpha_mode = AlphaMode::Mask { cutoff: 0.5 };
    // the layer's UV set 1 (the quad mirrored) scaled by 2 in its transform, before the texture's own (none)
    let mut transformed = case(
        "blend layer set and transform",
        LayeredMaterial {
            layers: vec![Some(Layer {
                material: textured_layer,
                texcoord: Some(1),
                transform: Some(UvTransform {
                    scale: [2.0, 2.0],
                    ..UvTransform::IDENTITY
                }),
            })],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
        corners(|_, _| [1.0, 0.0, 0.0, 0.0]),
        Box::new(|i, j| {
            let c = usize::from((2.0 * (1.0 - at(i))).fract() >= 0.5);
            let r = usize::from((2.0 * at(j)).fract() >= 0.5);
            Want::base(rgb(texel(c, r)))
        }),
    );
    transformed.streams.push((Semantic::TexCoord(1), mirrored()));
    // weights read from COLOR_0: the layers do not take it as a colour
    let mut weights0 = case(
        "blend weights from COLOR_0",
        LayeredMaterial {
            layers: vec![layer(red), layer(green)],
            weights: LayerWeights::new(Semantic::Color(0), WeightMapping::Identity),
        },
        corners(|_, _| [0.0; 4]),
        Box::new(|_, _| Want::base([0.25, 0.75, 0.0])),
    );
    weights0.streams = vec![(Semantic::Color(0), corners(|_, _| [0.25, 0.75, 0.0, 1.0]))];
    // COLOR_0 (0.5 grey) taken by the red layer, not by the green one, which ignores it: half of each
    let mut taking = case(
        "blend layer takes COLOR_0",
        LayeredMaterial {
            layers: vec![layer(red_taking), layer(green_ignoring)],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
        corners(|_, _| [0.5, 0.5, 0.0, 0.0]),
        Box::new(|_, _| Want::base([0.25, 0.5, 0.0])),
    );
    taking
        .streams
        .push((Semantic::Color(0), corners(|_, _| [0.5, 0.5, 0.5, 1.0])));
    out.extend([transformed, weights0, taking]);
    out
}

/// UV set 0 mirrored left to right, per corner.
fn mirrored() -> Stream {
    Stream::F32x2(vec![[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]])
}

/// The quad of case `k`: one metre, its top-left corner at its cell's, facing the camera (+Z) unless `back`.
fn quad(k: usize, case: &Case, material: Entity) -> Primitive {
    let (col, row) = ((k % COLS) as f32, (k / COLS) as f32);
    let cell = PITCH as f32 / PX as f32;
    let (x0, top) = (col * cell + 0.125, -(row * cell + 0.125));
    let (x1, bottom) = (x0 + 1.0, top - 1.0);
    let n = if case.back { -1.0 } else { 1.0 };
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(vec![
                [x0, top, 0.0],
                [x1, top, 0.0],
                [x1, bottom, 0.0],
                [x0, bottom, 0.0],
            ]),
        ),
        (Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, n]; 4])),
        (
            Semantic::TexCoord(0),
            Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        ),
    ]);
    streams.extend(case.streams.iter().cloned());
    // counter-clockwise seen from +Z: top-left, bottom-left, bottom-right, then top-left, bottom-right, top-right
    let mut indices = vec![0, 3, 2, 0, 2, 1];
    if case.back {
        indices.reverse();
    }
    Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(indices),
        material: Some(material),
    }
}

/// Write, save and render the world of `cases`, in a folder of `scratch`.
fn rendered(
    blender: &Blender,
    scratch: &std::path::Path,
    cases: &[Case],
    w: &mut World,
) -> (Rendered, BlenderReport) {
    for (k, case) in cases.iter().enumerate() {
        let m = w.spawn();
        w.insert(m, Name(case.name.into()));
        w.insert(m, case.material.clone());
        if case.ignores_color {
            w.insert(m, VertexColor::Ignored);
        }
        if let Some(l) = &case.layered {
            w.insert(m, l.clone());
        }
        let q = w.spawn();
        w.insert(q, Name(format!("quad {k}")));
        w.insert(q, Mesh::new(vec![quad(k, case, m)]));
        w.insert(q, Transform::IDENTITY);
    }
    let folder = scratch.join("scene");
    let report = write_file(w, &WriteOptions::default(), &folder).unwrap();
    println!("{report}");
    let blend = scratch.join("scene.blend");
    let saved = save(blender, &folder, &blend).unwrap();
    println!("saved in {:.1} s: {}", saved.seconds, saved.built);
    let rows = cases.len().div_ceil(COLS);
    let (width, height) = (
        COLS as f32 * PITCH as f32 / PX as f32,
        rows as f32 * PITCH as f32 / PX as f32,
    );
    let options = RenderOptions {
        camera: Camera {
            eye: [width / 2.0, -height / 2.0, 10.0],
            target: [width / 2.0, -height / 2.0, 0.0],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Orthographic { height },
            near: 0.1,
            far: 100.0,
        },
        size: [(COLS * PITCH) as u32, (rows * PITCH) as u32],
        samples: SAMPLES,
        seed: 0,
        filter_width: 0.01,
        device: Device::Cpu,
    };
    let r = render(blender, &blend, &options, &scratch.join("render")).unwrap();
    println!(
        "rendered in {:.1} s: {:?}",
        r.seconds,
        r.passes.keys().collect::<Vec<_>>()
    );
    (r, report)
}

#[test]
fn each_construct_renders_as_worked_by_hand() {
    let Some(executable) = inputs::tool(inputs::Tool::Blender) else {
        return;
    };
    let blender = Blender { executable };
    let scratch = inputs::own_scratch("blender-constructs");
    let mut w = World::new();
    let cases = cases(&mut w);
    let (r, report) = rendered(&blender, &scratch, &cases, &mut w);
    // the one filtered colour slot (the bilinear case's) is counted as Cycles filters it, before decoding
    assert_eq!(report.counts.srgb_filtered_encoded, 1);
    // every blend drawn but the one that reads a stream the quad lacks, which is counted
    assert_eq!(report.blends, 11);
    assert_eq!(
        report.blends_drawn_from_host,
        BTreeMap::from([("reads COLOR_3, which scene.glb does not carry".to_owned(), 1)])
    );
    assert_eq!(report.blend_layers_without_color0, 0);
    let width = r.size[0] as usize;
    let base = r.pass(aov::BASE_COLOR).unwrap();
    let emission = r.pass(aov::EMISSION).unwrap();
    let coverage = r.pass(aov::COVERAGE).unwrap();
    let combined = r.pass("Combined").unwrap();
    let mut failures = Vec::new();
    for (k, case) in cases.iter().enumerate() {
        let (col, row) = (k % COLS, k / COLS);
        for j in 0..PX {
            for i in 0..PX {
                let want = (case.expect)(i, j);
                let tolerance = want.tolerance + want.slope * OFF_CENTRE;
                let p = (row * PITCH + 1 + j) * width + col * PITCH + 1 + i;
                let got = |pass: &[f32], channels: usize| -> Vec<f32> {
                    pass[p * channels..p * channels + channels.min(3)].to_vec()
                };
                let mut check = |what: &str, got: Vec<f32>, want: &[f32]| {
                    if got.iter().zip(want).any(|(g, w)| (g - w).abs() > tolerance) {
                        failures.push(format!("{} ({i}, {j}) {what}: {got:?}, want {want:?}", case.name));
                    }
                };
                check("base colour", got(&base, 4), &want.base);
                check("coverage", got(&coverage, 1), &[want.coverage]);
                if let Some(e) = want.emission {
                    check("emission", got(&emission, 4), &e);
                }
                if let Some(c) = want.combined {
                    check("image", got(&combined, 4), &c);
                }
            }
        }
    }
    // each case's count and its first difference
    let mut by_case: BTreeMap<&str, (usize, &String)> = BTreeMap::new();
    for f in &failures {
        by_case.entry(f.split(" (").next().unwrap()).or_insert((0, f)).0 += 1;
    }
    let shown: Vec<String> = by_case
        .values()
        .map(|(n, f)| format!("{n} values, first {f}"))
        .collect();
    assert!(
        failures.is_empty(),
        "{} pixel values differ in {} cases:\n{}",
        failures.len(),
        by_case.len(),
        shown.join("\n")
    );
}
