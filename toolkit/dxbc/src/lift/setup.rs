//! What a lift needs before it writes any code: the interface (signatures, interpolation from the declarations,
//! resources from the declarations named and laid out by the container's reflection where it has any), how the entry
//! point carries it, and the facts the declarations state. Pure data; the module is built from it.

use std::collections::{BTreeMap, BTreeSet};

use naga::ImageDimension;
use primase_shader::build::Kind;
use primase_shader::{
    Access, BufferKind, BufferLayout, D3dBinding, D3dClass, EntryIo, Interface, Label, Resource, ResourceId,
    ResourceKind, ResourceProvenance, SampleType, ScalarType, Stage, StorageTarget, SystemValue, TextureDim,
    Value, Varying, VaryingProvenance, entry_io,
};

use crate::decode::{Declared, Decoded, Register, Selection};
use crate::{Result, error, reflection, signature_elements};

/// One declared resource, with what the module's global for it needs.
#[derive(Clone, Debug)]
pub(crate) enum Declaration {
    ConstantBuffer {
        slot: u32,
        rows: u32,
    },
    Texture(Texture),
    /// A typed buffer view: its loads return four lanes.
    TypedBuffer {
        slot: u32,
    },
    RawBuffer {
        slot: u32,
    },
    StructuredBuffer {
        slot: u32,
        stride: u32,
    },
    Sampler {
        slot: u32,
        comparison: bool,
    },
    /// A typed unordered-access texture view.
    View(View),
    /// An unordered-access view of a buffer: raw, structured or typed.
    UavBuffer(UavBuffer),
}

/// An unordered-access buffer view `u<slot>`, with the access the program makes of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UavBuffer {
    pub slot: u32,
    pub kind: UavKind,
    pub load: bool,
    pub store: bool,
    /// The integer type its atomic operations have, when it has any.
    pub atomic: Option<naga::Scalar>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UavKind {
    Raw,
    Structured {
        stride: u32,
    },
    /// A typed buffer: four lanes of this kind per element.
    Typed(Kind),
}

/// Thread-group shared memory `g<register>`: its size in words, the element stride of a structured one in bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shared {
    pub register: u32,
    pub words: u32,
    pub stride: Option<u32>,
    pub atomic: Option<naga::Scalar>,
}

/// A typed unordered-access view `u<slot>` of a texture, with the access the program makes of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    pub slot: u32,
    pub dim: ImageDimension,
    pub arrayed: bool,
    /// What its loads return and its stores take in each lane.
    pub kind: Kind,
    pub load: bool,
    pub store: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Texture {
    pub slot: u32,
    pub dim: ImageDimension,
    pub arrayed: bool,
    pub multi: bool,
    /// What a load returns in each lane.
    pub kind: Kind,
    /// Compared against a reference (`sample_c`, `gather4_c`): a depth image in the module.
    pub depth: bool,
}

pub(crate) struct Setup {
    pub interface: Interface,
    pub io: EntryIo,
    pub early_depth_test: bool,
    /// Parallel to `interface.resources`.
    pub declarations: Vec<Declaration>,
    pub immediates: Vec<[u32; 4]>,
    pub shared: Vec<Shared>,
    /// A compute program's thread group size.
    pub workgroup: Option<[u32; 3]>,
    /// The compute ids the program declares it reads.
    pub ids: BTreeSet<SystemValue>,
    pub indexable: Vec<(u32, u32)>,
    pub temps: u32,
    /// The lanes of each input register the program declares it reads.
    pub declared_inputs: BTreeMap<u32, u8>,
    pub notes: BTreeSet<String>,
}

impl Setup {
    pub(crate) fn empty() -> Self {
        Self {
            interface: Interface {
                stage: Stage::Pixel,
                inputs: Vec::new(),
                outputs: Vec::new(),
                resources: Vec::new(),
                facts: Default::default(),
            },
            io: EntryIo {
                arguments: Vec::new(),
                results: Vec::new(),
            },
            early_depth_test: false,
            declarations: Vec::new(),
            immediates: Vec::new(),
            shared: Vec::new(),
            workgroup: None,
            ids: BTreeSet::new(),
            indexable: Vec::new(),
            temps: 0,
            declared_inputs: BTreeMap::new(),
            notes: BTreeSet::new(),
        }
    }

    pub(crate) fn new(bytes: &[u8], decoded: &Decoded, stage: Stage) -> Result<Self> {
        let (signature_stage, input_elements, output_elements) = signature_elements(bytes)?;
        debug_assert_eq!(signature_stage, stage);
        let mut setup = Self::empty();
        let refuse = |detail: String| error(0, detail);
        let mut declared_inputs: BTreeMap<u32, u8> = BTreeMap::new();
        let (mut primitive_id, mut input_coverage) = (false, false);
        let (mut depth, mut coverage, mut stencil) = (None, false, false);
        let mut interpolations: Vec<(u32, u8, primase_shader::Interpolation)> = Vec::new();
        let mut declared_outputs: BTreeMap<u32, u8> = BTreeMap::new();
        for d in &decoded.declarations {
            let at = |detail: String| error(d.word, detail);
            match &d.kind {
                Declared::Temps(n) => setup.temps = *n,
                Declared::IndexableTemp {
                    register,
                    size,
                    components,
                } => {
                    if *components > 4 || *size == 0 {
                        return Err(at(format!(
                            "dcl_indexable_temp x{register}[{size}] of {components} components"
                        )));
                    }
                    setup.indexable.push((*register, *size));
                }
                Declared::GlobalFlags(flags) => {
                    if flags & 2 != 0 {
                        return Err(at(
                            "double precision is enabled (global flag): doubles are not lifted".into(),
                        ));
                    }
                    if flags & 4 != 0 {
                        setup.early_depth_test = true;
                    }
                    if flags & 0x20 != 0 {
                        setup
                            .notes
                            .insert("min-precision operands are computed at 32 bits".into());
                    }
                }
                Declared::ImmediateBuffer(rows) => setup.immediates = rows.clone(),
                Declared::CustomData { .. } => {}
                Declared::Input {
                    operand,
                    interpolation,
                    name,
                } => match operand.register {
                    Register::Input => {
                        let Selection::Mask(mask) = operand.selection else {
                            return Err(at("an input declaration without a lane mask".into()));
                        };
                        let register = operand.indices.first().map_or(0, |i| i.base);
                        *declared_inputs.entry(register).or_default() |= mask;
                        if let Some(mode) = interpolation {
                            let interp = primase_shader::d3d::interpolation(*mode).map_err(at)?;
                            interpolations.push((register, mask, interp));
                        }
                        let _ = name;
                    }
                    Register::PrimitiveId => primitive_id = true,
                    Register::InputCoverage => input_coverage = true,
                    Register::ThreadId => {
                        setup.ids.insert(SystemValue::DispatchThreadId);
                    }
                    Register::ThreadGroupId => {
                        setup.ids.insert(SystemValue::GroupId);
                    }
                    Register::ThreadIdInGroup => {
                        setup.ids.insert(SystemValue::GroupThreadId);
                    }
                    Register::ThreadIdInGroupFlattened => {
                        setup.ids.insert(SystemValue::GroupIndex);
                    }
                    other => return Err(at(format!("a declared input of register type {other:?}"))),
                },
                Declared::Output { operand, .. } => match operand.register {
                    Register::Output => {
                        let Selection::Mask(mask) = operand.selection else {
                            return Err(at("an output declaration without a lane mask".into()));
                        };
                        let register = operand.indices.first().map_or(0, |i| i.base);
                        *declared_outputs.entry(register).or_default() |= mask;
                    }
                    Register::OutputDepth => depth = Some(SystemValue::Depth),
                    Register::OutputDepthGreaterEqual => depth = Some(SystemValue::DepthGreaterEqual),
                    Register::OutputDepthLessEqual => depth = Some(SystemValue::DepthLessEqual),
                    Register::OutputCoverage => coverage = true,
                    Register::OutputStencilRef => stencil = true,
                    other => return Err(at(format!("a declared output of register type {other:?}"))),
                },
                Declared::ConstantBuffer {
                    slot,
                    rows,
                    dimensions,
                    ..
                } => {
                    if *dimensions > 2 {
                        return Err(at("SM5.1 constant buffer declaration".into()));
                    }
                    setup.declarations.push(Declaration::ConstantBuffer {
                        slot: *slot,
                        rows: *rows,
                    });
                }
                Declared::Sampler { slot, mode } => {
                    if *mode == 2 {
                        return Err(at("mono sampler".into()));
                    }
                    setup.declarations.push(Declaration::Sampler {
                        slot: *slot,
                        comparison: *mode == 1,
                    });
                }
                Declared::Resource {
                    slot,
                    dimension,
                    samples,
                    returns,
                    ..
                } => {
                    let kind = match returns[0] {
                        1 | 2 | 5 => Kind::F,
                        3 => Kind::I,
                        4 | 6 => Kind::U,
                        other => return Err(at(format!("resource return type {other}"))),
                    };
                    let (dim, arrayed) = match dimension {
                        1 => {
                            setup.declarations.push(Declaration::TypedBuffer { slot: *slot });
                            continue;
                        }
                        2 => (ImageDimension::D1, false),
                        3 | 4 => (ImageDimension::D2, false),
                        5 => (ImageDimension::D3, false),
                        6 => (ImageDimension::Cube, false),
                        8 | 9 => (ImageDimension::D2, true),
                        10 => (ImageDimension::Cube, true),
                        other => return Err(at(format!("resource dimension {other}"))),
                    };
                    setup.declarations.push(Declaration::Texture(Texture {
                        slot: *slot,
                        dim,
                        arrayed,
                        multi: matches!(dimension, 4 | 9) || *samples != 0,
                        kind,
                        depth: false,
                    }));
                }
                Declared::ResourceRaw { slot } => {
                    setup.declarations.push(Declaration::RawBuffer { slot: *slot })
                }
                Declared::ResourceStructured { slot, stride } => {
                    setup.declarations.push(Declaration::StructuredBuffer {
                        slot: *slot,
                        stride: *stride,
                    })
                }
                Declared::UavTyped {
                    slot,
                    dimension,
                    returns,
                    ..
                } => {
                    let kind = match returns[0] {
                        1 | 2 | 5 => Kind::F,
                        3 => Kind::I,
                        4 | 6 => Kind::U,
                        other => return Err(at(format!("unordered-access view return type {other}"))),
                    };
                    let (dim, arrayed) = match dimension {
                        2 => (ImageDimension::D1, false),
                        3 => (ImageDimension::D2, false),
                        5 => (ImageDimension::D3, false),
                        7 => (ImageDimension::D1, true),
                        8 => (ImageDimension::D2, true),
                        1 => {
                            setup.declarations.push(Declaration::UavBuffer(UavBuffer {
                                slot: *slot,
                                kind: UavKind::Typed(kind),
                                load: false,
                                store: false,
                                atomic: None,
                            }));
                            continue;
                        }
                        _ => return Err(at(format!("unordered-access view dimension {dimension}"))),
                    };
                    setup.declarations.push(Declaration::View(View {
                        slot: *slot,
                        dim,
                        arrayed,
                        kind,
                        load: false,
                        store: false,
                    }));
                }
                Declared::UavRaw { slot, .. } => setup.declarations.push(Declaration::UavBuffer(UavBuffer {
                    slot: *slot,
                    kind: UavKind::Raw,
                    load: false,
                    store: false,
                    atomic: None,
                })),
                Declared::UavStructured { slot, stride, .. } => {
                    setup.declarations.push(Declaration::UavBuffer(UavBuffer {
                        slot: *slot,
                        kind: UavKind::Structured {
                            stride: (*stride).max(4),
                        },
                        load: false,
                        store: false,
                        atomic: None,
                    }))
                }
                Declared::ThreadGroup(size) => setup.workgroup = Some(*size),
                Declared::GroupSharedRaw { register, bytes } => setup.shared.push(Shared {
                    register: *register,
                    words: bytes.div_ceil(4),
                    stride: None,
                    atomic: None,
                }),
                Declared::GroupSharedStructured {
                    register,
                    stride,
                    count,
                } => {
                    let words = stride
                        .div_ceil(4)
                        .checked_mul(*count)
                        .ok_or_else(|| at("thread-group memory too large".into()))?;
                    setup.shared.push(Shared {
                        register: *register,
                        words,
                        stride: Some(*stride),
                        atomic: None,
                    });
                }
                Declared::Other { opcode, .. } => {
                    return Err(at(format!(
                        "{} ({opcode}) is not lifted",
                        crate::decode::opcode_name(*opcode)
                    )));
                }
            }
        }
        setup.declared_inputs = declared_inputs;
        // the access the program makes of each typed view
        for ins in &decoded.instructions {
            let (operand, store) = match ins.opcode {
                163 => (2, false),
                164 => (0, true),
                _ => continue,
            };
            let slot = ins
                .operands
                .get(operand)
                .and_then(|o| o.indices.first())
                .map_or(0, |i| i.base);
            for d in &mut setup.declarations {
                if let Declaration::View(v) = d
                    && v.slot == slot
                {
                    v.store |= store;
                    v.load |= !store;
                }
            }
        }
        // the access the program makes of each unordered-access buffer and of thread-group memory, and the integer
        // type of the atomic operations that reach them
        for ins in &decoded.instructions {
            let name = ins.name();
            let (resource, load, store, atomic) = match name {
                "ld_raw" => (3, true, false, None),
                "ld_structured" => (4, true, false, None),
                "ld_uav_typed" => (3, true, false, None),
                "store_raw" | "store_structured" | "store_uav_typed" => (1, false, true, None),
                n if n.starts_with("atomic_") => (
                    1,
                    true,
                    true,
                    Some(if matches!(n, "atomic_imax" | "atomic_imin") {
                        naga::Scalar::I32
                    } else {
                        naga::Scalar::U32
                    }),
                ),
                n if n.starts_with("imm_atomic_")
                    && !matches!(n, "imm_atomic_alloc" | "imm_atomic_consume") =>
                {
                    (
                        2,
                        true,
                        true,
                        Some(if matches!(n, "imm_atomic_imax" | "imm_atomic_imin") {
                            naga::Scalar::I32
                        } else {
                            naga::Scalar::U32
                        }),
                    )
                }
                _ => continue,
            };
            // the resource operand is the one after the destination (and the addresses of a load, which come first)
            let operand = match name {
                "ld_raw" => 2,
                "ld_structured" => 3,
                "ld_uav_typed" => 2,
                n if n.starts_with("imm_atomic_") => 1,
                _ => 0,
            };
            let _ = resource;
            let Some(op) = ins.operands.get(operand) else {
                continue;
            };
            let index = op.indices.first().map_or(0, |i| i.base);
            match op.register {
                Register::Uav => {
                    for d in &mut setup.declarations {
                        if let Declaration::UavBuffer(b) = d
                            && b.slot == index
                        {
                            b.load |= load;
                            b.store |= store;
                            if let Some(a) = atomic {
                                // a signed minimum or maximum makes the buffer's atomics signed
                                b.atomic = Some(match (b.atomic, a) {
                                    (Some(naga::Scalar::I32), _) | (_, naga::Scalar::I32) => {
                                        naga::Scalar::I32
                                    }
                                    _ => naga::Scalar::U32,
                                });
                            }
                        }
                    }
                }
                Register::GroupSharedMemory => {
                    for g in &mut setup.shared {
                        if g.register == index
                            && let Some(a) = atomic
                        {
                            g.atomic = Some(match (g.atomic, a) {
                                (Some(naga::Scalar::I32), _) | (_, naga::Scalar::I32) => naga::Scalar::I32,
                                _ => naga::Scalar::U32,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        // a texture compared against a reference is a depth image
        for ins in &decoded.instructions {
            if matches!(ins.opcode, 70 | 71 | 126) {
                let slot = ins
                    .operands
                    .get(2)
                    .and_then(|o| o.indices.first())
                    .map_or(0, |i| i.base);
                for d in &mut setup.declarations {
                    if let Declaration::Texture(t) = d
                        && t.slot == slot
                    {
                        t.depth = true;
                    }
                }
            }
        }

        // the signature elements the program uses: a register the program declares, or a special input or output it
        // declares by name; an element it never declares is not part of what it exposes (and may be one no stage
        // binding exists for: Unity's pixel programs list SV_InstanceID)
        let mut interface = Interface {
            stage,
            inputs: Vec::new(),
            outputs: Vec::new(),
            resources: Vec::new(),
            facts: Default::default(),
        };
        for e in &input_elements {
            let declared = setup
                .declared_inputs
                .get(&e.register)
                .is_some_and(|m| m & e.mask != 0);
            // D3D_NAME_PRIMITIVE_ID and _COVERAGE are read as special operands, not through a register
            if declared || (e.system_value == 7 && primitive_id) || (e.system_value == 66 && input_coverage) {
                interface.inputs.push(e.varying(stage, false)?);
            }
        }
        for e in &output_elements {
            let declared = declared_outputs.get(&e.register).is_some_and(|m| m & e.mask != 0);
            if !declared && !e.name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("SV_")) {
                continue;
            }
            let v = e.varying(stage, true)?;
            let keep = match v.system {
                Some(SystemValue::Depth | SystemValue::DepthGreaterEqual | SystemValue::DepthLessEqual) => {
                    depth.is_some()
                }
                Some(SystemValue::Coverage) => coverage,
                Some(SystemValue::StencilRef) => stencil,
                _ => declared,
            };
            if keep {
                interface.outputs.push(v);
            }
        }
        // the interface: signature elements, the interpolation the declarations state, the system values the
        // signatures leave out
        for input in &mut interface.inputs {
            let Some(d3d) = input.provenance.d3d else { continue };
            let mut declared = interpolations
                .iter()
                .filter(|(r, m, _)| *r == d3d.register && m & d3d.mask != 0)
                .map(|(_, _, i)| *i);
            input.interp = declared.next();
            if declared.any(|other| Some(other) != input.interp) {
                return Err(refuse(format!(
                    "input {}{} is declared with different interpolations",
                    input.label.name, input.label.index
                )));
            }
        }
        let special = |name: &str, system: SystemValue, scalar: ScalarType| Varying {
            label: Label {
                name: name.into(),
                index: 0,
            },
            system: Some(system),
            value: Value { scalar, width: 1 },
            interp: None,
            used: true,
            provenance: VaryingProvenance::default(),
        };
        if primitive_id
            && !interface
                .inputs
                .iter()
                .any(|v| v.system == Some(SystemValue::PrimitiveIndex))
        {
            interface.inputs.push(special(
                "SV_PrimitiveID",
                SystemValue::PrimitiveIndex,
                ScalarType::Uint,
            ));
        }
        if input_coverage
            && !interface
                .inputs
                .iter()
                .any(|v| v.system == Some(SystemValue::Coverage))
        {
            interface
                .inputs
                .push(special("SV_Coverage", SystemValue::Coverage, ScalarType::Uint));
        }
        if let Some(system) = depth {
            if !interface.outputs.iter().any(|v| v.system == Some(system)) {
                let name = match system {
                    SystemValue::DepthGreaterEqual => "SV_DepthGreaterEqual",
                    SystemValue::DepthLessEqual => "SV_DepthLessEqual",
                    _ => "SV_Depth",
                };
                interface.outputs.push(special(name, system, ScalarType::Float));
            }
            if system != SystemValue::Depth {
                setup
                    .notes
                    .insert("a conservative depth output is written as a plain depth output".into());
            }
        }
        if coverage
            && !interface
                .outputs
                .iter()
                .any(|v| v.system == Some(SystemValue::Coverage))
        {
            interface
                .outputs
                .push(special("SV_Coverage", SystemValue::Coverage, ScalarType::Uint));
        }
        if stencil
            && !interface
                .outputs
                .iter()
                .any(|v| v.system == Some(SystemValue::StencilRef))
        {
            interface.outputs.push(special(
                "SV_StencilRef",
                SystemValue::StencilRef,
                ScalarType::Uint,
            ));
        }
        // a compute program's ids are system inputs the signature does not list
        for id in setup.ids.clone() {
            let (name, width) = match id {
                SystemValue::DispatchThreadId => ("SV_DispatchThreadID", 3),
                SystemValue::GroupId => ("SV_GroupID", 3),
                SystemValue::GroupThreadId => ("SV_GroupThreadID", 3),
                _ => ("SV_GroupIndex", 1),
            };
            let mut v = special(name, id, ScalarType::Uint);
            v.value.width = width;
            interface.inputs.push(v);
        }
        // outputs: a stage's depth hint is a plain depth output in the module
        setup.interface = interface;

        // resources, in declaration order; reflection names them and lays out the constant buffers
        let rdef = reflection(bytes).ok();
        for (i, d) in setup.declarations.iter_mut().enumerate() {
            let (class, register, kind, default_name) = match d {
                Declaration::ConstantBuffer { slot, rows } => (
                    D3dClass::ConstantBuffer,
                    *slot,
                    ResourceKind::ConstantBuffer {
                        layout: BufferLayout {
                            size: rows
                                .checked_mul(16)
                                .ok_or_else(|| refuse("constant buffer size".into()))?,
                            constants: Vec::new(),
                        },
                    },
                    format!("cb{slot}"),
                ),
                Declaration::Texture(t) => (
                    D3dClass::ShaderResource,
                    t.slot,
                    ResourceKind::Texture {
                        dim: match t.dim {
                            ImageDimension::D1 => TextureDim::D1,
                            ImageDimension::D2 => TextureDim::D2,
                            ImageDimension::D3 => TextureDim::D3,
                            ImageDimension::Cube => TextureDim::Cube,
                        },
                        arrayed: t.arrayed,
                        multisampled: t.multi,
                        sample: if t.depth {
                            SampleType::Depth
                        } else {
                            match t.kind {
                                Kind::I => SampleType::Sint,
                                Kind::U => SampleType::Uint,
                                _ => SampleType::Float,
                            }
                        },
                    },
                    format!("t{}", t.slot),
                ),
                Declaration::TypedBuffer { slot } => {
                    let scalar = typed_scalar(decoded, *slot);
                    (
                        D3dClass::ShaderResource,
                        *slot,
                        ResourceKind::Buffer(BufferKind::Typed(Value { scalar, width: 4 })),
                        format!("t{slot}"),
                    )
                }
                Declaration::RawBuffer { slot } => (
                    D3dClass::ShaderResource,
                    *slot,
                    ResourceKind::Buffer(BufferKind::Raw),
                    format!("t{slot}"),
                ),
                Declaration::StructuredBuffer { slot, stride } => (
                    D3dClass::ShaderResource,
                    *slot,
                    ResourceKind::Buffer(BufferKind::Structured { stride: *stride }),
                    format!("t{slot}"),
                ),
                Declaration::View(v) => (
                    D3dClass::UnorderedAccess,
                    v.slot,
                    ResourceKind::Storage {
                        target: StorageTarget::Image {
                            dim: match v.dim {
                                ImageDimension::D1 => TextureDim::D1,
                                ImageDimension::D2 => TextureDim::D2,
                                ImageDimension::D3 => TextureDim::D3,
                                ImageDimension::Cube => TextureDim::Cube,
                            },
                            arrayed: v.arrayed,
                            sample: match v.kind {
                                Kind::I => SampleType::Sint,
                                Kind::U => SampleType::Uint,
                                _ => SampleType::Float,
                            },
                        },
                        access: match (v.load, v.store) {
                            (true, true) => Access::ReadWrite,
                            (false, true) => Access::Write,
                            _ => Access::Read,
                        },
                    },
                    format!("u{}", v.slot),
                ),
                Declaration::UavBuffer(b) => (
                    D3dClass::UnorderedAccess,
                    b.slot,
                    ResourceKind::Storage {
                        target: StorageTarget::Buffer(match b.kind {
                            UavKind::Raw => BufferKind::Raw,
                            UavKind::Structured { stride } => BufferKind::Structured { stride },
                            UavKind::Typed(kind) => BufferKind::Typed(Value {
                                scalar: match kind {
                                    Kind::I => ScalarType::Sint,
                                    Kind::U => ScalarType::Uint,
                                    _ => ScalarType::Float,
                                },
                                width: 4,
                            }),
                        }),
                        access: match (b.load, b.store) {
                            (true, true) => Access::ReadWrite,
                            (false, true) => Access::Write,
                            _ => Access::Read,
                        },
                    },
                    format!("u{}", b.slot),
                ),
                Declaration::Sampler { slot, comparison } => (
                    D3dClass::Sampler,
                    *slot,
                    ResourceKind::Sampler {
                        comparison: *comparison,
                    },
                    format!("s{slot}"),
                ),
            };
            let reflected = rdef.as_ref().and_then(|r| {
                r.resources.iter().find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|b| b.class == class && b.register == register)
                })
            });
            let mut kind = kind;
            let mut name = default_name;
            if let Some(r) = reflected {
                name = r.provenance.name.clone();
                if let (
                    ResourceKind::ConstantBuffer { layout },
                    ResourceKind::ConstantBuffer { layout: stated },
                ) = (&mut kind, &r.kind)
                {
                    let size = layout.size.max(stated.size);
                    *layout = stated.clone();
                    layout.size = size;
                    if let Declaration::ConstantBuffer { rows, .. } = d {
                        *rows = (*rows).max(size.div_ceil(16));
                    }
                }
            }
            setup.interface.resources.push(Resource {
                id: ResourceId(i as u32),
                kind,
                count: 1,
                provenance: ResourceProvenance {
                    name,
                    d3d: Some(D3dBinding {
                        class,
                        register,
                        space: 0,
                    }),
                },
            });
        }
        if stage == Stage::Vertex
            && !setup
                .interface
                .outputs
                .iter()
                .any(|v| v.system == Some(SystemValue::ClipPosition))
        {
            return Err(refuse(
                "a vertex program that writes no SV_Position (a stream-output or geometry-feeding stage)"
                    .into(),
            ));
        }
        setup.io = entry_io(&setup.interface).map_err(refuse)?;
        Ok(setup)
    }
}

/// A typed buffer view's lane type, from its declaration's first return nibble.
fn typed_scalar(decoded: &Decoded, slot: u32) -> ScalarType {
    for d in &decoded.declarations {
        if let Declared::Resource { slot: s, returns, .. } = &d.kind
            && *s == slot
        {
            return match returns[0] {
                3 => ScalarType::Sint,
                4 | 6 => ScalarType::Uint,
                _ => ScalarType::Float,
            };
        }
    }
    ScalarType::Float
}
