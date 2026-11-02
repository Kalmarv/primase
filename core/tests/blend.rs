#![allow(clippy::unwrap_used)]
//! `core::shader::blend` on `fixtures/blend.hlsl` as fxc compiles it: the albedo is read as two layers laid over one
//! another, as the program states them (the bottom layer's colour moved part of the way to a constant, the second
//! layer's scaled past one at a scaled and offset UV, the weight made of a vertex lane and a mask on the second UV set,
//! the blend saturated), and what core.layered computes from that reading is what iridia-exec running the program
//! gives, bit for bit, across colours past one, weights past 0 and 1 and NaN weights. A palette row that is not white
//! where the freeze holds it is refused, not dropped.
//!
//! The values stay within what both sides compute alike: no subnormal value (Direct3D flushes them; core's rules do
//! not), and a mask texture's lanes finite (a mask is a dot product of the sample's four lanes, which a lane the
//! program does not read reaches only as `0 * lane`).
use std::collections::BTreeMap;

use iridia::Program;
use iridia_exec::{
    Input, Invocation, Layout, PixelShader, ProgramShader, View, Views, evaluate::Constants, evaluate::Grid,
    evaluate_raw,
};
use iridia_interface::{AddressMode, D3dClass, Label, ResourceId, SamplerState, Stage};
use primase_core::layered::{Channel, ColorLane, Expression, Source, lerp, mask_value, saturate};
use primase_core::pbr::UvTransform;
use primase_core::shader::{
    ConstantBinding, ConstantRole, Freeze, InputLink, LayeredAlbedo, ObjectParameter, OutputBinding,
    OutputRole, StageBinding, TextureBinding, TextureSource, read_layered_albedo,
};
use primase_core::{Entity, Semantic, World};

fn program() -> Program {
    iridia_dxbc::lift(include_bytes!("fixtures/blend.dxbc")).unwrap()
}

/// `fixtures/single.hlsl`: one texture, laid over nothing.
fn single() -> Program {
    iridia_dxbc::lift(include_bytes!("fixtures/single.dxbc")).unwrap()
}

/// The resource of `class` register `register`, where the program declares it.
fn declared(p: &Program, class: D3dClass, register: u32) -> Option<ResourceId> {
    p.interface
        .resources
        .iter()
        .find(|r| {
            r.provenance
                .d3d
                .is_some_and(|d| d.class == class && d.register == register)
        })
        .map(|r| r.id)
}

/// The resource of `class` register `register`.
fn resource(p: &Program, class: D3dClass, register: u32) -> ResourceId {
    declared(p, class, register).unwrap()
}

/// The material's constants: `tile`, `adjust`, `tint`, `gain` (the weight's gain, then the mask coordinate's
/// scale and offset), `slot`.
#[derive(Clone, Copy)]
struct Material {
    tile: [f32; 4],
    adjust: [f32; 4],
    tint: [f32; 4],
    gain: [f32; 4],
    slot: f32,
}

impl Material {
    fn words(&self) -> Vec<u32> {
        [
            self.tile,
            self.adjust,
            self.tint,
            self.gain,
            [0., 0., 0., self.slot],
        ]
        .into_iter()
        .flatten()
        .map(f32::to_bits)
        .collect()
    }
}

/// The three textures, one entity each.
struct Textures {
    shader: Entity,
    base: Entity,
    layer: Entity,
    mask: Entity,
}

fn binding(p: &Program, m: &Material, t: &Textures) -> StageBinding {
    let mut stage = StageBinding::new(Stage::Pixel, t.shader);
    let state = SamplerState {
        wrap: [Some(AddressMode::Repeat); 3],
        ..SamplerState::default()
    };
    // the textures the program declares
    stage.textures = [(0, t.base), (1, t.layer), (2, t.mask)]
        .into_iter()
        .filter_map(|(register, texture)| {
            Some(TextureBinding {
                resource: declared(p, D3dClass::ShaderResource, register)?,
                sampler: None,
                source: TextureSource::Texture {
                    texture,
                    sampler: state,
                },
            })
        })
        .collect();
    stage.constants = vec![
        ConstantBinding {
            resource: resource(p, D3dClass::ConstantBuffer, 1),
            offset: 0,
            size: 80,
            name: None,
            role: ConstantRole::Material(m.words()),
            lines: None,
        },
        ConstantBinding {
            resource: resource(p, D3dClass::ConstantBuffer, 4),
            offset: 0,
            size: 416,
            name: None,
            role: ConstantRole::Object(ObjectParameter::InstanceColour),
            lines: None,
        },
    ];
    stage.outputs = vec![OutputBinding {
        label: Label {
            name: "SV_Target".into(),
            index: 1,
        },
        lanes: 0b0111,
        role: OutputRole::Albedo,
    }];
    stage
}

/// The freeze: the palette's rows, the UV sets, the weight lanes and the record number.
fn freeze(p: &Program, palette: &[[f32; 4]; 26], record: f32) -> Freeze {
    let b4 = resource(p, D3dClass::ConstantBuffer, 4);
    let mut words = BTreeMap::new();
    for (row, lanes) in palette.iter().enumerate() {
        for (lane, x) in lanes.iter().enumerate() {
            words.insert((b4, (16 * row + 4 * lane) as u32), x.to_bits());
        }
    }
    let stream = |semantic, channel| Some(InputLink::Stream { semantic, channel });
    Freeze {
        words,
        inputs: BTreeMap::from([
            (
                ("TEXCOORD".into(), 0),
                [
                    stream(Semantic::TexCoord(0), 0),
                    stream(Semantic::TexCoord(0), 1),
                    stream(Semantic::TexCoord(1), 0),
                    stream(Semantic::TexCoord(1), 1),
                ],
            ),
            (
                ("TEXCOORD".into(), 1),
                [0, 1, 2, 3].map(|c| stream(Semantic::Color(1), c)),
            ),
            (
                ("NORMAL".into(), 0),
                [None, None, None, Some(InputLink::Word(record.to_bits()))],
            ),
        ]),
    }
}

/// One run of the program on one pixel: the albedo's three lanes.
fn run(
    shader: &ProgramShader,
    m: &Material,
    palette: &[[f32; 4]; 26],
    record: f32,
    texels: [[f32; 4]; 3],
    weights: [f32; 4],
) -> [u32; 3] {
    let layout = Layout::of(shader).unwrap();
    let mut constants = Constants::new(&layout);
    constants.set_words(1, 0, &m.words()).unwrap();
    let rows: Vec<u32> = palette.iter().flatten().map(|x| x.to_bits()).collect();
    constants.set_words(4, 0, &rows).unwrap();
    let textures = Views::refusing(BTreeMap::from([
        (0, View::Constant(texels[0])),
        (1, View::Constant(texels[1])),
        (2, View::Constant(texels[2])),
    ]));
    let inputs = BTreeMap::from([
        (("NORMAL".into(), 0), Input::Constant([0.1, 0.2, 0.3, record])),
        (("TEXCOORD".into(), 0), Input::Constant([0.3, 0.6, 0.2, 0.9])),
        (("TEXCOORD".into(), 1), Input::Constant(weights)),
    ]);
    let target = shader
        .interface()
        .outputs
        .iter()
        .find(|o| o.label.name == "SV_Target" && o.label.index == 1)
        .and_then(|o| o.provenance.d3d)
        .unwrap()
        .register;
    let raw = evaluate_raw(
        shader,
        &constants,
        &textures,
        &inputs,
        Grid {
            size: 1,
            period: [1., 1.],
        },
        &[(target, 15)],
        Invocation::Scalar,
    )
    .unwrap();
    let out = raw.pixels[0][0].0;
    [out[0], out[1], out[2]]
}

/// What core.layered computes from `read`, given each texture's texel and the vertex weight lanes.
fn blended(read: &LayeredAlbedo, texel: impl Fn(Entity) -> [f32; 4], weights: [f32; 4]) -> [u32; 3] {
    let colour = |i: usize| -> [f32; 3] {
        let layer = &read.layers[i];
        let t = texel(layer.texture.texture);
        // the colour its material gives: the texture times the factor
        let given = [
            t[0] * layer.factor[0],
            t[1] * layer.factor[1],
            t[2] * layer.factor[2],
            t[3],
        ];
        match &layer.color {
            None => [given[0], given[1], given[2]],
            Some(lanes) => lanes.clone().map(|e: Expression<ColorLane>| {
                let values: Vec<f32> = e
                    .sources
                    .iter()
                    .map(|ColorLane(l)| given[usize::from(*l)])
                    .collect();
                e.evaluate(&values)
            }),
        }
    };
    let weight = |c: &Channel| -> f32 {
        let values: Vec<f32> = c
            .sources()
            .into_iter()
            .map(|s| match s {
                Source::Vertex { semantic, channel } => {
                    assert_eq!(*semantic, Semantic::Color(1));
                    weights[usize::from(*channel)]
                }
                Source::Mask {
                    texture,
                    channels,
                    bias,
                } => mask_value(*channels, *bias, texel(texture.texture)),
            })
            .collect();
        c.evaluate(&values)
    };
    let mut c = colour(0);
    for i in 1..read.layers.len() {
        let w = weight(&read.weights[i]);
        let l = colour(i);
        c = std::array::from_fn(|k| lerp(c[k], l[k], w));
    }
    if read.saturate {
        c = c.map(saturate);
    }
    c.map(f32::to_bits)
}

fn white() -> [[f32; 4]; 26] {
    [[1.; 4]; 26]
}

#[test]
fn the_albedo_reads_as_the_layers_the_program_lays_over_one_another() {
    let p = program();
    let mut world = World::default();
    let t = Textures {
        shader: world.spawn(),
        base: world.spawn(),
        layer: world.spawn(),
        mask: world.spawn(),
    };
    let m = Material {
        tile: [3., 2., -1., 0.5],
        adjust: [1., 0.9, 0.8, 0.3],
        tint: [1.47, 1., 0.5, 1.],
        gain: [5., 3., 0.75, 0.5],
        slot: 0.,
    };
    let read = read_layered_albedo(&p, &binding(&p, &m, &t), &freeze(&p, &white(), 0.)).unwrap();
    assert!(read.saturate);
    assert_eq!(read.unstated_wrap, 0);
    assert_eq!(read.layers.len(), 2);
    // the bottom layer: the base map on the first UV set, its colour moved 0.3 of the way to (1, 0.9, 0.8)
    let bottom = &read.layers[0];
    assert_eq!((bottom.texture.texture, bottom.texture.texcoord), (t.base, 0));
    assert_eq!(bottom.texture.transform, None);
    let lanes = bottom.color.as_ref().unwrap();
    for (k, to) in [1., 0.9, 0.8].into_iter().enumerate() {
        let x = 0.6f32;
        assert_eq!(
            lanes[k].evaluate(&[x]).to_bits(),
            lerp(x, to, 0.3).to_bits(),
            "lane {k}"
        );
        assert_eq!(lanes[k].sources, vec![ColorLane(k as u8)]);
    }
    // the second: the layer map at uv * (3, 2) + (-1, 0.5), scaled by the tint, past one in red
    let second = &read.layers[1];
    assert_eq!((second.texture.texture, second.texture.texcoord), (t.layer, 0));
    assert_eq!(
        second.texture.transform,
        Some(UvTransform {
            offset: [-1., 0.5],
            scale: [3., 2.],
            rotation: 0.
        })
    );
    assert_eq!((second.factor, second.color.is_none()), ([1.47, 1., 0.5], true));
    // its weight reads the first weight lane and the mask's red on the second UV set
    let sources = read.weights[1].sources();
    assert_eq!(sources.len(), 2, "{sources:?}");
    assert!(sources.iter().any(|s| matches!(
        s,
        Source::Vertex {
            semantic: Semantic::Color(1),
            channel: 0
        }
    )));
    assert!(sources.iter().any(|s| matches!(
        s,
        Source::Mask { texture, channels: [1., 0., 0., 0.], bias } if texture.texture == t.mask && texture.texcoord == 1 && *bias == 0.
    )));
    // the mask's coordinate, scaled by (3, 0.75) and then by 2, less 0.5, is one transform
    let mask = sources.iter().find_map(|s| match s {
        Source::Mask { texture, .. } => Some(texture),
        Source::Vertex { .. } => None,
    });
    assert_eq!(
        mask.unwrap().transform,
        Some(UvTransform {
            offset: [-0.5, -0.5],
            scale: [6., 1.5],
            rotation: 0.
        })
    );
    // the bottom layer's weight is the constant 1
    assert!(read.weights[0].sources().is_empty());
}

#[test]
fn what_core_computes_from_the_reading_is_the_programs_albedo_bit_for_bit() {
    let p = program();
    let mut world = World::default();
    let t = Textures {
        shader: world.spawn(),
        base: world.spawn(),
        layer: world.spawn(),
        mask: world.spawn(),
    };
    let materials = [
        Material {
            tile: [3., 2., -1., 0.5],
            adjust: [1., 0.9, 0.8, 0.3],
            tint: [1.47, 1., 0.5, 1.],
            gain: [5., 3., 0.75, 0.5],
            slot: 0.,
        },
        Material {
            tile: [1., 1., 0., 0.],
            adjust: [0.2, 0.4, 0.6, 1.],
            tint: [0.75, 0.5, 0.25, 1.],
            gain: [0.5, 1., 1., 0.],
            slot: 2.,
        },
    ];
    let texels = [
        [0.25, 0.5, 0.75, 1.],
        [1., 1., 1., 1.],
        [0., 0., 0., 0.],
        [0.9, 0.1, 0.4, 0.5],
        [1.5, 3., 0.2, 1.],
    ];
    let weights = [-1., 0., 0.25, 0.5, 0.75, 1., 2., f32::NAN, f32::INFINITY];
    let shader = ProgramShader::new(p.clone()).unwrap();
    let mut cases = 0;
    // the controls: the comparison sees a reading that leaves out the saturation, or whose tint is one ulp off in red
    let (mut unsaturated_differs, mut nudged_differs) = (0, 0);
    for m in &materials {
        let stage = binding(&p, m, &t);
        // the record number the vertex program hands on: 0 and 1 (rows 9 to 12 and 22 to 25, both white)
        for record in [0., 1.] {
            let read = read_layered_albedo(&p, &stage, &freeze(&p, &white(), record)).unwrap();
            let unsaturated = LayeredAlbedo {
                saturate: false,
                ..read.clone()
            };
            let mut nudged = read.clone();
            nudged.layers[1].factor[0] = f32::from_bits(nudged.layers[1].factor[0].to_bits() + 1);
            for base in texels {
                for layer in texels {
                    for mask in texels.iter().filter(|x| x.iter().all(|v: &f32| v.is_finite())) {
                        for w in weights {
                            let lanes = [w, 0.5, 0.25, 0.];
                            let ran = run(&shader, m, &white(), record, [base, layer, *mask], lanes);
                            let texel = |e: Entity| {
                                if e == t.base {
                                    base
                                } else if e == t.layer {
                                    layer
                                } else {
                                    *mask
                                }
                            };
                            assert_eq!(
                                blended(&read, texel, lanes),
                                ran,
                                "material {:?} record {record} base {base:?} layer {layer:?} mask {mask:?} weight {w}",
                                m.tint
                            );
                            unsaturated_differs += usize::from(blended(&unsaturated, texel, lanes) != ran);
                            nudged_differs += usize::from(blended(&nudged, texel, lanes) != ran);
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 2 * 2 * 5 * 5 * 5 * 9);
    assert!(
        unsaturated_differs > 0 && nudged_differs > 0,
        "{unsaturated_differs} {nudged_differs}"
    );
    eprintln!(
        "controls: {unsaturated_differs} of {cases} differ unsaturated, {nudged_differs} with the tint one ulp off"
    );
}

#[test]
fn a_palette_row_that_is_not_white_is_refused_as_a_factor() {
    let p = program();
    let mut world = World::default();
    let t = Textures {
        shader: world.spawn(),
        base: world.spawn(),
        layer: world.spawn(),
        mask: world.spawn(),
    };
    let m = Material {
        tile: [1., 1., 0., 0.],
        adjust: [1., 1., 1., 0.],
        tint: [1., 1., 1., 1.],
        gain: [1., 1., 1., 0.],
        slot: 1.,
    };
    let stage = binding(&p, &m, &t);
    // record 0, slot 1: row 10
    let mut palette = white();
    palette[10] = [0.5, 1., 1., 1.];
    let refused = read_layered_albedo(&p, &stage, &freeze(&p, &palette, 0.)).unwrap_err();
    assert_eq!(refused.kind, "factor", "{refused:?}");
    // the same row read by another record is not this draw's: read
    assert!(read_layered_albedo(&p, &stage, &freeze(&p, &palette, 1.)).is_ok());
    // a record the freeze does not state leaves the factor unknown: refused, not taken as 1
    let mut unknown = freeze(&p, &white(), 0.);
    unknown.inputs.remove(&("NORMAL".into(), 0));
    let refused = read_layered_albedo(&p, &stage, &unknown).unwrap_err();
    assert_eq!(refused.kind, "factor", "{refused:?}");
}

/// One texture with no interpolation over it reads as one layer: the texture at its scaled and offset UV, its colour
/// the sample times the tint (a factor, past one in red), the colour saturated; and what core.layered computes from that
/// reading is the program's albedo bit for bit, across texels and tints past one. The controls: a reading without the
/// saturation, or with the tint one ulp off in red, differs.
#[test]
fn a_single_texture_reads_as_one_layer_bit_for_bit() {
    let p = single();
    let mut world = World::default();
    let t = Textures {
        shader: world.spawn(),
        base: world.spawn(),
        layer: world.spawn(),
        mask: world.spawn(),
    };
    let materials = [
        Material {
            tile: [3., 2., -1., 0.5],
            adjust: [0.; 4],
            tint: [1.47, 1., 0.5, 1.],
            gain: [0.; 4],
            slot: 0.,
        },
        Material {
            tile: [1., 1., 0., 0.],
            adjust: [0.; 4],
            tint: [0.75, 0.5, 0.25, 1.],
            gain: [0.; 4],
            slot: 2.,
        },
    ];
    let texels = [
        [0.25, 0.5, 0.75, 1.],
        [1., 1., 1., 1.],
        [0., 0., 0., 0.],
        [0.9, 0.1, 0.4, 0.5],
        [1.5, 3., 0.2, 1.],
        [-0.5, 0.7, 2., 1.],
    ];
    let shader = ProgramShader::new(p.clone()).unwrap();
    let (mut cases, mut unsaturated_differs, mut nudged_differs) = (0, 0, 0);
    for m in &materials {
        let stage = binding(&p, m, &t);
        assert_eq!(stage.textures.len(), 1);
        for record in [0., 1.] {
            let read = read_layered_albedo(&p, &stage, &freeze(&p, &white(), record)).unwrap();
            assert_eq!(read.layers.len(), 1);
            assert_eq!(read.weights.len(), 1);
            assert!(read.weights[0].sources().is_empty());
            assert!(read.saturate);
            let layer = &read.layers[0];
            assert_eq!((layer.texture.texture, layer.texture.texcoord), (t.base, 0));
            let transform = (m.tile != [1., 1., 0., 0.]).then_some(UvTransform {
                offset: [m.tile[2], m.tile[3]],
                scale: [m.tile[0], m.tile[1]],
                rotation: 0.,
            });
            assert_eq!(layer.texture.transform, transform);
            assert_eq!(
                (layer.factor, layer.color.is_none()),
                ([m.tint[0], m.tint[1], m.tint[2]], true)
            );
            let unsaturated = LayeredAlbedo {
                saturate: false,
                ..read.clone()
            };
            let mut nudged = read.clone();
            nudged.layers[0].factor[0] = f32::from_bits(nudged.layers[0].factor[0].to_bits() + 1);
            for base in texels {
                let lanes = [0.; 4];
                let ran = run(&shader, m, &white(), record, [base, base, base], lanes);
                let texel = |_: Entity| base;
                assert_eq!(
                    blended(&read, texel, lanes),
                    ran,
                    "material {:?} record {record} base {base:?}",
                    m.tint
                );
                unsaturated_differs += usize::from(blended(&unsaturated, texel, lanes) != ran);
                nudged_differs += usize::from(blended(&nudged, texel, lanes) != ran);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 2 * 2 * 6);
    assert!(
        unsaturated_differs > 0 && nudged_differs > 0,
        "{unsaturated_differs} {nudged_differs}"
    );
}
