//! The program validator: naga's own validation of the module, then the rules naga cannot know because they are about
//! primase's vocabulary: the interface against the entry point, the resources against the globals.

use std::collections::BTreeSet;
use std::fmt;

use naga::{AddressSpace, Binding, BuiltIn, ImageClass, ScalarKind, ShaderStage, TypeInner};

use super::Program;
use crate::Stage;
use crate::{
    Access, BufferKind, ResourceKind, SampleType, ScalarType, StorageTarget, SystemValue, TextureDim, Value,
    Varying,
};

/// One rule a program broke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// naga's validator refused the module.
    Module(String),
    /// The entry point, the interface or a resource disagrees with the module.
    Program(String),
}
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Module(m) => write!(f, "module: {m}"),
            Problem::Program(m) => write!(f, "program: {m}"),
        }
    }
}

/// The capabilities programs may use: those WebGPU's WGSL has without extensions, so that anything that passes
/// can be written as WGSL.
fn capabilities() -> naga::valid::Capabilities {
    naga::valid::Capabilities::empty()
}

/// Check `program`: naga's validation of its module, then the program rules. Every problem found is returned.
pub fn validate(program: &Program) -> Result<naga::valid::ModuleInfo, Vec<Problem>> {
    let mut problems = Vec::new();
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), capabilities())
        .validate(&program.module)
    {
        Ok(info) => Some(info),
        Err(e) => {
            problems.push(Problem::Module(e.as_inner().to_string()));
            None
        }
    };
    interface(program, &mut problems);
    resources(program, &mut problems);
    match info {
        Some(info) if problems.is_empty() => Ok(info),
        _ => Err(problems),
    }
}

fn scalar_of(s: ScalarType) -> Option<naga::Scalar> {
    Some(match s {
        ScalarType::Bool => naga::Scalar::BOOL,
        ScalarType::Sint => naga::Scalar::I32,
        ScalarType::Uint => naga::Scalar::U32,
        ScalarType::Float => naga::Scalar::F32,
        ScalarType::Double => return None,
    })
}

/// Whether a naga type is the value `v`: a scalar for width 1, a vector otherwise. A bool varying is a `bool` in the
/// module even though it takes 32 bits where it is stored.
fn is_value(inner: &TypeInner, v: Value) -> bool {
    let Some(scalar) = scalar_of(v.scalar) else {
        return false;
    };
    match (inner, v.width) {
        (TypeInner::Scalar(s), 1) => *s == scalar,
        (TypeInner::Vector { size, scalar: s }, w @ 2..=4) => *s == scalar && *size as u8 == w,
        _ => false,
    }
}

/// The value the module carries for a varying: the interface's own, except where the pipeline fixes it. A built-in has
/// its API type (a front-facing flag is a bool, the position four floats, an index a u32) and a pixel stage's colour
/// output always has four lanes (D3D lets a target element be narrower than its render target, WebGPU does not).
pub(crate) fn carried(stage: Stage, output: bool, v: &Varying) -> Value {
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
        Some(SystemValue::Depth) => (ScalarType::Float, 1),
        Some(SystemValue::Target(_)) if stage == Stage::Pixel && output => (v.value.scalar, 4),
        _ => (v.value.scalar, v.value.width),
    };
    Value { scalar, width }
}

/// The binding a system value has as an input or output of `stage`; `None` for one the module cannot carry.
fn builtin(stage: Stage, output: bool, system: SystemValue) -> Option<BuiltIn> {
    use SystemValue::*;
    Some(match (stage, output, system) {
        (Stage::Vertex, true, ClipPosition) | (Stage::Pixel, false, FragCoord) => {
            BuiltIn::Position { invariant: false }
        }
        (Stage::Pixel, false, FrontFacing) => BuiltIn::FrontFacing,
        (Stage::Vertex, false, VertexIndex) => BuiltIn::VertexIndex,
        (Stage::Vertex, false, InstanceIndex) => BuiltIn::InstanceIndex,
        (Stage::Pixel, false, PrimitiveIndex) => BuiltIn::PrimitiveIndex,
        (Stage::Pixel, false, SampleIndex) => BuiltIn::SampleIndex,
        (Stage::Pixel, _, Coverage) => BuiltIn::SampleMask,
        (Stage::Pixel, true, Depth) => BuiltIn::FragDepth,
        _ => return None,
    })
}

fn interpolation(v: &Varying) -> (Option<naga::Interpolation>, Option<naga::Sampling>) {
    let Some(i) = v.interp else {
        return (None, None);
    };
    (
        Some(match i.mode {
            crate::InterpMode::Perspective => naga::Interpolation::Perspective,
            crate::InterpMode::Linear => naga::Interpolation::Linear,
            crate::InterpMode::Flat => naga::Interpolation::Flat,
        }),
        match i.sampling {
            crate::InterpSampling::Center => None,
            crate::InterpSampling::Centroid => Some(naga::Sampling::Centroid),
            crate::InterpSampling::Sample => Some(naga::Sampling::Sample),
        },
    )
}

/// The binding a varying must have. `location` is the next free `@location`, used up when the varying takes one.
/// `Err` says why the module cannot carry it.
pub(crate) fn binding_of(
    stage: Stage,
    output: bool,
    v: &Varying,
    location: &mut u32,
) -> Result<Binding, String> {
    match v.system {
        Some(SystemValue::Target(n)) if stage == Stage::Pixel && output => Ok(Binding::Location {
            location: n,
            interpolation: None,
            sampling: None,
            blend_src: None,
            per_primitive: false,
        }),
        Some(system) => builtin(stage, output, system)
            .map(Binding::BuiltIn)
            .ok_or_else(|| {
                format!(
                    "{:?} as a {stage:?} {} has no built-in binding",
                    system,
                    if output { "output" } else { "input" }
                )
            }),
        None => {
            let (interpolation, sampling) = if stage == Stage::Pixel && !output {
                interpolation(v)
            } else {
                (None, None)
            };
            let at = *location;
            *location += 1;
            Ok(Binding::Location {
                location: at,
                interpolation,
                sampling,
                blend_src: None,
                per_primitive: false,
            })
        }
    }
}

fn interface(p: &Program, problems: &mut Vec<Problem>) {
    let mut fail = |m: String| problems.push(Problem::Program(m));
    let [entry] = p.module.entry_points.as_slice() else {
        return fail(format!(
            "the module has {} entry points, not one",
            p.module.entry_points.len()
        ));
    };
    if entry.name != p.entry {
        fail(format!(
            "the entry point is named {}, not {}",
            entry.name, p.entry
        ));
    }
    let stage = match entry.stage {
        ShaderStage::Vertex => Some(crate::Stage::Vertex),
        ShaderStage::Fragment => Some(crate::Stage::Pixel),
        _ => None,
    };
    if stage != Some(p.stage) || p.interface.stage != p.stage {
        fail(format!(
            "stages disagree: the entry point is {:?}, the program {:?}, its interface {:?}",
            entry.stage, p.stage, p.interface.stage
        ));
        return;
    }
    let function = &entry.function;
    let types = &p.module.types;

    // inputs: one argument each
    if function.arguments.len() != p.interface.inputs.len() {
        fail(format!(
            "{} entry arguments for {} input varyings",
            function.arguments.len(),
            p.interface.inputs.len()
        ));
    }
    let mut location = 0;
    for (v, arg) in p.interface.inputs.iter().zip(&function.arguments) {
        let name = Program::varying_name(&v.label);
        if arg.name.as_deref() != Some(name.as_str()) {
            fail(format!("input {name}: the argument is named {:?}", arg.name));
        }
        if !is_value(&types[arg.ty].inner, carried(p.stage, false, v)) {
            fail(format!(
                "input {name}: the argument's type is not {:?}",
                carried(p.stage, false, v)
            ));
        }
        match binding_of(p.stage, false, v, &mut location) {
            Ok(want) if arg.binding.as_ref() != Some(&want) => {
                fail(format!(
                    "input {name}: binding {:?}, expected {want:?}",
                    arg.binding
                ));
            }
            Ok(_) => {}
            Err(e) => fail(format!("input {name}: {e}")),
        }
    }

    // outputs: the members of the result structure
    let mut location = 0;
    match (&function.result, p.interface.outputs.is_empty()) {
        (None, true) => {}
        (None, false) => fail("outputs declared but the entry point returns nothing".into()),
        (Some(_), true) => fail("the entry point returns a value but the interface has no outputs".into()),
        (Some(result), false) => match &types[result.ty].inner {
            TypeInner::Struct { members, .. } => {
                if members.len() != p.interface.outputs.len() {
                    fail(format!(
                        "{} result members for {} output varyings",
                        members.len(),
                        p.interface.outputs.len()
                    ));
                }
                for (v, m) in p.interface.outputs.iter().zip(members) {
                    let name = Program::varying_name(&v.label);
                    if m.name.as_deref() != Some(name.as_str()) {
                        fail(format!("output {name}: the member is named {:?}", m.name));
                    }
                    if !is_value(&types[m.ty].inner, carried(p.stage, true, v)) {
                        fail(format!(
                            "output {name}: the member's type is not {:?}",
                            carried(p.stage, true, v)
                        ));
                    }
                    match binding_of(p.stage, true, v, &mut location) {
                        Ok(want) if m.binding.as_ref() != Some(&want) => {
                            fail(format!(
                                "output {name}: binding {:?}, expected {want:?}",
                                m.binding
                            ));
                        }
                        Ok(_) => {}
                        Err(e) => fail(format!("output {name}: {e}")),
                    }
                }
            }
            _ => fail("the entry point's result is not a structure".into()),
        },
    }
    // labels are unique within a direction
    for (what, list) in [("input", &p.interface.inputs), ("output", &p.interface.outputs)] {
        let mut seen = BTreeSet::new();
        for v in list.iter() {
            if !seen.insert(&v.label) {
                fail(format!(
                    "{what} label {} repeats",
                    Program::varying_name(&v.label)
                ));
            }
        }
    }
}

fn texture_dim(dim: TextureDim) -> naga::ImageDimension {
    match dim {
        TextureDim::D1 => naga::ImageDimension::D1,
        TextureDim::D2 => naga::ImageDimension::D2,
        TextureDim::D3 => naga::ImageDimension::D3,
        TextureDim::Cube => naga::ImageDimension::Cube,
    }
}

fn resources(p: &Program, problems: &mut Vec<Problem>) {
    let mut fail = |m: String| problems.push(Problem::Program(m));
    let mut bound = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for r in &p.interface.resources {
        let id = r.id.0;
        let Some(handle) = p.global(r.id) else {
            fail(format!("resource {id} ({}) has no global", r.provenance.name));
            continue;
        };
        if !bound.insert(handle) {
            fail(format!("resource {id}: its global is another resource's too"));
        }
        let Some(global) = p.module.global_variables.try_get(handle).ok() else {
            fail(format!("resource {id}: its global is not in the module"));
            continue;
        };
        match &global.binding {
            Some(b) => {
                if !slots.insert((b.group, b.binding)) {
                    fail(format!(
                        "resource {id}: group {} binding {} is used twice",
                        b.group, b.binding
                    ));
                }
            }
            None => fail(format!("resource {id}: its global has no binding")),
        }
        if r.count != 1 {
            fail(format!(
                "resource {id}: arrays of resources (count {}) are not carried",
                r.count
            ));
        }
        let inner = &p.module.types[global.ty].inner;
        let space = global.space;
        let ok = match &r.kind {
            ResourceKind::ConstantBuffer { layout } => {
                let rows = layout.size.div_ceil(16);
                space == AddressSpace::Uniform
                    && matches!(inner, TypeInner::Array { base, size: naga::ArraySize::Constant(n), stride: 16 }
                        if n.get() >= rows
                            && p.module.types[*base].inner
                                == TypeInner::Vector { size: naga::VectorSize::Quad, scalar: naga::Scalar::U32 })
            }
            ResourceKind::Texture {
                dim,
                arrayed,
                multisampled,
                sample,
            } => {
                let class = match sample {
                    SampleType::Float => ImageClass::Sampled {
                        kind: ScalarKind::Float,
                        multi: *multisampled,
                    },
                    SampleType::Sint => ImageClass::Sampled {
                        kind: ScalarKind::Sint,
                        multi: *multisampled,
                    },
                    SampleType::Uint => ImageClass::Sampled {
                        kind: ScalarKind::Uint,
                        multi: *multisampled,
                    },
                    SampleType::Depth => ImageClass::Depth { multi: *multisampled },
                };
                space == AddressSpace::Handle
                    && *inner
                        == TypeInner::Image {
                            dim: texture_dim(*dim),
                            arrayed: *arrayed,
                            class,
                        }
            }
            ResourceKind::Sampler { comparison } => {
                space == AddressSpace::Handle
                    && *inner
                        == TypeInner::Sampler {
                            comparison: *comparison,
                        }
            }
            ResourceKind::Buffer(kind) => {
                matches!(space, AddressSpace::Storage { access } if access == naga::StorageAccess::LOAD)
                    && buffer_fits(&p.module, inner, *kind)
            }
            ResourceKind::Storage { target, access } => {
                let want = match access {
                    Access::Read => naga::StorageAccess::LOAD,
                    Access::Write => naga::StorageAccess::STORE,
                    Access::ReadWrite => naga::StorageAccess::LOAD | naga::StorageAccess::STORE,
                };
                match target {
                    StorageTarget::Buffer(kind) => {
                        matches!(space, AddressSpace::Storage { access } if access == want)
                            && buffer_fits(&p.module, inner, *kind)
                    }
                    StorageTarget::Image { dim, arrayed, .. } => {
                        space == AddressSpace::Handle
                            && matches!(inner, TypeInner::Image { dim: d, arrayed: a, class: ImageClass::Storage { access, .. } }
                                if *d == texture_dim(*dim) && *a == *arrayed && *access == want)
                    }
                }
            }
        };
        if !ok {
            fail(format!(
                "resource {id} ({}): the global {:?} in {space:?} is not a {:?}",
                r.provenance.name, global.name, r.kind
            ));
        }
    }
    let ids: BTreeSet<_> = p.interface.resources.iter().map(|r| r.id).collect();
    if ids.len() != p.interface.resources.len() {
        fail("resource ids repeat".into());
    }
    for (id, _) in &p.bindings {
        if !ids.contains(id) {
            fail(format!(
                "binding for resource {} which the interface does not have",
                id.0
            ));
        }
    }
    for (h, g) in p.module.global_variables.iter() {
        let binds = !matches!(
            g.space,
            AddressSpace::Private | AddressSpace::WorkGroup | AddressSpace::Function
        );
        if binds && !bound.contains(&h) {
            fail(format!(
                "global {:?} is bound but is no resource of the interface",
                g.name
            ));
        }
    }
}

/// Whether `inner` is a runtime array a buffer of `kind` reads. A buffer is addressed in words, whatever its elements:
/// raw and structured buffers are `array<u32>` (a structured element takes `stride / 4` words), a typed buffer is
/// `array<vec4<u32>>` of the four lanes its loads return, as bits (converting from the view's format is the binding's
/// business).
fn buffer_fits(module: &naga::Module, inner: &TypeInner, kind: BufferKind) -> bool {
    let TypeInner::Array {
        base,
        size: naga::ArraySize::Dynamic,
        stride,
    } = inner
    else {
        return false;
    };
    let base = &module.types[*base].inner;
    match kind {
        BufferKind::Raw => *base == TypeInner::Scalar(naga::Scalar::U32) && *stride == 4,
        BufferKind::Structured { stride: s } => {
            *base == TypeInner::Scalar(naga::Scalar::U32) && *stride == 4 && s % 4 == 0
        }
        BufferKind::Typed(v) => {
            v.width == 4
                && *stride == 16
                && *base
                    == TypeInner::Vector {
                        size: naga::VectorSize::Quad,
                        scalar: naga::Scalar::U32,
                    }
        }
    }
}
