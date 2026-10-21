#![allow(clippy::unwrap_used)]
//! `evaluate` on one HLSL fixture compiled to both bytecodes (tests/fixtures/evaluate.hlsl), with outputs worked out
//! from the HLSL, and constant packing worked out from D3D's packing rules.
use primase_shader::{
    Buffer, Constants, Execution, Field, Grid, Image, Input, Lane, Layout, Member, PixelShader, Reflection,
    Resource, Sampler, Texture, Type, Variable, evaluate,
};
use std::collections::BTreeMap;
use std::sync::Arc;

fn shaders() -> Vec<(&'static str, Box<dyn PixelShader>)> {
    vec![
        (
            "DXBC",
            Box::new(primase_dxbc::Shader::parse(include_bytes!("fixtures/evaluate.dxbc")).unwrap()),
        ),
        (
            "DXIL",
            Box::new(primase_dxil::Shader::parse(include_bytes!("fixtures/evaluate.dxil")).unwrap()),
        ),
    ]
}

/// Returns the coordinates it is sampled at, so the program's UV arithmetic shows in its output.
#[derive(Debug)]
struct Coordinates;
impl Image for Coordinates {
    fn sample(&self, uv: [f32; 2], _lod: primase_shader::Lod) -> Result<[f32; 4], String> {
        Ok([uv[0], uv[1], 0.5, 1.])
    }
}

fn inputs() -> BTreeMap<(String, u32), Input> {
    BTreeMap::from([
        (("TEXCOORD".into(), 0), Input::GridUv),
        (
            ("texcoord".into(), 1),
            Input::Lanes([
                Lane::Bits(1.5f32.to_bits()),
                Lane::V,
                Lane::Bits(0),
                Lane::Bits(0),
            ]),
        ),
        (("TEXCOORD".into(), 2), Input::Constant([0.75, 0., 0., 0.])),
    ])
}

fn constants(layout: &Layout) -> Constants {
    let mut c = Constants::new(layout);
    assert!(c.set("tint", &[2., 4., 0.5, 1.]).unwrap());
    assert!(c.set("tiling", &[2., 4.]).unwrap());
    assert!(c.set("strength", &[2.]).unwrap());
    c
}

#[test]
fn both_bytecodes_evaluate_the_fixture_as_worked_out() {
    let textures = BTreeMap::from([(3, Texture::Image(Arc::new(Coordinates)))]);
    for (name, shader) in shaders() {
        let layout = Layout::of(shader.as_ref()).unwrap();
        // cbuffer Material: tint at byte 0 (4 words), tiling at 16 (2), strength packed after it at 24 (1); the
        // program reads two rows.
        assert_eq!(layout.buffers, [(1, 8)], "{name}");
        for (field, offset, words) in [("tint", 0, 4), ("tiling", 4, 2), ("strength", 6, 1)] {
            assert_eq!(
                layout.fields[field],
                Field {
                    buffer: 1,
                    offset,
                    words
                },
                "{name} {field}"
            );
        }
        let c = constants(&layout);
        // Word 7, the padding after strength, is left unset.
        assert_eq!(c.unset_words(), 1, "{name}");
        let grid = Grid {
            size: 2,
            period: [1., 1.],
        };
        let e = evaluate(
            shader.as_ref(),
            &c,
            &textures,
            &inputs(),
            grid,
            &[(0, 15), (1, 15), (2, 15)],
        )
        .unwrap();
        assert_eq!((e.samples, e.targets.len()), (4, 3), "{name}");
        // colour = sample(uv * tiling) * tint = (2U * 2, 4V * 4, 0.5 * 0.5, 1); data = (1.5 * 2, V * 2, 0.75, 1),
        // TEXCOORD1 arriving in the .zw of the UV's register. Pixel (x, y) has U = (x + 0.5) / 2, V likewise.
        let mut colour = Vec::new();
        let mut data = Vec::new();
        for v in [0.25f32, 0.75] {
            for u in [0.25f32, 0.75] {
                colour.push([4. * u, 16. * v, 0.25, 1.]);
                data.push([3., 2. * v, 0.75, 1.]);
            }
        }
        assert_eq!(e.targets[0].as_deref(), Some(&colour[..]), "{name}");
        assert!(e.targets[1].is_none(), "{name}: o1 is not written");
        assert_eq!(e.targets[2].as_deref(), Some(&data[..]), "{name}");
    }
}

#[test]
fn what_the_program_needs_and_is_not_given_is_refused() {
    let textures = BTreeMap::from([(3, Texture::Constant([1.; 4]))]);
    let grid = Grid {
        size: 2,
        period: [1., 1.],
    };
    for (name, shader) in shaders() {
        let shader = shader.as_ref();
        let layout = Layout::of(shader).unwrap();
        let c = constants(&layout);
        let message = |r: Result<_, primase_shader::Error>| {
            r.map(|_: primase_shader::Evaluation| ()).unwrap_err().detail
        };
        let mut missing = inputs();
        missing.remove(&("texcoord".into(), 1));
        assert!(
            message(evaluate(shader, &c, &textures, &missing, grid, &[(0, 15)])).contains("TEXCOORD1"),
            "{name}"
        );
        assert!(
            message(evaluate(
                shader,
                &c,
                &BTreeMap::new(),
                &inputs(),
                grid,
                &[(0, 15)]
            ))
            .contains("t3"),
            "{name}"
        );
        for bad in [
            Grid {
                size: 0,
                period: [1., 1.],
            },
            Grid {
                size: 2,
                period: [f32::NAN, 1.],
            },
        ] {
            assert!(
                evaluate(shader, &c, &textures, &inputs(), bad, &[(0, 15)]).is_err(),
                "{name}"
            );
        }
        assert!(
            evaluate(shader, &c, &textures, &inputs(), grid, &[(0, 0)]).is_err(),
            "{name}"
        );
    }
}

#[test]
fn constants_are_bounded_by_field_and_buffer() {
    let (_, shader) = shaders().remove(0);
    let layout = Layout::of(shader.as_ref()).unwrap();
    let mut c = Constants::new(&layout);
    assert!(!c.set("absent", &[1.]).unwrap());
    assert!(c.set("tiling", &[1., 2., 3.]).is_err());
    c.set_words(1, 7, &[0x3f80_0000]).unwrap();
    assert_eq!(c.unset_words(), 7);
    assert!(c.set_words(1, 7, &[0, 0]).is_err());
    assert!(c.set_words(0, 0, &[0]).is_err());
}

/// A reflection-only shader for layouts the fixture does not have.
#[derive(Debug)]
struct Reflected(Reflection, Vec<(u32, u32)>);
impl PixelShader for Reflected {
    fn reflection(&self) -> &Reflection {
        &self.0
    }
    fn input_count(&self) -> usize {
        0
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &self.1
    }
    fn textures(&self) -> &[u32] {
        &[]
    }
    fn samplers(&self) -> &[u32] {
        &[]
    }
    fn execute(
        &self,
        _: &[[u32; 4]],
        _: &[(u32, &[[u32; 4]])],
        _: &mut dyn Sampler,
    ) -> Result<Execution, primase_shader::Error> {
        unreachable!("layout only")
    }
}
fn ty(class: u16, rows: u16, columns: u16, elements: u16) -> Type {
    Type {
        class,
        scalar: 3,
        rows,
        columns,
        elements,
        members: Vec::new(),
    }
}
fn member(name: &str, offset: u32, ty: Type) -> Member {
    Member {
        name: name.into(),
        offset,
        ty,
    }
}
fn reflected(members: Vec<Member>) -> Reflected {
    let structure = Type {
        class: 5,
        scalar: 0,
        rows: 1,
        columns: 1,
        elements: 0,
        members,
    };
    Reflected(
        Reflection {
            buffers: vec![Buffer {
                name: "Frame".into(),
                size: 256,
                variables: vec![Variable {
                    name: "frame".into(),
                    offset: 32,
                    size: 224,
                    ty: structure,
                    default: None,
                }],
            }],
            resources: vec![Resource {
                name: "Frame".into(),
                kind: 0,
                return_type: 0,
                dimension: 0,
                slot: 2,
                count: 1,
            }],
            inputs: Vec::new(),
            outputs: Vec::new(),
        },
        vec![(2, 16)],
    )
}

#[test]
fn struct_members_are_flattened_with_d3d_packing() {
    // HLSL packing: each matrix row (row-major) or column (column-major) and each array element starts a 16-byte
    // register, and the last one takes only its own components.
    let shader = reflected(vec![
        member("normal", 0, ty(1, 1, 3, 0)),
        member("world", 16, ty(2, 4, 3, 0)),
        member("view", 80, ty(3, 3, 4, 0)),
        member("offsets", 144, ty(1, 1, 2, 3)),
        member("count", 184, ty(0, 1, 1, 0)),
    ]);
    let layout = Layout::of(&shader).unwrap();
    assert_eq!(layout.buffers, [(2, 64)]);
    // The struct variable starts at byte 32, word 8.
    for (name, offset, words) in [
        ("normal", 8, 3),
        ("world", 12, 15),
        ("view", 28, 15),
        ("offsets", 44, 10),
        ("count", 54, 1),
    ] {
        assert_eq!(
            layout.fields[name],
            Field {
                buffer: 2,
                offset,
                words
            },
            "{name}"
        );
    }
    let twice = reflected(vec![
        member("a", 0, ty(0, 1, 1, 0)),
        member("a", 4, ty(0, 1, 1, 0)),
    ]);
    assert!(Layout::of(&twice).is_err());
    let unaligned = reflected(vec![member("a", 2, ty(0, 1, 1, 0))]);
    assert!(Layout::of(&unaligned).is_err());
}
