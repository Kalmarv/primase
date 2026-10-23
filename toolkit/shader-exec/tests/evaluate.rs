#![allow(clippy::unwrap_used)]
//! `evaluate` on one HLSL fixture compiled to both bytecodes (tests/fixtures/evaluate.hlsl), with outputs worked out
//! from the HLSL, and constant packing worked out from D3D's packing rules.
use primase_shader::{
    BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, Interface, Majorness, Member, Resource,
    ResourceId, ResourceKind, ResourceProvenance, ScalarType, Stage, d3d,
};
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{
    Execution, Field, Grid, Image, Input, Lane, Layout, PixelShader, Sampler, Texture, evaluate,
    evaluate::Constants,
};
use std::collections::BTreeMap;
use std::sync::Arc;

fn executor(program: primase_shader::Program) -> ProgramShader {
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    ProgramShader::new(program).unwrap()
}

fn shaders() -> Vec<(&'static str, Box<dyn PixelShader>)> {
    vec![
        (
            "DXBC",
            Box::new(executor(
                primase_dxbc::lift(include_bytes!("fixtures/evaluate.dxbc")).unwrap(),
            )),
        ),
        (
            "DXIL",
            Box::new(executor(
                primase_dxil::lift(include_bytes!("fixtures/evaluate.dxil")).unwrap(),
            )),
        ),
    ]
}

/// Returns the coordinates it is sampled at, so the program's UV arithmetic shows in its output.
#[derive(Debug)]
struct Coordinates;
impl Image for Coordinates {
    fn sample(&self, uv: [f32; 2], _lod: primase_shader_exec::Lod) -> Result<[f32; 4], String> {
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
        let message = |r: Result<_, primase_shader_exec::Error>| {
            r.map(|_: primase_shader_exec::Evaluation| ()).unwrap_err().detail
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
struct Reflected(Interface, Vec<(u32, u32)>);
impl PixelShader for Reflected {
    fn interface(&self) -> &Interface {
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
    ) -> Result<Execution, primase_shader_exec::Error> {
        unreachable!("layout only")
    }
}
/// A constant of D3D reflection's class (1 vector, 2 row-major matrix, 3 column-major matrix, 0 scalar), shape and
/// array length, packed by D3D's rules.
fn ty(class: u16, rows: u32, columns: u32, elements: u32) -> ConstantType {
    let f = ScalarType::Float;
    let one = match class {
        0 => ConstantType::Scalar(f),
        1 => d3d::vector(f, columns).unwrap(),
        2 => d3d::matrix(f, rows, columns, Majorness::Row).unwrap(),
        _ => d3d::matrix(f, rows, columns, Majorness::Column).unwrap(),
    };
    if elements == 0 {
        one
    } else {
        d3d::array(one, elements).unwrap()
    }
}
fn member(name: &str, offset: u32, ty: ConstantType) -> Member {
    Member {
        name: name.into(),
        offset,
        ty,
    }
}
fn reflected(members: Vec<Member>) -> Reflected {
    Reflected(
        Interface {
            stage: Stage::Pixel,
            inputs: Vec::new(),
            outputs: Vec::new(),
            resources: vec![Resource {
                id: ResourceId(0),
                kind: ResourceKind::ConstantBuffer {
                    layout: BufferLayout {
                        size: 256,
                        constants: vec![Constant {
                            name: "frame".into(),
                            offset: 32,
                            ty: ConstantType::Struct { members },
                            default: None,
                        }],
                    },
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: "Frame".into(),
                    d3d: Some(D3dBinding {
                        class: D3dClass::ConstantBuffer,
                        register: 2,
                        space: 0,
                    }),
                    counter_of: None,
                    layout: None,
                },
            }],
            facts: Default::default(),
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

/// The same fixture in 2 x 2 quads (DXBC: the bytecode with quad evaluation): the same outputs as one pixel at a
/// time, on a grid of 2 and on an odd one (3: its last row and column run with helper lanes past the edge, not kept);
/// each implicit sample's footprint the quad's gradients of `uv * tiling`, `(2, 4)` times the grid's step (0.5 on the
/// grid of 2): `(1, 0)` and `(0, 2)`.
#[test]
fn quads_give_the_scalar_outputs_and_their_footprints() {
    #[derive(Debug, Default)]
    struct Footprints(std::sync::Mutex<Vec<primase_shader_exec::Lod>>);
    impl Image for Footprints {
        fn sample(&self, uv: [f32; 2], lod: primase_shader_exec::Lod) -> Result<[f32; 4], String> {
            self.0.lock().unwrap().push(lod);
            Ok([uv[0], uv[1], 0.5, 1.])
        }
    }
    let (_, shader) = shaders().remove(0);
    let layout = Layout::of(shader.as_ref()).unwrap();
    let c = constants(&layout);
    let targets = [(0, 15), (2, 15)];
    for size in [2, 3] {
        let footprints = Arc::new(Footprints::default());
        let textures = BTreeMap::from([(3, Texture::Image(footprints.clone()))]);
        let grid = Grid {
            size,
            period: [1., 1.],
        };
        let scalar = primase_shader_exec::evaluate_raw(
            shader.as_ref(),
            &c,
            &textures,
            &inputs(),
            grid,
            &targets,
            primase_shader_exec::Invocation::Scalar,
        )
        .unwrap();
        footprints.0.lock().unwrap().clear();
        let quads = primase_shader_exec::evaluate_raw(
            shader.as_ref(),
            &c,
            &textures,
            &inputs(),
            grid,
            &targets,
            primase_shader_exec::Invocation::Quads,
        )
        .unwrap();
        assert_eq!(quads.pixels, scalar.pixels, "grid of {size}");
        // on several threads (a row, or row of quads, each): the same pixels and counts as one
        for invocation in [
            primase_shader_exec::Invocation::Scalar,
            primase_shader_exec::Invocation::Quads,
        ] {
            let one = if invocation == primase_shader_exec::Invocation::Scalar {
                &scalar
            } else {
                &quads
            };
            for n in [2, 8] {
                let many = primase_shader_exec::evaluate_raw_with(
                    primase_core::parallel::Threads::new(n).unwrap(),
                    shader.as_ref(),
                    &c,
                    &textures,
                    &inputs(),
                    grid,
                    &targets,
                    invocation,
                )
                .unwrap();
                assert_eq!(
                    (&many.pixels, many.instructions, many.samples),
                    (&one.pixels, one.instructions, one.samples),
                    "grid of {size}, {invocation:?} on {n} threads"
                );
            }
        }
        footprints.0.lock().unwrap().clear();
        // the footprints again, of one quads run
        primase_shader_exec::evaluate_raw(
            shader.as_ref(),
            &c,
            &textures,
            &inputs(),
            grid,
            &targets,
            primase_shader_exec::Invocation::Quads,
        )
        .unwrap();
        let step = 1. / size as f32;
        let want = [[2. * step, 0.], [0., 4. * step]];
        let seen = footprints.0.lock().unwrap();
        // every lane of every quad samples, helpers included; the gradients differences of f32 coordinates
        assert_eq!(
            seen.len(),
            (size as usize).div_ceil(2).pow(2) * 4,
            "grid of {size}"
        );
        assert!(
            seen.iter()
                .all(|l| matches!(l, primase_shader_exec::Lod::Gradients(g)
                if (0..2).all(|i| (0..2).all(|k| (g[i][k] - want[i][k]).abs() < 1e-6)))),
            "grid of {size}: {seen:?}"
        );
    }
}

/// Writes to o0 what a load of t0 at texel (1, 2), level 0, returns.
#[derive(Debug)]
struct Loads(Interface);
impl PixelShader for Loads {
    fn interface(&self) -> &Interface {
        &self.0
    }
    fn input_count(&self) -> usize {
        0
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &[]
    }
    fn textures(&self) -> &[u32] {
        &[0]
    }
    fn samplers(&self) -> &[u32] {
        &[]
    }
    fn execute(
        &self,
        _: &[[u32; 4]],
        _: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, primase_shader_exec::Error> {
        let value = sampler
            .load(primase_shader_exec::Load {
                texture: 0,
                texel: [1, 2, 0],
                mip: 0,
            })
            .map_err(|detail| primase_shader_exec::Error {
                bytecode: "test",
                at: 0,
                detail,
            })?;
        Ok(Execution {
            outputs: vec![value],
            output_masks: vec![15],
            instructions: 1,
            samples: 0,
            discarded: false,
        })
    }
}

/// A constant texture answers a load with its value's bits, wherever it is loaded; an image refuses a load.
#[test]
fn a_constant_texture_answers_loads() {
    let shader = Loads(Interface {
        stage: Stage::Pixel,
        inputs: vec![],
        outputs: vec![],
        resources: vec![],
        facts: Default::default(),
    });
    let grid = Grid {
        size: 2,
        period: [1.; 2],
    };
    let constants = Constants::new(&Layout::default());
    let run = |texture: Texture| {
        primase_shader_exec::evaluate_raw(
            &shader,
            &constants,
            &BTreeMap::from([(0, texture)]),
            &BTreeMap::new(),
            grid,
            &[(0, 15)],
            primase_shader_exec::Invocation::Scalar,
        )
    };
    let raw = run(Texture::Constant([0.25, -1., 0., 2.])).unwrap();
    let bits = [0.25f32, -1., 0., 2.].map(f32::to_bits);
    assert!(raw.pixels[0].iter().all(|&p| p == (bits, 15)));
    assert!(run(Texture::Image(Arc::new(Coordinates))).is_err());
}
