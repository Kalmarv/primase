//! How a program's interface is carried by its entry point: the one place that says which module argument or result
//! member a varying is, with its name, type and binding. The validator checks a module against it and a lifter builds
//! the module from it.

use naga::Binding;

use crate::{Interface, Label, ScalarType, Stage, StageFacts, SystemValue, Value, Varying};

/// The type of an entry point argument or result member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoType {
    /// A scalar (width 1) or vector.
    Value(Value),
    /// An array of this many floats: the clip distances of a vertex stage, all of its `SV_ClipDistance` outputs
    /// together.
    ClipDistances(u32),
    /// An array of this value: a hull or domain program's control points (one per point), a hull program's outputs.
    Array(Value, u32),
}

/// One argument or result member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoItem {
    /// `<NAME><index>` after the label, or `clip_distances`.
    pub name: String,
    pub ty: IoType,
    pub binding: Binding,
    /// Which varyings of the interface's list (inputs for an argument, outputs for a result) it carries: one, or every
    /// clip distance.
    pub varyings: Vec<usize>,
}

/// The entry point's arguments (one per input varying) and result members (one per output varying, the clip distances
/// being one together), in interface order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryIo {
    pub arguments: Vec<IoItem>,
    pub results: Vec<IoItem>,
}

/// The name a varying has as an argument or result member.
pub fn varying_name(label: &Label) -> String {
    format!("{}{}", label.name, label.index)
}

/// The value the module carries for a varying: the interface's own, except where the pipeline fixes it. A built-in has
/// its API type (a front-facing flag is a bool, the position four floats, an index a u32) and a pixel stage's colour
/// output always has four lanes (D3D lets a target element be narrower than its render target, WebGPU does not).
pub fn carried(stage: Stage, output: bool, v: &Varying) -> Value {
    let (scalar, width) = match v.system {
        Some(SystemValue::ClipPosition | SystemValue::FragCoord) => (ScalarType::Float, 4),
        Some(SystemValue::FrontFacing) => (ScalarType::Bool, 1),
        Some(
            SystemValue::VertexIndex
            | SystemValue::InstanceIndex
            | SystemValue::PrimitiveIndex
            | SystemValue::SampleIndex
            | SystemValue::Coverage,
        ) => (ScalarType::Uint, 1),
        Some(SystemValue::GroupId | SystemValue::GroupThreadId | SystemValue::DispatchThreadId) => {
            (ScalarType::Uint, 3)
        }
        Some(SystemValue::GroupIndex | SystemValue::LaneIndex | SystemValue::LaneCount) => {
            (ScalarType::Uint, 1)
        }
        Some(SystemValue::Depth | SystemValue::DepthGreaterEqual | SystemValue::DepthLessEqual) => {
            (ScalarType::Float, 1)
        }
        Some(SystemValue::Target(_)) if stage == Stage::Pixel && output => (v.value.scalar, 4),
        _ => (v.value.scalar, v.value.width),
    };
    Value { scalar, width }
}

/// The built-in a system value is as an input or output of `stage`; `None` for one the module cannot carry.
fn builtin(stage: Stage, output: bool, system: SystemValue) -> Option<naga::BuiltIn> {
    use SystemValue::*;
    Some(match (stage, output, system) {
        (Stage::Vertex, true, ClipPosition) | (Stage::Pixel, false, FragCoord) => {
            naga::BuiltIn::Position { invariant: false }
        }
        (Stage::Pixel, false, FrontFacing) => naga::BuiltIn::FrontFacing,
        (Stage::Vertex, false, VertexIndex) => naga::BuiltIn::VertexIndex,
        (Stage::Vertex, false, InstanceIndex) => naga::BuiltIn::InstanceIndex,
        (Stage::Pixel, false, PrimitiveIndex) => naga::BuiltIn::PrimitiveIndex,
        (Stage::Pixel, false, SampleIndex) => naga::BuiltIn::SampleIndex,
        (Stage::Pixel, _, Coverage) => naga::BuiltIn::SampleMask,
        (Stage::Compute, false, GroupId) => naga::BuiltIn::WorkGroupId,
        (Stage::Compute, false, GroupThreadId) => naga::BuiltIn::LocalInvocationId,
        (Stage::Compute, false, DispatchThreadId) => naga::BuiltIn::GlobalInvocationId,
        (Stage::Compute, false, GroupIndex) => naga::BuiltIn::LocalInvocationIndex,
        (Stage::Compute | Stage::Pixel, false, LaneIndex) => naga::BuiltIn::SubgroupInvocationId,
        (Stage::Compute | Stage::Pixel, false, LaneCount) => naga::BuiltIn::SubgroupSize,
        // naga has no conservative-depth variants: the depth hints are not carried
        (Stage::Pixel, true, Depth | DepthGreaterEqual | DepthLessEqual) => naga::BuiltIn::FragDepth,
        _ => return None,
    })
}

/// The location binding of a plain varying. A pixel input takes the interface's interpolation; a value that is not a
/// float is always flat, and a flat value has no sampling (naga and WebGPU both insist). A vertex output states the
/// perspective default (or flat, for a value that is not a float) that its consumer's interface is matched against.
fn location(stage: Stage, output: bool, v: &Varying, at: u32) -> Binding {
    let float = v.value.scalar == ScalarType::Float;
    let (interpolation, sampling) = match (stage, output) {
        (Stage::Pixel, false) => {
            let declared = v.interp.map(|i| {
                (
                    match i.mode {
                        crate::InterpMode::Perspective => naga::Interpolation::Perspective,
                        crate::InterpMode::Linear => naga::Interpolation::Linear,
                        crate::InterpMode::Flat => naga::Interpolation::Flat,
                    },
                    match i.sampling {
                        crate::InterpSampling::Center => None,
                        crate::InterpSampling::Centroid => Some(naga::Sampling::Centroid),
                        crate::InterpSampling::Sample => Some(naga::Sampling::Sample),
                    },
                )
            });
            match declared {
                Some((mode, sampling)) if float && mode != naga::Interpolation::Flat => {
                    (Some(mode), sampling)
                }
                Some(_) => (Some(naga::Interpolation::Flat), None),
                None if float => (Some(naga::Interpolation::Perspective), None),
                None => (Some(naga::Interpolation::Flat), None),
            }
        }
        (Stage::Vertex, true) => (
            Some(if float {
                naga::Interpolation::Perspective
            } else {
                naga::Interpolation::Flat
            }),
            None,
        ),
        _ => (None, None),
    };
    Binding::Location {
        location: at,
        interpolation,
        sampling,
        blend_src: None,
        per_primitive: false,
    }
}

/// One argument or result member of a stage function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnItem {
    pub name: String,
    pub ty: IoType,
    /// The varying it carries, by index: into the interface's inputs, outputs or (a hull program's patch-constant
    /// result and a domain program's patch-constant arguments) the patch constants, as the function says; `None`
    /// for the ids the pipeline supplies.
    pub varying: Option<usize>,
}

/// A function of a program that has no entry point: its name, arguments and result members (a structure `Out` when
/// there are any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSig {
    pub name: &'static str,
    pub arguments: Vec<FnItem>,
    pub result: Vec<FnItem>,
}

/// The functions a program of a stage naga has no entry point for is made of; `None` for a stage that is an entry
/// point. The arguments carry no bindings: they are named after the varying they carry, as an entry point's are.
///
/// - A vertex program whose outputs feed a tessellator: `main(inputs) -> Out`.
/// - A hull program: `main(inputs as arrays of the input control points, SV_OutputControlPointID: u32) -> Out`, the
///   control-point phase, run for each output control point; and `patch_constant(inputs as arrays of the input
///   control points, outputs as arrays of the output control points under `out_<name>`, SV_PrimitiveID: u32) -> Out`,
///   the patch-constant phase, run once for a patch, its result the patch constants.
/// - A domain program: `main(patch constants, inputs as arrays of the input control points, SV_DomainLocation:
///   vec3<f32>, SV_PrimitiveID: u32) -> Out`.
/// - A geometry program: `main(inputs as arrays of the primitive's vertices, SV_PrimitiveID: u32, SV_GSInstanceID:
///   u32) -> Out`, the result each output varying as an array of the most vertices an invocation emits, then
///   `SV_VertexCount` (how many were emitted) and `SV_StripStart` (per vertex, 1 where it begins a strip).
pub fn stage_functions(interface: &Interface) -> Result<Option<Vec<FunctionSig>>, String> {
    let stage = interface.stage;
    let values = |list: &[Varying], output: bool| -> Vec<FnItem> {
        list.iter()
            .enumerate()
            .map(|(i, v)| FnItem {
                name: varying_name(&v.label),
                ty: IoType::Value(carried(stage, output, v)),
                varying: Some(i),
            })
            .collect()
    };
    let arrays = |list: &[Varying], output: bool, prefix: &str, n: u32| -> Vec<FnItem> {
        list.iter()
            .enumerate()
            .map(|(i, v)| FnItem {
                name: format!("{prefix}{}", varying_name(&v.label)),
                ty: IoType::Array(carried(stage, output, v), n),
                varying: Some(i),
            })
            .collect()
    };
    let scalar = |name: &str, width: u8, scalar: ScalarType| FnItem {
        name: name.into(),
        ty: IoType::Value(Value { scalar, width }),
        varying: None,
    };
    Ok(match (&interface.stage, &interface.facts) {
        (Stage::Vertex, StageFacts::VertexWithoutPosition) => Some(vec![FunctionSig {
            name: "main",
            arguments: values(&interface.inputs, false),
            result: values(&interface.outputs, true),
        }]),
        (Stage::Hull, StageFacts::Hull(h)) => {
            let mut main_args = arrays(&interface.inputs, false, "", h.input_control_points);
            main_args.push(scalar("SV_OutputControlPointID", 1, ScalarType::Uint));
            let mut constant_args = arrays(&interface.inputs, false, "", h.input_control_points);
            constant_args.extend(arrays(&interface.outputs, true, "out_", h.output_control_points));
            constant_args.push(scalar("SV_PrimitiveID", 1, ScalarType::Uint));
            Some(vec![
                FunctionSig {
                    name: "main",
                    arguments: main_args,
                    result: values(&interface.outputs, true),
                },
                FunctionSig {
                    name: "patch_constant",
                    arguments: constant_args,
                    result: values(&h.patch_constants, true),
                },
            ])
        }
        (Stage::Domain, StageFacts::Domain(d)) => {
            let mut args = values(&d.patch_constants, false);
            args.extend(arrays(&interface.inputs, false, "", d.input_control_points));
            args.push(scalar("SV_DomainLocation", 3, ScalarType::Float));
            args.push(scalar("SV_PrimitiveID", 1, ScalarType::Uint));
            Some(vec![FunctionSig {
                name: "main",
                arguments: args,
                result: values(&interface.outputs, true),
            }])
        }
        (Stage::Hull | Stage::Domain, _) => {
            return Err(format!("a {stage:?} program without its patch facts"));
        }
        (Stage::Geometry, StageFacts::Geometry(g)) => {
            let mut args = arrays(&interface.inputs, false, "", g.input.vertices());
            args.push(scalar("SV_PrimitiveID", 1, ScalarType::Uint));
            args.push(scalar("SV_GSInstanceID", 1, ScalarType::Uint));
            // the emitted vertices: each output varying for every vertex the invocation may emit, how many it did, and
            // whether each starts a new strip (the first, and the first after a cut)
            let mut result = arrays(&interface.outputs, true, "", g.max_vertices);
            result.push(scalar("SV_VertexCount", 1, ScalarType::Uint));
            result.push(FnItem {
                name: "SV_StripStart".into(),
                ty: IoType::Array(
                    Value {
                        scalar: ScalarType::Uint,
                        width: 1,
                    },
                    g.max_vertices,
                ),
                varying: None,
            });
            Some(vec![FunctionSig {
                name: "main",
                arguments: args,
                result,
            }])
        }
        (Stage::Geometry, _) => return Err("a geometry program without its facts".into()),
        _ => None,
    })
}

/// The entry point's arguments and result members for `interface`. `Err` says why the module cannot carry it (a
/// system value no stage binding exists for).
pub fn entry_io(interface: &Interface) -> Result<EntryIo, String> {
    let stage = interface.stage;
    let mut io = EntryIo {
        arguments: Vec::new(),
        results: Vec::new(),
    };
    for output in [false, true] {
        let list = if output {
            &interface.outputs
        } else {
            &interface.inputs
        };
        let items = if output {
            &mut io.results
        } else {
            &mut io.arguments
        };
        let mut next_location = 0;
        let mut clip: Option<usize> = None;
        for (i, v) in list.iter().enumerate() {
            let name = varying_name(&v.label);
            let direction = if output { "output" } else { "input" };
            if v.system == Some(SystemValue::ClipDistance) {
                if stage != Stage::Vertex || !output {
                    return Err(format!(
                        "{name}: clip distances as a {stage:?} {direction} have no built-in binding"
                    ));
                }
                let width = u32::from(v.value.width);
                match clip {
                    Some(at) => {
                        let item = &mut items[at];
                        item.varyings.push(i);
                        let IoType::ClipDistances(n) = &mut item.ty else {
                            unreachable!("the clip item")
                        };
                        *n += width;
                    }
                    None => {
                        clip = Some(items.len());
                        items.push(IoItem {
                            name: "clip_distances".into(),
                            ty: IoType::ClipDistances(width),
                            binding: Binding::BuiltIn(naga::BuiltIn::ClipDistances),
                            varyings: vec![i],
                        });
                    }
                }
                continue;
            }
            let binding = match v.system {
                Some(SystemValue::Target(n)) if stage == Stage::Pixel && output => Binding::Location {
                    location: n,
                    interpolation: None,
                    sampling: None,
                    blend_src: None,
                    per_primitive: false,
                },
                Some(system) => Binding::BuiltIn(builtin(stage, output, system).ok_or_else(|| {
                    format!("{name}: {system:?} as a {stage:?} {direction} has no built-in binding")
                })?),
                None => {
                    next_location += 1;
                    location(stage, output, v, next_location - 1)
                }
            };
            items.push(IoItem {
                name,
                ty: IoType::Value(carried(stage, output, v)),
                binding,
                varyings: vec![i],
            });
        }
    }
    Ok(io)
}
