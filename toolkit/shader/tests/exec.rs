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
    assert!(shader.needs_quad());
    assert!(!ProgramShader::new(branch_program()).unwrap().needs_quad());
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
    // a volume is reached through sample_image: textures() lists the planes Sampler::sample answers
    assert_eq!(shader.textures(), &[] as &[u32]);
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

/// `if (uv.x > 0.5) { return red; } return green;`: two returns with results of their own.
fn branch_program() -> Program {
    let mut b = Builder::new();
    let f2 = b.ty_of(Kind::F, 2);
    let uv = b.app(naga::Expression::FunctionArgument(0));
    let uv = val(uv, Kind::F, 2);
    let x = b.lane(uv, 0);
    let half = b.lit_f(0.5);
    let above = b.binary(naga::BinaryOperator::Greater, x, half);
    let (one, zero) = (b.lit_f(1.0), b.lit_f(0.0));
    let red = b.compose(&[one, zero, zero, one]);
    let green = b.compose(&[zero, one, zero, one]);
    let out_ty = result_type(&mut b);
    let red_out = b.app(naga::Expression::Compose {
        ty: out_ty,
        components: vec![red.expr],
    });
    let green_out = b.app(naga::Expression::Compose {
        ty: out_ty,
        components: vec![green.expr],
    });
    let mut early = naga::Block::new();
    early.push(
        naga::Statement::Return { value: Some(red_out) },
        naga::Span::UNDEFINED,
    );
    b.push(naga::Statement::If {
        condition: above.expr,
        accept: early,
        reject: naga::Block::new(),
    });
    b.push(naga::Statement::Return {
        value: Some(green_out),
    });
    let module = b.finish(
        "main",
        naga::ShaderStage::Fragment,
        vec![argument("TEXCOORD0", f2)],
        Some(out_ty),
        false,
    );
    Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 2, 0x3)],
            outputs: vec![target()],
            resources: Vec::new(),
        },
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

#[test]
fn each_return_leaves_with_its_own_result() {
    let shader = ProgramShader::new(branch_program()).unwrap();
    let run = |x: f32| {
        let inputs = [bits([x, 0.0, 0.0, 0.0])];
        lanes(shader.execute(&inputs, &[], &mut Unused).unwrap().outputs[0])
    };
    assert_eq!(run(0.75), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(run(0.25), [0.0, 1.0, 0.0, 1.0]);
    // a quad whose lanes leave at different returns
    let lane = |x: f32| vec![bits([x, 0.0, 0.0, 0.0])];
    let inputs = [lane(0.25), lane(0.75), lane(0.25), lane(0.75)];
    let e = shader
        .execute_quad([&inputs[0], &inputs[1], &inputs[2], &inputs[3]], &[], &mut Unused)
        .unwrap();
    let colours: Vec<[f32; 4]> = e.iter().map(|e| lanes(e.outputs[0])).collect();
    assert_eq!(
        colours,
        [
            [0.0, 1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0]
        ]
    );
}

/// `SV_Target0 = float4(uv.x, uv.y, uv.y, uv.x)`, the argument named by two expressions: both are the one argument.
#[test]
fn an_argument_named_by_several_expressions_is_the_same_value_in_each() {
    let mut b = Builder::new();
    let f2 = b.ty_of(Kind::F, 2);
    let first = b.app(naga::Expression::FunctionArgument(0));
    let second = b.app(naga::Expression::FunctionArgument(0));
    let x = b.lane(val(first, Kind::F, 2), 0);
    let y = b.lane(val(second, Kind::F, 2), 1);
    let colour = b.compose(&[x, y, y, x]);
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
    let program = Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 2, 0x3)],
            outputs: vec![target()],
            resources: Vec::new(),
        },
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    };
    let shader = ProgramShader::new(program).unwrap();
    let e = shader
        .execute(&[bits([0.25, 0.75, 0.0, 0.0])], &[], &mut Unused)
        .unwrap();
    assert_eq!(lanes(e.outputs[0]), [0.25, 0.75, 0.75, 0.25]);
}

/// `SV_Target0 = tex[uint(x)].SampleLevel(smp[uint(x) + 1], (0.5, 0.5), 0)` with four textures and an unbounded
/// sampler array: both picked by an index the program computes.
fn array_program() -> Program {
    let mut b = Builder::new();
    let image_ty = b.ty(TypeInner::Image {
        dim: ImageDimension::D2,
        arrayed: false,
        class: ImageClass::Sampled {
            kind: ScalarKind::Float,
            multi: false,
        },
    });
    let sampler_ty = b.ty(TypeInner::Sampler { comparison: false });
    let images = b.ty(TypeInner::BindingArray {
        base: image_ty,
        size: naga::ArraySize::Constant(std::num::NonZeroU32::new(4).unwrap()),
    });
    let samplers = b.ty(TypeInner::BindingArray {
        base: sampler_ty,
        size: naga::ArraySize::Dynamic,
    });
    let global = |b: &mut Builder, name: &str, group, ty| {
        b.global(GlobalVariable {
            name: Some(name.into()),
            space: AddressSpace::Handle,
            binding: Some(ResourceBinding { group, binding: 0 }),
            ty,
            init: None,
            memory_decorations: Default::default(),
        })
    };
    let image = global(&mut b, "t0", 1, images);
    let sampler = global(&mut b, "s0", 2, samplers);
    let f1 = b.ty_of(Kind::F, 1);
    let x = b.app(naga::Expression::FunctionArgument(0));
    let element = b.convert(val(x, Kind::F, 1), Kind::U);
    let one = b.lit_u(1);
    let next = b.binary(naga::BinaryOperator::Add, element, one);
    let (image_base, sampler_base) = (b.global_ptr(image), b.global_ptr(sampler));
    let image_ptr = b.app(naga::Expression::Access {
        base: image_base,
        index: element.expr,
    });
    let sampler_ptr = b.app(naga::Expression::Access {
        base: sampler_base,
        index: next.expr,
    });
    let half = b.lit_f(0.5);
    let uv = b.compose(&[half, half]);
    let colour = b.app(naga::Expression::ImageSample {
        image: image_ptr,
        sampler: sampler_ptr,
        gather: None,
        coordinate: uv.expr,
        array_index: None,
        offset: None,
        level: SampleLevel::Zero,
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
        vec![argument("IDX0", f1)],
        Some(out_ty),
        false,
    );
    let (image, sampler) = {
        let mut globals = module.global_variables.iter();
        (globals.next().unwrap().0, globals.next().unwrap().0)
    };
    let resource = |id: u32, name: &str, kind: ResourceKind, class: D3dClass, count: u32| Resource {
        id: ResourceId(id),
        kind,
        count,
        provenance: ResourceProvenance {
            name: name.into(),
            d3d: Some(D3dBinding {
                class,
                register: 0,
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
            inputs: vec![varying("IDX", 1, 0x1)],
            outputs: vec![target()],
            resources: vec![
                resource(
                    0,
                    "tex",
                    ResourceKind::Texture {
                        dim: TextureDim::D2,
                        arrayed: false,
                        multisampled: false,
                        sample: SampleType::Float,
                    },
                    D3dClass::ShaderResource,
                    4,
                ),
                resource(
                    1,
                    "smp",
                    ResourceKind::Sampler { comparison: false },
                    D3dClass::Sampler,
                    Resource::UNBOUNDED,
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

/// Answers a sample with the elements it names: (texture element, sampler element, 0, 1).
struct Elements;
impl primase_shader::exec::Backend for Elements {
    fn sample(&mut self, r: ImageSample<primase_shader::exec::Element>) -> Result<[f32; 4], String> {
        assert_eq!((r.texture.id, r.sampler.id), (ResourceId(0), ResourceId(1)));
        Ok([r.texture.index as f32, r.sampler.index as f32, 0.0, 1.0])
    }
}

/// Four lanes, each with its own index: a divergent index picks a different element in each, and an index past the
/// array's four textures stops the run naming the resource and the index. The unbounded sampler array has no length
/// to check here (the caller's to check): sampler 5 is asked for by texture 4.
#[test]
fn an_index_the_program_computes_picks_an_element_of_a_resource_array() {
    use primase_shader::exec::{Executable, Resources, Runner};
    let program = array_program();
    program.validate().unwrap();
    let exe = Executable::new(&program).unwrap();
    let resources = Resources::default();
    let bound = exe.bind_resources(&resources).unwrap();
    let mut backend = Elements;
    let mut quad = Runner::<4>::new(&exe, &bound, &mut backend, None);
    let arg = |x: f32| [x.to_bits(), 0, 0, 0];
    let out = quad
        .run([&[arg(0.0)][..], &[arg(3.0)][..], &[arg(1.0)][..], &[arg(2.0)][..]])
        .unwrap();
    let lanes: Vec<[f32; 4]> = out.iter().map(|o| lanes(o.values[0])).collect();
    assert_eq!(
        lanes,
        [
            [0.0, 1.0, 0.0, 1.0],
            [3.0, 4.0, 0.0, 1.0],
            [1.0, 2.0, 0.0, 1.0],
            [2.0, 3.0, 0.0, 1.0]
        ]
    );
    let mut one = Runner::<1>::new(&exe, &bound, &mut backend, None);
    let error = one.run([&[arg(4.0)][..]]).unwrap_err();
    assert!(
        error.contains("resource 0: element 4 of an array of 4"),
        "{error}"
    );
}

/// The program writes as WGSL with its arrays of bindings, and the interface's count must be the global's.
#[test]
fn a_resource_array_is_written_and_its_count_is_checked() {
    let mut program = array_program();
    let wgsl = primase_shader::emit::wgsl(&program).unwrap();
    assert!(wgsl.contains("binding_array<texture_2d<f32>, 4>"), "{wgsl}");
    assert!(wgsl.contains("binding_array<sampler>"), "{wgsl}");
    program.interface.resources[0].count = 3;
    let problems = program
        .validate()
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("resource 0: the global's array size")),
        "{problems:?}"
    );
    program.interface.resources[0].count = 1;
    let problems = program
        .validate()
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("resource 0 (tex): the global")),
        "{problems:?}"
    );
}

/// A pixel program over `uv` (TEXCOORD0, two lanes) whose `SV_Target0` is what `build` makes of the lanes.
fn program_of(build: impl FnOnce(&mut Builder, Val, Val) -> Val) -> Program {
    let mut b = Builder::new();
    let f2 = b.ty_of(Kind::F, 2);
    let uv = b.app(naga::Expression::FunctionArgument(0));
    let uv = val(uv, Kind::F, 2);
    let (x, y) = (b.lane(uv, 0), b.lane(uv, 1));
    let colour = build(&mut b, x, y);
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
    Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 2, 0x3)],
            outputs: vec![target()],
            resources: Vec::new(),
        },
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

/// The inverse trigonometric and hyperbolic functions, the fused multiply-add and the float remainder, against the
/// values f64 gives rounded once (and C's fmod: the sign of the dividend).
#[test]
fn trigonometry_fma_and_the_float_remainder() {
    use naga::MathFunction as Mf;
    let run = |program: Program, x: f32, y: f32| {
        let shader = ProgramShader::new(program).unwrap();
        lanes(
            shader
                .execute(&[bits([x, y, 0.0, 0.0])], &[], &mut Unused)
                .unwrap()
                .outputs[0],
        )
    };
    let unary = program_of(|b, x, _| {
        let f = |b: &mut Builder, fun| b.math(fun, &[x]);
        let parts = [Mf::Acos, Mf::Asin, Mf::Atan, Mf::Tan].map(|fun| f(b, fun));
        b.compose(&parts)
    });
    let hyperbolic = program_of(|b, x, _| {
        let parts = [Mf::Sinh, Mf::Cosh, Mf::Tanh, Mf::Sinh].map(|fun| b.math(fun, &[x]));
        b.compose(&parts)
    });
    let x = 0.3f32;
    let want = |f: fn(f64) -> f64| f(x as f64) as f32;
    assert_eq!(
        run(unary, x, 0.0),
        [want(f64::acos), want(f64::asin), want(f64::atan), want(f64::tan)]
    );
    assert_eq!(
        run(hyperbolic, x, 0.0),
        [want(f64::sinh), want(f64::cosh), want(f64::tanh), want(f64::sinh)]
    );
    let remainder = program_of(|b, x, y| {
        let rem = b.binary(naga::BinaryOperator::Modulo, x, y);
        let neg = b.unary(naga::UnaryOperator::Negate, x);
        let rem_neg = b.binary(naga::BinaryOperator::Modulo, neg, y);
        let z = b.lit_f(0.5);
        let fused = b.math(Mf::Fma, &[x, y, z]);
        b.compose(&[rem, rem_neg, fused, z])
    });
    assert_eq!(
        run(remainder, 5.5, 2.0),
        [1.5, -1.5, 5.5f32.mul_add(2.0, 0.5), 0.5]
    );
}
