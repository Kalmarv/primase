#![allow(clippy::unwrap_used)]
//! The program executor on hand-built programs: a scalar run with the derivatives the caller states, a quad's own
//! derivatives, and the requests a 3D sample makes of its sampler.
use naga::{
    AddressSpace, Binding, DerivativeAxis, DerivativeControl, FunctionArgument, GlobalVariable, ImageClass,
    ImageDimension, ResourceBinding, SampleLevel, ScalarKind, StructMember, TypeInner,
};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::exec::{Dim, ImageLod, ImageSample, ProgramShader};
use primase_shader::{
    BufferLayout, Conventions, D3dBinding, D3dClass, D3dModes, D3dSignature, Interface, Label, PixelShader,
    Program, Provenance, Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, Sampler,
    ScalarType, ShaderFormat, Stage, TextureDim, Value, Varying, VaryingProvenance,
};

fn val(expr: naga::Handle<naga::Expression>, kind: Kind, width: u8) -> Val {
    Val { expr, kind, width }
}

fn varying(name: &str, width: u8, mask: u8) -> Varying {
    Varying {
        label: Label {
            name: name.into(),
            index: 0,
        },
        system: None,
        value: Value {
            scalar: ScalarType::Float,
            width,
        },
        interp: Some(primase_shader::Interpolation {
            mode: primase_shader::InterpMode::Perspective,
            sampling: primase_shader::InterpSampling::Center,
        }),
        used: true,
        provenance: VaryingProvenance {
            d3d: Some(D3dSignature {
                register: 0,
                mask,
                read_write_mask: 0,
            }),
        },
    }
}

fn target() -> Varying {
    let mut v = varying("SV_Target", 4, 0xf);
    v.system = Some(primase_shader::SystemValue::Target(0));
    v.interp = None;
    v
}

fn result_type(b: &mut Builder) -> naga::Handle<naga::Type> {
    let f4 = b.ty_of(Kind::F, 4);
    b.ty(TypeInner::Struct {
        members: vec![StructMember {
            name: Some("SV_Target0".into()),
            ty: f4,
            binding: Some(Binding::Location {
                location: 0,
                interpolation: None,
                sampling: None,
                blend_src: None,
                per_primitive: false,
            }),
            offset: 0,
        }],
        span: 16,
    })
}

fn argument(name: &str, ty: naga::Handle<naga::Type>) -> FunctionArgument {
    FunctionArgument {
        name: Some(name.into()),
        ty,
        binding: Some(Binding::Location {
            location: 0,
            interpolation: Some(naga::Interpolation::Perspective),
            sampling: None,
            blend_src: None,
            per_primitive: false,
        }),
    }
}

fn provenance() -> Provenance {
    Provenance {
        format: ShaderFormat::Dxbc,
        target: "ps_5_0".into(),
        lifter: "test".into(),
        lifter_version: 0,
        source_hash: None,
    }
}

/// `SV_Target0 = float4(uv.x * cb0[0].x, uv.y * cb0[0].y, ddx(uv.x), ddy(uv.y))`
fn derivative_program() -> Program {
    let mut b = Builder::new();
    let u4 = b.ty_of(Kind::U, 4);
    let cb_ty = b.ty(TypeInner::Array {
        base: u4,
        size: naga::ArraySize::Constant(std::num::NonZeroU32::new(1).unwrap()),
        stride: 16,
    });
    let cb = b.global(GlobalVariable {
        name: Some("cb0".into()),
        space: AddressSpace::Uniform,
        binding: Some(ResourceBinding { group: 0, binding: 0 }),
        ty: cb_ty,
        init: None,
        memory_decorations: Default::default(),
    });
    let f2 = b.ty_of(Kind::F, 2);
    let uv = b.app(naga::Expression::FunctionArgument(0));
    let uv = val(uv, Kind::F, 2);
    let ptr = b.global_ptr(cb);
    let zero = b.lit_u(0);
    let row = b.app(naga::Expression::Access {
        base: ptr,
        index: zero.expr,
    });
    let row = b.load(row, Kind::U, 4);
    let scale = b.bitcast(row, Kind::F);
    let scale = b.swizzle(scale, &[0, 1]);
    let product = b.binary(naga::BinaryOperator::Multiply, uv, scale);
    let (x, y) = (b.lane(uv, 0), b.lane(uv, 1));
    let dx = b.app(naga::Expression::Derivative {
        axis: DerivativeAxis::X,
        ctrl: DerivativeControl::Coarse,
        expr: x.expr,
    });
    let dy = b.app(naga::Expression::Derivative {
        axis: DerivativeAxis::Y,
        ctrl: DerivativeControl::Coarse,
        expr: y.expr,
    });
    let (px, py) = (b.lane(product, 0), b.lane(product, 1));
    let colour = b.compose(&[px, py, val(dx, Kind::F, 1), val(dy, Kind::F, 1)]);
    let out_ty = result_type(&mut b);
    let out = b.app(naga::Expression::Compose {
        ty: out_ty,
        components: vec![colour.expr],
    });
    b.push(naga::Statement::Return { value: Some(out) });
    let module = b.finish(
        "main",
        naga::ShaderStage::Fragment,
        vec![argument("TEXCOORD0", f2)],
        Some(out_ty),
        false,
    );
    let cb_global = module.global_variables.iter().next().unwrap().0;
    Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 2, 0x3)],
            outputs: vec![target()],
            resources: vec![Resource {
                id: ResourceId(0),
                kind: ResourceKind::ConstantBuffer {
                    layout: BufferLayout {
                        size: 16,
                        constants: vec![],
                    },
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: "cb0".into(),
                    d3d: Some(D3dBinding {
                        class: D3dClass::ConstantBuffer,
                        register: 3,
                        space: 0,
                    }),
                },
            }],
        },
        bindings: vec![(ResourceId(0), cb_global)],
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

/// `SV_Target0 = tex3d.Sample(smp, TEXCOORD0.xyz)`
fn volume_program() -> Program {
    let mut b = Builder::new();
    let image_ty = b.ty(TypeInner::Image {
        dim: ImageDimension::D3,
        arrayed: false,
        class: ImageClass::Sampled {
            kind: ScalarKind::Float,
            multi: false,
        },
    });
    let sampler_ty = b.ty(TypeInner::Sampler { comparison: false });
    let image = b.global(GlobalVariable {
        name: Some("t2".into()),
        space: AddressSpace::Handle,
        binding: Some(ResourceBinding { group: 1, binding: 2 }),
        ty: image_ty,
        init: None,
        memory_decorations: Default::default(),
    });
    let sampler = b.global(GlobalVariable {
        name: Some("s1".into()),
        space: AddressSpace::Handle,
        binding: Some(ResourceBinding { group: 2, binding: 1 }),
        ty: sampler_ty,
        init: None,
        memory_decorations: Default::default(),
    });
    let f3 = b.ty_of(Kind::F, 3);
    let uvw = b.app(naga::Expression::FunctionArgument(0));
    let (image_ptr, sampler_ptr) = (b.global_ptr(image), b.global_ptr(sampler));
    let colour = b.app(naga::Expression::ImageSample {
        image: image_ptr,
        sampler: sampler_ptr,
        gather: None,
        coordinate: uvw,
        array_index: None,
        offset: None,
        level: SampleLevel::Auto,
        depth_ref: None,
        clamp_to_edge: false,
    });
    let out_ty = result_type(&mut b);
    let out = b.app(naga::Expression::Compose {
        ty: out_ty,
        components: vec![colour],
    });
    b.push(naga::Statement::Return { value: Some(out) });
    let module = b.finish(
        "main",
        naga::ShaderStage::Fragment,
        vec![argument("TEXCOORD0", f3)],
        Some(out_ty),
        false,
    );
    let (image, sampler) = {
        let mut globals = module.global_variables.iter();
        (globals.next().unwrap().0, globals.next().unwrap().0)
    };
    let resource = |id: u32, name: &str, kind: ResourceKind, class: D3dClass, register: u32| Resource {
        id: ResourceId(id),
        kind,
        count: 1,
        provenance: ResourceProvenance {
            name: name.into(),
            d3d: Some(D3dBinding {
                class,
                register,
                space: 0,
            }),
        },
    };
    Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 3, 0x7)],
            outputs: vec![target()],
            resources: vec![
                resource(
                    0,
                    "volume",
                    ResourceKind::Texture {
                        dim: TextureDim::D3,
                        arrayed: false,
                        multisampled: false,
                        sample: SampleType::Float,
                    },
                    D3dClass::ShaderResource,
                    2,
                ),
                resource(
                    1,
                    "linear",
                    ResourceKind::Sampler { comparison: false },
                    D3dClass::Sampler,
                    1,
                ),
            ],
        },
        bindings: vec![(ResourceId(0), image), (ResourceId(1), sampler)],
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

struct Unused;
impl Sampler for Unused {
    fn sample(&mut self, _: primase_shader::Sample) -> Result<[f32; 4], String> {
        Err("no samples expected".into())
    }
}

fn bits(x: [f32; 4]) -> [u32; 4] {
    x.map(f32::to_bits)
}

fn lanes(x: [u32; 4]) -> [f32; 4] {
    x.map(f32::from_bits)
}

#[test]
fn a_scalar_run_reads_the_derivatives_the_caller_states() {
    let shader = ProgramShader::new(derivative_program()).unwrap();
    assert_eq!(shader.input_count(), 1);
    assert_eq!(shader.constant_buffers(), &[(3, 1)]);
    let cb = [bits([2.0, 3.0, 0.0, 0.0])];
    let inputs = [bits([0.25, 0.5, 0.0, 0.0])];
    let derivatives = [[bits([0.125, 0.0, 0.0, 0.0]), bits([0.0, 0.0625, 0.0, 0.0])]];
    let e = shader
        .execute_with_derivatives(&inputs, &derivatives, &[(3, &cb)], &mut Unused)
        .unwrap();
    assert_eq!(lanes(e.outputs[0]), [0.5, 1.5, 0.125, 0.0625]);
    assert_eq!(e.output_masks, vec![0xf]);
    // the derivatives of an input are an error where they are not given
    let e = shader.execute(&inputs, &[(3, &cb)], &mut Unused).unwrap_err();
    assert!(e.detail.contains("derivatives"), "{e}");
    // a missing constant buffer
    let e = shader.execute(&inputs, &[], &mut Unused).unwrap_err();
    assert!(e.detail.contains("cb3"), "{e}");
}

#[test]
fn a_quad_takes_its_own_derivatives_from_its_four_lanes() {
    let shader = ProgramShader::new(derivative_program()).unwrap();
    let cb = [bits([2.0, 3.0, 0.0, 0.0])];
    // lanes 0, 1 the top row, 2, 3 the bottom: x grows 0.125 to the right, y 0.0625 downwards
    let lane = |k: u32| {
        [bits([
            0.25 + (k & 1) as f32 * 0.125,
            0.5 + (k >> 1) as f32 * 0.0625,
            0.0,
            0.0,
        ])]
    };
    let inputs = [lane(0), lane(1), lane(2), lane(3)];
    let e = shader
        .execute_quad(
            [&inputs[0], &inputs[1], &inputs[2], &inputs[3]],
            &[(3, &cb)],
            &mut Unused,
        )
        .unwrap();
    for (k, e) in e.iter().enumerate() {
        let (x, y) = (0.25 + (k & 1) as f32 * 0.125, 0.5 + (k >> 1) as f32 * 0.0625);
        assert_eq!(lanes(e.outputs[0]), [x * 2.0, y * 3.0, 0.125, 0.0625], "lane {k}");
    }
}

#[derive(Default)]
struct Recording {
    requests: Vec<ImageSample<u32>>,
}
impl Sampler for Recording {
    fn sample(&mut self, _: primase_shader::Sample) -> Result<[f32; 4], String> {
        Err("a plain 2D sample was not expected".into())
    }
    fn sample_image(&mut self, request: ImageSample<u32>) -> Result<[f32; 4], String> {
        self.requests.push(request);
        Ok([0.5, 0.25, 0.125, 1.0])
    }
}

#[test]
fn a_volume_sample_reaches_a_sampler_that_answers_it_and_is_refused_by_one_that_does_not() {
    let shader = ProgramShader::new(volume_program()).unwrap();
    assert_eq!(shader.textures(), &[2]);
    assert_eq!(shader.samplers(), &[1]);
    let inputs = [bits([0.1, 0.2, 0.3, 0.0])];
    let mut sampler = Recording::default();
    let e = shader.execute(&inputs, &[], &mut sampler).unwrap();
    assert_eq!(lanes(e.outputs[0]), [0.5, 0.25, 0.125, 1.0]);
    assert_eq!(e.samples, 1);
    let [r] = sampler.requests.as_slice() else {
        panic!("{:?}", sampler.requests)
    };
    assert_eq!((r.texture, r.sampler, r.dim), (2, 1, Dim::D3));
    assert_eq!(r.coord, [0.1, 0.2, 0.3]);
    assert_eq!(r.lod, ImageLod::Implicit);
    assert!(r.offset == [0; 3] && r.gather.is_none() && r.depth_ref.is_none() && !r.depth);
    // the default sampler answers plain 2D samples only
    let e = shader.execute(&inputs, &[], &mut Unused).unwrap_err();
    assert!(e.detail.contains("plain 2D samples only"), "{e}");
}
