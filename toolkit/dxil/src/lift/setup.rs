//! What a lift needs before it writes any code: the interface (the signature elements and resources the program
//! uses), how the entry point carries it, and the program's own instruction index.

use std::collections::{BTreeMap, BTreeSet};

use primase_shader::{
    EntryIo, Interface, Resource, ResourceId, ResourceKind, Stage, StageFacts, SystemValue, entry_io,
};

use crate::module::{Constant, Function, Module, Op, Value, ValueId};
use crate::reflection::{self, Element, Range};
use crate::{Chunk, Error, Result, error};

/// The constant integer a value is, if it is one.
pub(super) fn const_int(m: &Module, f: &Function, v: ValueId) -> Option<i64> {
    let value = if v < f.first {
        m.values.get(v)?
    } else {
        f.locals.get(v - f.first)?
    };
    match value {
        Value::Constant(Constant::Integer(i)) => Some(*i),
        Value::Constant(Constant::Null) => Some(0),
        _ => None,
    }
}

/// The range a `CreateHandleFromBinding` names: its class, lower bound and register space (the binding constant
/// `{ lower, upper, space, class }`), and the offset into the range (the call's index is the register itself).
pub(super) fn binding_of(m: &Module, f: &Function, bind: ValueId) -> Option<(u8, u32, u32)> {
    let value = if bind < f.first {
        m.values.get(bind)?
    } else {
        f.locals.get(bind - f.first)?
    };
    // a zero binding (register 0 of space 0, a shader resource) is stored as the null constant
    let field = |k: usize| match value {
        Value::Constant(Constant::Null) => Some(0),
        Value::Constant(Constant::Aggregate(items)) => const_int(m, f, *items.get(k)?),
        _ => None,
    };
    Some((
        u8::try_from(field(3)?).ok()?,
        u32::try_from(field(0)?).ok()?,
        u32::try_from(field(2)?).ok()?,
    ))
}

/// The system value a ray tracing `dx.op` reads (`InstanceID` 141 to `RayTCurrent` 154 except the matrices, and
/// `PrimitiveIndex` 161 and `GeometryIndex` 213).
pub(super) fn ray_system_value(code: u64) -> Option<SystemValue> {
    Some(match code {
        141 => SystemValue::InstanceId,
        142 => SystemValue::InstanceIndex,
        143 => SystemValue::HitKind,
        144 => SystemValue::RayFlags,
        145 => SystemValue::DispatchRaysIndex,
        146 => SystemValue::DispatchRaysDimensions,
        147 => SystemValue::WorldRayOrigin,
        148 => SystemValue::WorldRayDirection,
        149 => SystemValue::ObjectRayOrigin,
        150 => SystemValue::ObjectRayDirection,
        153 => SystemValue::RayTMin,
        154 => SystemValue::RayTCurrent,
        161 => SystemValue::PrimitiveIndex,
        213 => SystemValue::GeometryIndex,
        _ => return None,
    })
}

/// The module global whose value `v` is: the load of a global (or of a cast of one) that a library's handle is made
/// from.
pub(super) fn variable_loaded(m: &Module, f: &Function, v: ValueId) -> Option<ValueId> {
    let load = f
        .instructions
        .iter()
        .find(|i| i.result.is_some_and(|(r, _)| r == v))?;
    let Op::Load { pointer } = &load.op else {
        return None;
    };
    let mut at = *pointer;
    loop {
        let value = if at < f.first {
            m.values.get(at)?
        } else {
            f.locals.get(at - f.first)?
        };
        match value {
            Value::Global { .. } if at < f.first => return Some(at),
            Value::Constant(Constant::Cast { value, .. }) => at = *value,
            _ => return None,
        }
    }
}

/// The `dx.op` opcode a call makes: the callee is a `dx.op.*` declaration and the first argument a constant.
pub(super) fn dx_opcode(m: &Module, f: &Function, function: ValueId, args: &[ValueId]) -> Option<u64> {
    if !m.name(function)?.starts_with("dx.op.") {
        return None;
    }
    u64::try_from(const_int(m, f, *args.first()?)?).ok()
}

/// The resource ranges of the program, in the interface's order, and which of them the program uses.
pub(crate) struct Setup {
    pub interface: Interface,
    pub io: EntryIo,
    pub input_elements: Vec<Element>,
    pub output_elements: Vec<Element>,
    /// A hull or domain program's patch-constant signature elements.
    pub patch_elements: Vec<Element>,
    /// The functions of a program of a stage with no entry point (see [`primase_shader::stage_functions`]).
    pub sigs: Option<Vec<primase_shader::FunctionSig>>,
    /// The kept resources' ranges, parallel to `interface.resources`.
    pub ranges: Vec<Range>,
    pub notes: BTreeSet<String>,
    /// A thread group laid out in more than one dimension has no lane-number built-in in WebGPU: the lane is made at
    /// the start of the program (see `Lift::lane_at_start`).
    pub lane_by_rank: bool,
    /// The resources of descriptor-heap handles (`ResourceDescriptorHeap[i]`): the interface resource of each
    /// `CreateHandleFromHeap` call's result.
    pub heap: BTreeMap<ValueId, usize>,
}

impl Setup {
    /// The setup of a program whose instructions are those of `functions` (the entry function, and a hull program's
    /// patch-constant function).
    pub(super) fn new(chunks: &[Chunk<'_>], m: &Module, functions: &[&Function]) -> Result<Self> {
        let named = reflection::module(chunks, b"STAT")?;
        let mut interface = reflection::reflection(chunks, m, named.as_ref())?;
        if !matches!(
            interface.stage,
            Stage::Pixel
                | Stage::Vertex
                | Stage::Compute
                | Stage::Hull
                | Stage::Domain
                | Stage::Geometry
                | Stage::Mesh
        ) && !interface.stage.is_ray_tracing()
        {
            return Err(error(0, format!("{:?} programs are not lifted", interface.stage)));
        }
        let (input_elements, output_elements) = reflection::elements(m)?;
        let patch_elements = reflection::patch_elements(m)?;
        let program_ranges: Vec<Range> = reflection::ranges(m)?.into_iter().map(|r| r.0).collect();
        if program_ranges.len() != interface.resources.len() {
            return Err(error(
                0,
                "the reflection module's resources differ from the program's",
            ));
        }
        // which cells of the signatures and which ranges the instructions use
        let mut cells_in: BTreeSet<(u32, u8)> = BTreeSet::new();
        let mut cells_out: BTreeSet<(u32, u8)> = BTreeSet::new();
        let mut cells_patch: BTreeSet<(u32, u8)> = BTreeSet::new();
        let mut system_out: BTreeSet<u8> = BTreeSet::new();
        let mut used_ranges: BTreeSet<(u8, u32)> = BTreeSet::new();
        let mut used_bindings: BTreeSet<(u8, u32, u32)> = BTreeSet::new();
        // a library makes its handles from the value of the resource's global
        let mut used_variables: BTreeSet<ValueId> = BTreeSet::new();
        // the system inputs the program reads that the signature does not list: the sample index and, in a compute
        // program, the invocation's ids
        let mut system_inputs: BTreeSet<SystemValue> = BTreeSet::new();
        // the rows of an instance's transform the program reads
        let mut matrix_rows: BTreeSet<(SystemValue, u32)> = BTreeSet::new();
        for entry in functions.iter().copied() {
            for inst in &entry.instructions {
                let Op::Call { function, args } = &inst.op else {
                    continue;
                };
                let Some(code) = dx_opcode(m, entry, *function, args) else {
                    continue;
                };
                let konst = |n: usize| args.get(n).and_then(|&a| const_int(m, entry, a));
                match code {
                    // LoadInput, StoreOutput, LoadOutputControlPoint, LoadPatchConstant, StorePatchConstant, StoreVertexOutput
                    4 | 5 | 103 | 104 | 106 | 171 => {
                        let list = match code {
                            4 => &input_elements,
                            5 | 103 | 171 => &output_elements,
                            _ => &patch_elements,
                        };
                        let Some(id) = konst(1) else {
                            return Err(error(inst.at, "a signature element named by a computed id"));
                        };
                        let e = list
                            .iter()
                            .find(|e| i64::from(e.id) == id)
                            .ok_or_else(|| error(inst.at, format!("signature element {id}")))?;
                        if !e.placed {
                            if code == 5 || code == 171 {
                                system_out.insert(e.system_value);
                                continue;
                            }
                            return Err(error(
                                inst.at,
                                format!("input element {} is not placed in a register", e.name),
                            ));
                        }
                        let (rows, columns) = match (konst(2), konst(3)) {
                            (Some(r), Some(c)) => (r..r + 1, c..c + 1),
                            (Some(r), None) => (r..r + 1, 0..i64::from(e.columns)),
                            (None, Some(c)) => (0..i64::from(e.rows), c..c + 1),
                            (None, None) => (0..i64::from(e.rows), 0..i64::from(e.columns)),
                        };
                        for r in rows {
                            for c in columns.clone() {
                                let cell = (e.row + r as u32, e.column + c as u8);
                                match code {
                                    4 => cells_in.insert(cell),
                                    5 | 103 | 171 => cells_out.insert(cell),
                                    _ => cells_patch.insert(cell),
                                };
                            }
                        }
                    }
                    57 => {
                        if let (Some(class), Some(id)) = (konst(1), konst(2)) {
                            used_ranges.insert((class as u8, id as u32));
                        } else {
                            return Err(error(inst.at, "a handle of a computed range"));
                        }
                    }
                    // SampleIndex: the pixel's sample number, which the input signature does not list
                    90 => {
                        system_inputs.insert(SystemValue::SampleIndex);
                    }
                    // ThreadId, GroupId, ThreadIdInGroup, FlattenedThreadIdInGroup
                    93 => {
                        system_inputs.insert(SystemValue::DispatchThreadId);
                    }
                    94 => {
                        system_inputs.insert(SystemValue::GroupId);
                    }
                    95 => {
                        system_inputs.insert(SystemValue::GroupThreadId);
                    }
                    96 => {
                        system_inputs.insert(SystemValue::GroupIndex);
                    }
                    // IsFirstLane, LaneIndex and PrefixBitCount read the lane's number; LaneCount the wave's size
                    110 | 111 | 136 => {
                        system_inputs.insert(SystemValue::LaneIndex);
                    }
                    112 => {
                        system_inputs.insert(SystemValue::LaneCount);
                    }
                    151 | 152 => {
                        let Some(row) = konst(1).filter(|r| (0..3).contains(r)) else {
                            return Err(error(inst.at, "a transform read at a computed row"));
                        };
                        let system = if code == 151 {
                            SystemValue::ObjectToWorld
                        } else {
                            SystemValue::WorldToObject
                        };
                        matrix_rows.insert((system, row as u32));
                    }
                    // the ray tracing system values: the ids and the traced ray's state
                    141..=150 | 153 | 154 | 161 | 213 => {
                        if let Some(system) = ray_system_value(code) {
                            system_inputs.insert(system);
                        }
                    }
                    217 => {
                        let Some((class, lower, space)) = binding_of(m, entry, args[1]) else {
                            return Err(error(inst.at, "a handle of a binding that is not constant"));
                        };
                        used_bindings.insert((class, lower, space));
                    }
                    160 => match variable_loaded(m, entry, args[1]) {
                        Some(global) => {
                            used_variables.insert(global);
                        }
                        None => {
                            return Err(error(
                                inst.at,
                                "a handle of a resource that is not a global's value",
                            ));
                        }
                    },
                    _ => {}
                }
            }
        }
        // the signature varyings that cover a used cell, and the system outputs the program writes
        let keeps = |v: &primase_shader::Varying, cells: &BTreeSet<(u32, u8)>, system: &BTreeSet<u8>| {
            match v.provenance.d3d {
                Some(d3d) if d3d.register != u32::MAX => {
                    (0..4u8).any(|lane| d3d.mask >> lane & 1 != 0 && cells.contains(&(d3d.register, lane)))
                }
                _ => match v.system {
                    // DXIL semantic kinds: 17 depth, 18 depth less-equal, 19 depth greater-equal, 14 coverage
                    Some(SystemValue::Depth) => system.contains(&17),
                    Some(SystemValue::DepthLessEqual) => system.contains(&18),
                    Some(SystemValue::DepthGreaterEqual) => system.contains(&19),
                    Some(SystemValue::Coverage) => system.contains(&14),
                    _ => false,
                },
            }
        };
        let none = BTreeSet::new();
        interface.inputs.retain(|v| keeps(v, &cells_in, &none));
        interface.outputs.retain(|v| keeps(v, &cells_out, &system_out));
        match &mut interface.facts {
            StageFacts::Hull(h) => h.patch_constants.retain(|v| keeps(v, &cells_patch, &none)),
            StageFacts::Domain(d) => d.patch_constants.retain(|v| keeps(v, &cells_patch, &none)),
            _ => {}
        }
        // a vertex program that writes no position feeds a tessellator (or a stream-output stage): it is functions
        if interface.stage == Stage::Vertex
            && !interface
                .outputs
                .iter()
                .any(|v| v.system == Some(SystemValue::ClipPosition))
        {
            interface.facts = StageFacts::VertexWithoutPosition;
        }
        // WebGPU gives a lane number only to a thread group laid out along x
        let multi_dimensional = interface.stage == Stage::Compute
            && crate::reflection::num_threads(m)?.is_some_and(|[_, y, z]| y != 1 || z != 1);
        let lane_by_rank = multi_dimensional && system_inputs.remove(&SystemValue::LaneIndex);
        for system in system_inputs {
            if interface.inputs.iter().any(|v| v.system == Some(system)) {
                continue;
            }
            let (name, width) = match system {
                SystemValue::SampleIndex => ("SV_SampleIndex", 1),
                SystemValue::DispatchThreadId => ("SV_DispatchThreadID", 3),
                SystemValue::GroupId => ("SV_GroupID", 3),
                SystemValue::GroupThreadId => ("SV_GroupThreadID", 3),
                SystemValue::LaneIndex => ("SV_LaneIndex", 1),
                SystemValue::LaneCount => ("SV_LaneCount", 1),
                SystemValue::DispatchRaysIndex => ("SV_DispatchRaysIndex", 3),
                SystemValue::DispatchRaysDimensions => ("SV_DispatchRaysDimensions", 3),
                SystemValue::WorldRayOrigin => ("SV_WorldRayOrigin", 3),
                SystemValue::WorldRayDirection => ("SV_WorldRayDirection", 3),
                SystemValue::ObjectRayOrigin => ("SV_ObjectRayOrigin", 3),
                SystemValue::ObjectRayDirection => ("SV_ObjectRayDirection", 3),
                SystemValue::RayTMin => ("SV_RayTMin", 1),
                SystemValue::RayTCurrent => ("SV_RayTCurrent", 1),
                SystemValue::RayFlags => ("SV_RayFlags", 1),
                SystemValue::InstanceId => ("SV_InstanceID", 1),
                SystemValue::InstanceIndex => ("SV_InstanceIndex", 1),
                SystemValue::PrimitiveIndex => ("SV_PrimitiveIndex", 1),
                SystemValue::GeometryIndex => ("SV_GeometryIndex", 1),
                SystemValue::HitKind => ("SV_HitKind", 1),
                _ => ("SV_GroupIndex", 1),
            };
            let float = matches!(
                system,
                SystemValue::WorldRayOrigin
                    | SystemValue::WorldRayDirection
                    | SystemValue::ObjectRayOrigin
                    | SystemValue::ObjectRayDirection
                    | SystemValue::RayTMin
                    | SystemValue::RayTCurrent
            );
            interface.inputs.push(primase_shader::Varying {
                label: primase_shader::Label {
                    name: name.into(),
                    index: 0,
                },
                system: Some(system),
                value: primase_shader::Value {
                    scalar: if float {
                        primase_shader::ScalarType::Float
                    } else {
                        primase_shader::ScalarType::Uint
                    },
                    width,
                },
                interp: None,
                used: true,
                provenance: primase_shader::VaryingProvenance::default(),
            });
        }
        for (system, row) in matrix_rows {
            interface.inputs.push(primase_shader::Varying {
                label: primase_shader::Label {
                    name: if system == SystemValue::ObjectToWorld {
                        "SV_ObjectToWorld"
                    } else {
                        "SV_WorldToObject"
                    }
                    .into(),
                    index: row,
                },
                system: Some(system),
                value: primase_shader::Value {
                    scalar: primase_shader::ScalarType::Float,
                    width: 4,
                },
                interp: None,
                used: true,
                provenance: primase_shader::VaryingProvenance::default(),
            });
        }
        // the resources the program creates handles for, numbered anew
        let mut resources = Vec::new();
        let mut ranges = Vec::new();
        for (original, range) in interface.resources.drain(..).zip(program_ranges) {
            if !used_ranges.contains(&(range.class, range.id))
                && !used_bindings.contains(&(range.class, range.lower, range.space))
                && !range.variable.is_some_and(|g| used_variables.contains(&g))
            {
                continue;
            }
            if original.is_array()
                && !matches!(
                    original.kind,
                    ResourceKind::Texture { .. }
                        | ResourceKind::Sampler { .. }
                        | ResourceKind::Buffer(_)
                        | ResourceKind::Storage { .. }
                )
            {
                return Err(error(
                    0,
                    format!(
                        "{}: arrays of resources other than textures, samplers, buffers and storage ({}, a range of {}) are not lifted",
                        original.provenance.name,
                        match &original.kind {
                            ResourceKind::ConstantBuffer { .. } => "constant buffers",
                            ResourceKind::AccelerationStructure => "acceleration structures",
                            _ => "resources",
                        },
                        original.count
                    ),
                ));
            }
            resources.push(Resource {
                id: ResourceId(resources.len() as u32),
                ..original
            });
            ranges.push(range);
        }
        let heap = heap_resources(m, functions, &mut resources, &mut ranges)?;
        interface.resources = resources;
        let sigs = primase_shader::stage_functions(&interface).map_err(|detail| error(0, detail))?;
        let io = match sigs {
            // the functions of such a program carry no bindings: no entry point to describe
            Some(_) => EntryIo {
                arguments: Vec::new(),
                results: Vec::new(),
            },
            None => entry_io(&interface).map_err(|detail| error(0, detail))?,
        };
        Ok(Setup {
            interface,
            io,
            sigs,
            input_elements,
            output_elements,
            patch_elements,
            ranges,
            notes: BTreeSet::new(),
            lane_by_rank,
            heap,
        })
    }

    /// The input varying whose register and lanes hold this cell, and the lane's position in its value.
    pub(super) fn input_cell(&self, register: u32, lane: u8) -> Option<(usize, u8)> {
        cell(&self.interface.inputs, register, lane)
    }
    pub(super) fn output_cell(&self, register: u32, lane: u8) -> Option<(usize, u8)> {
        cell(&self.interface.outputs, register, lane)
    }

    /// The kept resource of a binding's class, lower bound and register space.
    pub(super) fn resource_by_binding(&self, class: u8, lower: u32, space: u32) -> Option<usize> {
        self.ranges
            .iter()
            .position(|r| r.class == class && r.lower == lower && r.space == space)
    }

    /// The kept resource of a handle's class and range id.
    pub(super) fn resource(&self, class: u8, id: u32) -> Option<usize> {
        self.ranges.iter().position(|r| r.class == class && r.id == id)
    }
    /// The kept resource a library names by this module global.
    pub(super) fn resource_of_variable(&self, global: ValueId) -> Option<usize> {
        self.ranges.iter().position(|r| r.variable == Some(global))
    }
    pub(super) fn kind(&self, resource: usize) -> &ResourceKind {
        &self.interface.resources[resource].kind
    }
}

/// The register space the descriptor heaps' resources are numbered in: apart from every space a program declares.
const HEAP_SPACE: u32 = 0x8000;

/// The resources the program makes handles for from a descriptor heap (Shader Model 6.6 `ResourceDescriptorHeap` and
/// `SamplerDescriptorHeap`). A heap is untyped, so the resource is what the annotation of the handle says it is: one
/// unbounded array of that kind of view, in a register space of its own, indexed by the program's index. The same
/// annotation shares one resource. Appended to `resources` and `ranges`; the result maps each call's result to its
/// resource.
fn heap_resources(
    m: &Module,
    functions: &[&Function],
    resources: &mut Vec<Resource>,
    ranges: &mut Vec<Range>,
) -> Result<BTreeMap<ValueId, usize>> {
    let mut made: BTreeMap<(u8, i64, i64), usize> = BTreeMap::new();
    let mut handles = BTreeMap::new();
    for f in functions.iter().copied() {
        for inst in &f.instructions {
            let (Op::Call { function, args }, Some((handle, _))) = (&inst.op, inst.result) else {
                continue;
            };
            if dx_opcode(m, f, *function, args) != Some(218) {
                continue;
            }
            let sampler = const_int(m, f, args[2]) == Some(1);
            // the annotation that says what the descriptor is: its properties, two constant words
            let properties = functions.iter().copied().find_map(|g| {
                g.instructions.iter().find_map(|i| {
                    let Op::Call { function, args } = &i.op else {
                        return None;
                    };
                    if dx_opcode(m, g, *function, args) != Some(216) || args[1] != handle {
                        return None;
                    }
                    let value = if args[2] < g.first {
                        m.values.get(args[2])?
                    } else {
                        g.locals.get(args[2] - g.first)?
                    };
                    match value {
                        Value::Constant(Constant::Aggregate(items)) => Some((
                            const_int(m, g, *items.first()?)?,
                            const_int(m, g, *items.get(1)?)?,
                        )),
                        Value::Constant(Constant::Null) => Some((0, 0)),
                        _ => None,
                    }
                })
            });
            let Some((first, second)) = properties else {
                return Err(error(
                    inst.at,
                    "a descriptor from a heap that the program does not annotate with its properties",
                ));
            };
            // the properties' words: the resource kind in the low byte; a view is an unordered-access one by 0x1000, a
            // comparison sampler by 0x8000; the second word is a typed view's component type (low byte) and lanes, a
            // structured buffer's stride, a constant buffer's size
            let kind = first & 0xff;
            let class = match (sampler || kind == 14, kind) {
                (true, _) => 3,
                (_, 13) => 2,
                _ if first & 0x1000 != 0 => 1,
                _ => 0,
            };
            let index = match made.get(&(class, first, second)) {
                Some(&i) => i,
                None => {
                    let n = made.len() as u32;
                    let range = Range {
                        class,
                        id: u32::MAX - n,
                        lower: n,
                        count: Resource::UNBOUNDED,
                        extra: match class {
                            3 => u32::from(first & 0x8000 != 0),
                            2 => {
                                u32::try_from(second).map_err(|_| error(inst.at, "a constant buffer size"))?
                            }
                            _ => kind as u32,
                        },
                        space: HEAP_SPACE,
                        element: (class < 2 && kind != 11 && kind != 12).then_some((second & 0xff) as u32),
                        stride: (kind == 12).then_some(second as u32),
                        counter: false,
                        variable: None,
                    };
                    let id = ResourceId(resources.len() as u32);
                    let name = if class == 3 {
                        "SamplerDescriptorHeap"
                    } else {
                        "ResourceDescriptorHeap"
                    };
                    let mut resource = reflection::resource_of(id, &range, name.into(), None, m)
                        .map_err(|e| error(inst.at, e.to_string()))?;
                    // a heap's descriptors have no register of the source's own
                    resource.provenance.d3d = None;
                    resources.push(resource);
                    ranges.push(range);
                    made.insert((class, first, second), resources.len() - 1);
                    resources.len() - 1
                }
            };
            handles.insert(handle, index);
        }
    }
    Ok(handles)
}

fn cell(list: &[primase_shader::Varying], register: u32, lane: u8) -> Option<(usize, u8)> {
    list.iter().enumerate().find_map(|(i, v)| {
        let d3d = v.provenance.d3d?;
        (d3d.register == register && d3d.mask >> lane & 1 != 0)
            .then(|| (i, (d3d.mask & ((1u8 << lane) - 1)).count_ones() as u8))
    })
}

pub(super) fn _unused(_: BTreeMap<u8, u8>, _: Error) {}
