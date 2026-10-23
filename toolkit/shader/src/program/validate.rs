//! The program validator: naga's own validation of the module, then the rules naga cannot know because they are about
//! primase's vocabulary: the interface against the entry point, the resources against the globals.

use std::collections::BTreeSet;
use std::fmt;

use naga::{AddressSpace, ImageClass, ScalarKind, ShaderStage, TypeInner};

use super::Program;
use super::io::{FunctionSig, IoType, entry_io, stage_functions};
use crate::{Access, BufferKind, ResourceKind, SampleType, ScalarType, StorageTarget, TextureDim, Value};

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

/// The capabilities programs may use: WGSL's baseline and what a D3D program can need beyond it (clip distances,
/// per-sample shading, the primitive index, cube array textures, half-float packing for `f16tof32`/`f32tof16`, forced
/// early depth tests, arrays of textures and samplers indexed by a computed, possibly divergent, index, and subgroup
/// (wave) operations in compute and pixel programs, atomic operations on storage textures). A consumer that cannot provide one refuses the program that
/// needs it; nothing outside this list is ever produced.
///
/// A compute program's thread group is validated against D3D's limits (1024 invocations, 1024 along x and y, 64 along
/// z), not WebGPU's guaranteed ones (256, 256, 256, 64, and 16 KiB of workgroup memory): a program beyond those says
/// so in its notes ([`crate::compute_limit_notes`]) and a consumer checks them against its device's limits.
pub const CAPABILITIES: naga::valid::Capabilities = naga::valid::Capabilities::CLIP_DISTANCES
    .union(naga::valid::Capabilities::MULTISAMPLED_SHADING)
    .union(naga::valid::Capabilities::PRIMITIVE_INDEX)
    .union(naga::valid::Capabilities::CUBE_ARRAY_TEXTURES)
    .union(naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32)
    .union(naga::valid::Capabilities::EARLY_DEPTH_TEST)
    .union(naga::valid::Capabilities::TEXTURE_AND_SAMPLER_BINDING_ARRAY)
    .union(naga::valid::Capabilities::TEXTURE_AND_SAMPLER_BINDING_ARRAY_NON_UNIFORM_INDEXING)
    .union(naga::valid::Capabilities::SUBGROUP)
    .union(naga::valid::Capabilities::TEXTURE_ATOMIC);

/// Check `program`: naga's validation of its module, then the program rules. Every problem found is returned.
pub fn validate(program: &Program) -> Result<naga::valid::ModuleInfo, Vec<Problem>> {
    let mut problems = Vec::new();
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), CAPABILITIES)
        .validate(&program.module)
    {
        Ok(info) => Some(info),
        Err(e) => {
            {
                // naga's error says where in a chain of sources; keep all of it
                let mut text = e.as_inner().to_string();
                let mut source = std::error::Error::source(e.as_inner());
                while let Some(inner) = source {
                    text.push_str(": ");
                    text.push_str(&inner.to_string());
                    source = inner.source();
                }
                problems.push(Problem::Module(text));
            }
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

/// Whether a module type is the interface type `ty`.
fn matches_io(module: &naga::Module, ty: &IoType, handle: naga::Handle<naga::Type>) -> bool {
    let types = &module.types;
    match ty {
        IoType::Value(v) => is_value(&types[handle].inner, *v),
        IoType::ClipDistances(n) => matches!(
            &types[handle].inner,
            TypeInner::Array { base, size: naga::ArraySize::Constant(len), .. }
                if len.get() == *n && types[*base].inner == TypeInner::Scalar(naga::Scalar::F32)
        ),
        IoType::Array(v, n) => matches!(
            &types[handle].inner,
            TypeInner::Array { base, size: naga::ArraySize::Constant(len), .. }
                if len.get() == *n && is_value(&types[*base].inner, *v)
        ),
    }
}

/// A program of a stage that has no entry point: its functions are the ones [`stage_functions`] names, with the
/// arguments and result members it says, no bindings, and the module has no entry point.
fn functions(p: &Program, sigs: &[FunctionSig], problems: &mut Vec<Problem>) {
    let mut fail = |m: String| problems.push(Problem::Program(m));
    if !p.module.entry_points.is_empty() {
        fail(format!(
            "a {:?} program has {} entry points; it is made of functions",
            p.stage,
            p.module.entry_points.len()
        ));
    }
    if p.module.functions.len() != sigs.len() {
        fail(format!(
            "a {:?} program is {} functions, the module has {}",
            p.stage,
            sigs.len(),
            p.module.functions.len()
        ));
    }
    if sigs.iter().all(|s| s.name != p.entry) {
        fail(format!(
            "the program's entry {} is not one of its functions",
            p.entry
        ));
    }
    for sig in sigs {
        let Some((_, f)) = p
            .module
            .functions
            .iter()
            .find(|(_, f)| f.name.as_deref() == Some(sig.name))
        else {
            fail(format!("the function {} is missing", sig.name));
            continue;
        };
        if f.arguments.len() != sig.arguments.len() {
            fail(format!(
                "{}: {} arguments for {} expected",
                sig.name,
                f.arguments.len(),
                sig.arguments.len()
            ));
        }
        for (item, arg) in sig.arguments.iter().zip(&f.arguments) {
            if arg.name.as_deref() != Some(item.name.as_str()) {
                fail(format!(
                    "{}: argument {} is named {:?}",
                    sig.name, item.name, arg.name
                ));
            }
            if !matches_io(&p.module, &item.ty, arg.ty) {
                fail(format!(
                    "{}: argument {}'s type is not {:?}",
                    sig.name, item.name, item.ty
                ));
            }
            if arg.binding.is_some() {
                fail(format!("{}: argument {} has a binding", sig.name, item.name));
            }
        }
        match (&f.result, sig.result.is_empty()) {
            (None, true) => {}
            (None, false) => fail(format!(
                "{}: results declared but the function returns nothing",
                sig.name
            )),
            (Some(_), true) => fail(format!(
                "{}: the function returns a value but has no results",
                sig.name
            )),
            (Some(result), false) => match &p.module.types[result.ty].inner {
                TypeInner::Struct { members, .. } if members.len() == sig.result.len() => {
                    for (item, m) in sig.result.iter().zip(members) {
                        if m.name.as_deref() != Some(item.name.as_str()) {
                            fail(format!(
                                "{}: result {} is named {:?}",
                                sig.name, item.name, m.name
                            ));
                        }
                        if !matches_io(&p.module, &item.ty, m.ty) {
                            fail(format!(
                                "{}: result {}'s type is not {:?}",
                                sig.name, item.name, item.ty
                            ));
                        }
                        if m.binding.is_some() {
                            fail(format!("{}: result {} has a binding", sig.name, item.name));
                        }
                    }
                }
                _ => fail(format!(
                    "{}: the result is not a structure of {} members",
                    sig.name,
                    sig.result.len()
                )),
            },
        }
        if p.interface.stage != p.stage {
            fail(format!(
                "stages disagree: the program {:?}, its interface {:?}",
                p.stage, p.interface.stage
            ));
        }
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

fn interface(p: &Program, problems: &mut Vec<Problem>) {
    match stage_functions(&p.interface) {
        Err(e) => return problems.push(Problem::Program(e)),
        Ok(Some(sigs)) => return functions(p, &sigs, problems),
        Ok(None) => {}
    }
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
        ShaderStage::Compute => Some(crate::Stage::Compute),
        _ => None,
    };
    if stage != Some(p.stage) || p.interface.stage != p.stage {
        fail(format!(
            "stages disagree: the entry point is {:?}, the program {:?}, its interface {:?}",
            entry.stage, p.stage, p.interface.stage
        ));
        return;
    }
    // a thread group is D3D's size: at most 1024 invocations, 1024 along x and y, 64 along z
    if entry.stage == ShaderStage::Compute {
        let [x, y, z] = entry.workgroup_size;
        if x == 0
            || y == 0
            || z == 0
            || x > 1024
            || y > 1024
            || z > 64
            || u64::from(x) * u64::from(y) * u64::from(z) > 1024
        {
            fail(format!("a thread group of {x} by {y} by {z}"));
        }
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
    let io = match entry_io(&p.interface) {
        Ok(io) => io,
        Err(e) => return fail(e),
    };
    let function = &entry.function;
    let types = &p.module.types;
    let matches_type = |ty: &IoType, handle| matches_io(&p.module, ty, handle);

    // inputs: one argument each
    if function.arguments.len() != io.arguments.len() {
        fail(format!(
            "{} entry arguments for {} input varyings",
            function.arguments.len(),
            io.arguments.len()
        ));
    }
    for (item, arg) in io.arguments.iter().zip(&function.arguments) {
        let name = &item.name;
        if arg.name.as_deref() != Some(name.as_str()) {
            fail(format!("input {name}: the argument is named {:?}", arg.name));
        }
        if !matches_type(&item.ty, arg.ty) {
            fail(format!("input {name}: the argument's type is not {:?}", item.ty));
        }
        if arg.binding.as_ref() != Some(&item.binding) {
            fail(format!(
                "input {name}: binding {:?}, expected {:?}",
                arg.binding, item.binding
            ));
        }
    }

    // outputs: the members of the result structure
    match (&function.result, io.results.is_empty()) {
        (None, true) => {}
        (None, false) => fail("outputs declared but the entry point returns nothing".into()),
        (Some(_), true) => fail("the entry point returns a value but the interface has no outputs".into()),
        (Some(result), false) => match &types[result.ty].inner {
            TypeInner::Struct { members, .. } => {
                if members.len() != io.results.len() {
                    fail(format!(
                        "{} result members for {} outputs",
                        members.len(),
                        io.results.len()
                    ));
                }
                for (item, m) in io.results.iter().zip(members) {
                    let name = &item.name;
                    if m.name.as_deref() != Some(name.as_str()) {
                        fail(format!("output {name}: the member is named {:?}", m.name));
                    }
                    if !matches_type(&item.ty, m.ty) {
                        fail(format!("output {name}: the member's type is not {:?}", item.ty));
                    }
                    if m.binding.as_ref() != Some(&item.binding) {
                        fail(format!(
                            "output {name}: binding {:?}, expected {:?}",
                            m.binding, item.binding
                        ));
                    }
                }
            }
            _ => fail("the entry point's result is not a structure".into()),
        },
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
        let array = r.is_array();
        if r.count == 0 {
            fail(format!("resource {id}: a range of no bindings"));
        }
        if array
            && !matches!(
                r.kind,
                ResourceKind::Texture { .. } | ResourceKind::Sampler { .. }
            )
        {
            fail(format!(
                "resource {id}: only textures and samplers are carried as arrays (count {})",
                r.count
            ));
        }
        // an array is a `binding_array` of the resource's own type, of the count (unbounded: dynamically sized)
        let mut inner = &p.module.types[global.ty].inner;
        if array {
            match inner {
                TypeInner::BindingArray { base, size } => {
                    let sized = match size {
                        naga::ArraySize::Constant(n) => !r.is_unbounded() && n.get() == r.count,
                        naga::ArraySize::Pending(_) => false,
                        naga::ArraySize::Dynamic => r.is_unbounded(),
                    };
                    if !sized {
                        fail(format!(
                            "resource {id}: the global's array size {size:?} is not the interface's count {}",
                            r.count
                        ));
                    }
                    inner = &p.module.types[*base].inner;
                }
                _ => fail(format!(
                    "resource {id}: an array of {} bindings is not a binding array",
                    r.count
                )),
            }
        }
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
                        // naga has no write-only storage buffers: a program that only writes has a read-write global
                        matches!(space, AddressSpace::Storage { access } if access == want
                            || (want == naga::StorageAccess::STORE
                                && access == naga::StorageAccess::LOAD | naga::StorageAccess::STORE))
                            && buffer_fits(&p.module, inner, *kind)
                    }
                    StorageTarget::Image { dim, arrayed, .. } => {
                        space == AddressSpace::Handle
                            && matches!(inner, TypeInner::Image { dim: d, arrayed: a, class: ImageClass::Storage { access, .. } }
                                if *d == texture_dim(*dim) && *a == *arrayed
                                    && access.difference(naga::StorageAccess::ATOMIC) == want)
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
    let words = |base: &TypeInner| {
        matches!(
            base,
            TypeInner::Scalar(naga::Scalar::U32) | TypeInner::Atomic(naga::Scalar::U32 | naga::Scalar::I32)
        )
    };
    match kind {
        // a buffer the program reaches with atomic operations is an array of atomic words
        BufferKind::Raw => words(base) && *stride == 4,
        BufferKind::Structured { stride: s } => words(base) && *stride == 4 && s % 4 == 0,
        BufferKind::Typed(v) => {
            v.width == 4
                && *stride == 16
                && (*base
                    == TypeInner::Vector {
                        size: naga::VectorSize::Quad,
                        scalar: naga::Scalar::U32,
                    }
                    // a buffer the program reaches with atomic operations: four atomic words an element
                    || matches!(base, TypeInner::Struct { members, span: 16 }
                        if members.len() == 4
                            && members.iter().enumerate().all(|(k, m)| {
                                m.offset == 4 * k as u32
                                    && matches!(
                                        module.types[m.ty].inner,
                                        TypeInner::Atomic(naga::Scalar::U32 | naga::Scalar::I32)
                                    )
                            })))
        }
    }
}
