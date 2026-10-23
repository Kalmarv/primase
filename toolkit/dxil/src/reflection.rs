//! Reflection of an SM6 container: signatures from its `ISG1`/`OSG1` parts, signature packing from the entry
//! point's metadata, and resources and constant-buffer layouts from `dx.resources` and `dx.typeAnnotations` (of the
//! `STAT` part's reflection module when present: the program's own metadata carries no names), mapped into
//! primase's interface vocabulary. D3D's and DXIL's enum numbers stop here: a value the mapping does not know is an
//! error naming it.
use crate::module::{Constant, Metadata, MetadataId, Module, Type, TypeId, Value};
use crate::{Chunk, Result, error};
use primase_shader::d3d;
use primase_shader::{
    Access, BufferKind, BufferLayout, Constant as BufferConstant, ConstantType, D3dBinding, D3dClass,
    DomainFacts, HullFacts, Interface, Majorness, Member, Partitioning, Resource, ResourceId, ResourceKind,
    ResourceProvenance, SampleType, ScalarType, Stage, StageFacts, StorageTarget, TessDomain, TessTopology,
    TextureDim, Value as Lanes, Varying,
};

/// One signature element of the entry point: its id (the `sigId` loadInput and storeOutput name), its first
/// register and column, and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: u32,
    pub name: String,
    /// The first semantic index.
    pub index: u32,
    /// DXIL component type (5 u32, 4 i32, 9 f32, ...).
    pub component_type: u8,
    /// DXIL semantic kind (0 arbitrary, 1 SV_VertexID, 3 SV_Position, 13 SV_IsFrontFace, ...).
    pub system_value: u8,
    /// DXIL interpolation mode (1 constant, 2 linear, 4 linear noperspective, ...).
    pub interpolation: u8,
    pub row: u32,
    pub column: u8,
    pub rows: u32,
    pub columns: u8,
    /// An element the packer did not place (system values read another way): no register.
    pub placed: bool,
}
/// A bound resource range: class (0 SRV, 1 UAV, 2 constant buffer, 3 sampler), range id, first register, count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub class: u8,
    pub id: u32,
    pub lower: u32,
    pub count: u32,
    /// Constant buffers: their size in bytes; SRVs and UAVs: their shape (`DxilResourceKind`, 2 a 2D texture);
    /// samplers: their kind (0 default, 1 comparison, 2 mono).
    pub extra: u32,
    /// The register space.
    pub space: u32,
    /// An SRV's or UAV's element type tag (`DxilComponentType`), where its tags state one.
    pub element: Option<u32>,
    /// A structured SRV's or UAV's element stride in bytes, where its tags state one.
    pub stride: Option<u32>,
    /// An unordered-access view with a hidden counter (an append or consume buffer, `IncrementCounter`).
    pub counter: bool,
    /// The module global a library names the resource by (a program of a library makes its handles from the
    /// global's value).
    pub variable: Option<crate::module::ValueId>,
}

fn node<'a>(m: &'a Module, id: MetadataId, at: &str) -> Result<&'a [Option<MetadataId>]> {
    match m.metadata.get(id) {
        Some(Metadata::Node(ops)) => Ok(ops),
        _ => Err(error(0, format!("{at}: expected a metadata node"))),
    }
}
fn operand(ops: &[Option<MetadataId>], i: usize, at: &str) -> Result<Option<MetadataId>> {
    ops.get(i)
        .copied()
        .ok_or_else(|| error(0, format!("{at}: node has no operand {i}")))
}
/// An integer constant metadata operand.
fn int(m: &Module, id: Option<MetadataId>, at: &str) -> Result<i64> {
    match id.and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => match m.values.get(*value) {
            Some(Value::Constant(Constant::Integer(v))) => Ok(*v),
            // Zero is stored as the null constant.
            Some(Value::Constant(Constant::Null)) => Ok(0),
            _ => Err(error(0, format!("{at}: expected an integer constant"))),
        },
        _ => Err(error(0, format!("{at}: expected an integer"))),
    }
}
fn string(m: &Module, id: Option<MetadataId>) -> String {
    match id.and_then(|i| m.metadata.get(i)) {
        Some(Metadata::String(s)) => String::from_utf8_lossy(s).into_owned(),
        _ => String::new(),
    }
}
fn named<'a>(m: &'a Module, name: &str) -> Option<&'a [MetadataId]> {
    m.named_metadata
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_slice())
}

/// The shader kind a library's entry point states (properties tag 8): the numbers of a program's kind (7 ray
/// generation to 12 callable, 13 mesh).
fn library_kind(m: &Module) -> Result<u32> {
    let entry = node(m, the_entry(m)?, "entry point")?;
    let Some(properties) = entry.get(4).copied().flatten() else {
        return Err(error(0, "a library entry point without properties"));
    };
    for pair in node(m, properties, "entry point properties")?.chunks(2) {
        let [tag, value] = pair else {
            return Err(error(0, "entry point properties: a tag without a value"));
        };
        if int(m, *tag, "property tag")? == 8 {
            return u32::try_from(int(m, *value, "shader kind")?).map_err(|_| error(0, "shader kind"));
        }
    }
    Err(error(0, "a library entry point that states no shader kind"))
}

/// The one entry point's record. A library also has a record with no function, which holds its resources.
fn the_entry(m: &Module) -> Result<MetadataId> {
    let entries = named(m, "dx.entryPoints").ok_or_else(|| error(0, "no dx.entryPoints"))?;
    let mut with_function = Vec::new();
    for &entry in entries {
        if operand(node(m, entry, "entry point")?, 0, "entry point")?.is_some() {
            with_function.push(entry);
        }
    }
    match with_function.as_slice() {
        [entry] => Ok(*entry),
        _ => Err(error(0, "not exactly one entry point")),
    }
}

/// The entry point's function value.
pub fn entry(m: &Module) -> Result<crate::module::ValueId> {
    let entry = &the_entry(m)?;
    match operand(node(m, *entry, "entry point")?, 0, "entry point")?.and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => Ok(*value),
        _ => Err(error(0, "entry point names no function")),
    }
}

/// The entry point's thread group size (`numthreads`): the value of the properties' tag 4. `None` when the entry
/// point states none (every stage but compute).
pub fn num_threads(m: &Module) -> Result<Option<[u32; 3]>> {
    let entry = &the_entry(m)?;
    let entry = node(m, *entry, "entry point")?;
    let Some(properties) = entry.get(4).copied().flatten() else {
        return Ok(None);
    };
    let properties = node(m, properties, "entry point properties")?;
    for pair in properties.chunks(2) {
        let [tag, value] = pair else {
            return Err(error(0, "entry point properties: a tag without a value"));
        };
        if int(m, *tag, "property tag")? != 4 {
            continue;
        }
        let size = node(
            m,
            value.ok_or_else(|| error(0, "numthreads without a value"))?,
            "numthreads",
        )?;
        let axis = |k: usize| -> Result<u32> {
            u32::try_from(int(m, size.get(k).copied().flatten(), "numthreads")?)
                .map_err(|_| error(0, "numthreads: an axis out of range"))
        };
        return Ok(Some([axis(0)?, axis(1)?, axis(2)?]));
    }
    Ok(None)
}

/// The entry point's input and output elements, by id.
pub fn elements(m: &Module) -> Result<(Vec<Element>, Vec<Element>)> {
    let [inputs, outputs, _] = signature_lists(m)?;
    Ok((inputs, outputs))
}

/// A hull or domain program's patch-constant elements, by id.
pub fn patch_elements(m: &Module) -> Result<Vec<Element>> {
    let [_, _, patch] = signature_lists(m)?;
    Ok(patch)
}

/// What a hull program's entry point states about its patch (the properties' tag 3): the patch-constant function, the
/// control-point counts, the tessellator's domain, partitioning and output primitive and the largest factor.
#[derive(Clone, Debug, PartialEq)]
pub struct HullState {
    pub patch_constant: crate::module::ValueId,
    pub input_control_points: u32,
    pub output_control_points: u32,
    pub domain: u32,
    pub partitioning: u32,
    pub output_primitive: u32,
    pub max_tess_factor: f32,
}

/// The value of a properties tag of the entry point, if the entry point states it.
fn entry_property(m: &Module, wanted: i64) -> Result<Option<&[Option<MetadataId>]>> {
    let entry = &the_entry(m)?;
    let entry = node(m, *entry, "entry point")?;
    let Some(properties) = entry.get(4).copied().flatten() else {
        return Ok(None);
    };
    for pair in node(m, properties, "entry point properties")?.chunks(2) {
        let [tag, value] = pair else {
            return Err(error(0, "entry point properties: a tag without a value"));
        };
        if int(m, *tag, "property tag")? == wanted {
            return Ok(Some(node(
                m,
                value.ok_or_else(|| error(0, "a property without a value"))?,
                "property",
            )?));
        }
    }
    Ok(None)
}

/// What a geometry program's entry point states (the properties' tag 1): the primitive it reads, how many vertices it
/// emits at most, the streams it writes, what its stream makes and how many invocations run for a primitive. Only stream 0
/// is carried: another stream is refused by name.
pub fn geometry_facts(m: &Module) -> Result<primase_shader::GeometryFacts> {
    use primase_shader::{GeometryFacts, GsInput, GsOutput};
    let state = entry_property(m, 1)?.ok_or_else(|| error(0, "a geometry program without its state"))?;
    let field = |k: usize| -> Result<u32> {
        u32::try_from(int(m, operand(state, k, "geometry state")?, "geometry state")?)
            .map_err(|_| error(0, "geometry state field"))
    };
    let (input, vertices, streams, topology, instances) =
        (field(0)?, field(1)?, field(2)?, field(3)?, field(4)?);
    if streams != 1 {
        return Err(error(
            0,
            format!("a geometry program that writes streams {streams:#b} (only stream 0 is lifted)"),
        ));
    }
    Ok(GeometryFacts {
        input: match input {
            1 => GsInput::Point,
            2 => GsInput::Line,
            3 => GsInput::Triangle,
            6 => GsInput::LineAdjacency,
            7 => GsInput::TriangleAdjacency,
            other => {
                return Err(error(
                    0,
                    format!("geometry input primitive {other} is not mapped"),
                ));
            }
        },
        output: match topology {
            1 => GsOutput::PointList,
            3 => GsOutput::LineStrip,
            5 => GsOutput::TriangleStrip,
            other => {
                return Err(error(
                    0,
                    format!("geometry output topology {other} is not mapped"),
                ));
            }
        },
        max_vertices: vertices,
        instances: instances.max(1),
    })
}

/// A hull program's patch state.
pub fn hull_state(m: &Module) -> Result<HullState> {
    let state = entry_property(m, 3)?.ok_or_else(|| error(0, "a hull program without its patch state"))?;
    let small = |k: usize| -> Result<u32> {
        u32::try_from(int(m, state.get(k).copied().flatten(), "hull state")?)
            .map_err(|_| error(0, "hull state field"))
    };
    let patch_constant = match state.first().copied().flatten().and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => *value,
        _ => return Err(error(0, "a hull program without its patch-constant function")),
    };
    let max = match state.get(6).copied().flatten().and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => match m.values.get(*value) {
            Some(Value::Constant(Constant::Float(bits))) => f32::from_bits(*bits as u32),
            _ => return Err(error(0, "the maximum tessellation factor is not a float")),
        },
        _ => return Err(error(0, "hull state without a maximum tessellation factor")),
    };
    Ok(HullState {
        patch_constant,
        input_control_points: small(1)?,
        output_control_points: small(2)?,
        domain: small(3)?,
        partitioning: small(4)?,
        output_primitive: small(5)?,
        max_tess_factor: max,
    })
}

/// A domain program's patch state (tag 2): the tessellator domain and the input control point count.
pub fn domain_state(m: &Module) -> Result<(u32, u32)> {
    let state = entry_property(m, 2)?.ok_or_else(|| error(0, "a domain program without its patch state"))?;
    let small = |k: usize| -> Result<u32> {
        u32::try_from(int(m, state.get(k).copied().flatten(), "domain state")?)
            .map_err(|_| error(0, "domain state field"))
    };
    Ok((small(0)?, small(1)?))
}

fn signature_lists(m: &Module) -> Result<[Vec<Element>; 3]> {
    let entry = &the_entry(m)?;
    let entry = node(m, *entry, "entry point")?;
    let Some(signatures) = operand(entry, 2, "entry point")? else {
        return Ok([Vec::new(), Vec::new(), Vec::new()]);
    };
    let signatures = node(m, signatures, "signatures")?;
    let list = |i: usize| -> Result<Vec<Element>> {
        let Some(list) = operand(signatures, i, "signatures")? else {
            return Ok(Vec::new());
        };
        node(m, list, "signature")?
            .iter()
            .map(|e| {
                let e = node(m, e.ok_or_else(|| error(0, "null signature element"))?, "element")?;
                let at = "signature element";
                let row = int(m, operand(e, 8, at)?, at)?;
                let small = |i: usize| -> Result<u8> {
                    u8::try_from(int(m, operand(e, i, at)?, at)?).map_err(|_| error(0, "element field"))
                };
                let index = match operand(e, 4, at)? {
                    Some(list) => match node(m, list, "semantic indices")?.first() {
                        Some(first) => {
                            u32::try_from(int(m, *first, at)?).map_err(|_| error(0, "semantic index"))?
                        }
                        None => 0,
                    },
                    None => 0,
                };
                let element = Element {
                    id: u32::try_from(int(m, operand(e, 0, at)?, at)?).map_err(|_| error(0, "element id"))?,
                    name: string(m, operand(e, 1, at)?),
                    index,
                    component_type: small(2)?,
                    system_value: small(3)?,
                    interpolation: small(5)?,
                    rows: u32::try_from(int(m, operand(e, 6, at)?, at)?)
                        .map_err(|_| error(0, "element rows"))?,
                    columns: u8::try_from(int(m, operand(e, 7, at)?, at)?)
                        .map_err(|_| error(0, "element columns"))?,
                    placed: row >= 0,
                    row: u32::try_from(row.max(0)).map_err(|_| error(0, "element row"))?,
                    column: u8::try_from(int(m, operand(e, 9, at)?, at)?.max(0))
                        .map_err(|_| error(0, "element column"))?,
                };
                // Four lanes per register; at most 32 registers.
                if element.rows == 0
                    || element.columns == 0
                    || u32::from(element.column) + u32::from(element.columns) > 4
                    || element.row.checked_add(element.rows).is_none_or(|end| end > 32)
                {
                    return Err(error(0, "signature element outside the registers"));
                }
                Ok(element)
            })
            .collect()
    };
    Ok([list(0)?, list(1)?, list(2)?])
}

/// A resource's tag list (alternating tag and value): the element type (tag 0) and the structured stride (tag 1).
fn tags(m: &Module, list: Option<MetadataId>) -> Result<(Option<u32>, Option<u32>)> {
    let Some(list) = list else {
        return Ok((None, None));
    };
    let ops = node(m, list, "resource tags")?;
    let (pairs, rest) = ops.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(error(0, "resource tag without its value"));
    }
    let (mut element, mut stride) = (None, None);
    for pair in pairs {
        let value = u32::try_from(int(m, pair[1], "resource tag value")?)
            .map_err(|_| error(0, "resource tag value"))?;
        match int(m, pair[0], "resource tag")? {
            0 => element = Some(value),
            1 => stride = Some(value),
            _ => {}
        }
    }
    Ok((element, stride))
}

/// The bound resource ranges of `dx.resources` (SRVs, UAVs, constant buffers, samplers), with their names.
/// The module global a resource record's variable names: the global itself, or the one it is a cast of.
fn variable_of(m: &Module, id: Option<MetadataId>) -> Option<crate::module::ValueId> {
    let Some(Metadata::Value { value, .. }) = id.and_then(|i| m.metadata.get(i)) else {
        return None;
    };
    match m.values.get(*value)? {
        Value::Constant(Constant::Cast { value, .. }) => Some(*value),
        Value::Global { .. } => Some(*value),
        _ => None,
    }
}

pub fn ranges(m: &Module) -> Result<Vec<(Range, String, Option<TypeId>)>> {
    let Some([resources]) = named(m, "dx.resources") else {
        return Ok(Vec::new());
    };
    let lists = node(m, *resources, "dx.resources")?;
    let mut out = Vec::new();
    for (class, list) in lists.iter().enumerate() {
        let Some(list) = list else { continue };
        for r in node(m, *list, "resource list")? {
            let r = node(m, r.ok_or_else(|| error(0, "null resource"))?, "resource")?;
            let at = "resource";
            // The record's variable is an undef pointer to the resource's type.
            let ty = match operand(r, 1, at)?.and_then(|i| m.metadata.get(i)) {
                Some(Metadata::Value { ty, .. }) => match m.types.get(*ty) {
                    Some(Type::Pointer(t)) => Some(*t),
                    _ => None,
                },
                _ => None,
            };
            let extra =
                u32::try_from(int(m, operand(r, 6, at)?, at)?).map_err(|_| error(0, "resource field"))?;
            let (element, stride) = match class {
                0 => tags(m, r.get(8).copied().flatten())?,
                1 => tags(m, r.get(10).copied().flatten())?,
                _ => (None, None),
            };
            out.push((
                Range {
                    class: class as u8,
                    id: u32::try_from(int(m, operand(r, 0, at)?, at)?)
                        .map_err(|_| error(0, "resource id"))?,
                    lower: u32::try_from(int(m, operand(r, 4, at)?, at)?)
                        .map_err(|_| error(0, "lower bound"))?,
                    // an unbounded range is written -1 (all ones in 32 bits)
                    count: {
                        let v = int(m, operand(r, 5, at)?, at)?;
                        u32::try_from(v)
                            .or_else(|_| i32::try_from(v).map(|v| u32::from_ne_bytes(v.to_ne_bytes())))
                            .map_err(|_| error(0, format!("range size {v}")))?
                    },
                    extra,
                    space: u32::try_from(int(m, operand(r, 3, at)?, at)?)
                        .map_err(|_| error(0, "register space"))?,
                    element,
                    stride,
                    // a UAV's record states whether it has a counter after the shape and the coherence
                    counter: class == 1
                        && r.get(8).copied().flatten().is_some()
                        && int(m, operand(r, 8, at)?, at)? != 0,
                    variable: variable_of(m, operand(r, 1, at)?),
                },
                string(m, operand(r, 2, at)?),
                ty,
            ));
        }
    }
    Ok(out)
}

/// The constant type of an LLVM type, with struct members from the type annotations and D3D's packing.
fn constant_type(m: &Module, t: TypeId, hint: Hint, depth: usize) -> Result<ConstantType> {
    let component = hint.component;
    if depth > 16 {
        return Err(error(0, "type nesting"));
    }
    // a half is read as a float: 32 bits is a valid implementation of it, and the legacy buffer layout gives it a lane
    let scalar = |t: &Type| -> Result<Option<ScalarType>> {
        Ok(match t {
            Type::Float | Type::Half => Some(ScalarType::Float),
            Type::Double => Some(ScalarType::Double),
            Type::Integer(1) => Some(ScalarType::Bool),
            Type::Integer(16 | 32) => Some(match component {
                Some(1) => ScalarType::Bool,
                Some(2 | 4) => ScalarType::Sint,
                Some(3 | 5) => ScalarType::Uint,
                other => {
                    return Err(error(
                        0,
                        format!("an integer constant of component type {other:?} is not mapped"),
                    ));
                }
            }),
            Type::Integer(bits) => {
                return Err(error(0, format!("a constant of {bits} bits is not mapped")));
            }
            _ => None,
        })
    };
    let wrap = |detail: String| error(0, detail);
    let ty = m.types.get(t).ok_or_else(|| error(0, "type id"))?;
    Ok(match ty {
        Type::Vector(n, e) => {
            let scalar = scalar(&m.types[*e])?.ok_or_else(|| error(0, "a vector of a non-scalar"))?;
            d3d::vector(scalar, u32::try_from(*n).map_err(|_| error(0, "vector width"))?).map_err(wrap)?
        }
        // a matrix: an array of its lines, each a vector
        Type::Array(lines, e)
            if hint.matrix.is_some() && matches!(m.types.get(*e), Some(Type::Vector(..))) =>
        {
            let (rows, columns, orientation) = hint.matrix.expect("matched");
            let Some(Type::Vector(lanes, inner)) = m.types.get(*e) else {
                unreachable!("matched")
            };
            let major = match orientation {
                1 => Majorness::Row,
                2 => Majorness::Column,
                other => return Err(error(0, format!("matrix orientation {other} is not mapped"))),
            };
            let (want_lines, want_lanes) = match major {
                Majorness::Row => (rows, columns),
                Majorness::Column => (columns, rows),
            };
            if (*lines, *lanes) != (u64::from(want_lines), u64::from(want_lanes)) {
                return Err(error(0, "a matrix annotation disagrees with its type"));
            }
            let scalar = scalar(&m.types[*inner])?.ok_or_else(|| error(0, "a matrix of a non-scalar"))?;
            d3d::matrix(scalar, rows, columns, major).map_err(wrap)?
        }
        Type::Array(n, e) => d3d::array(
            constant_type(m, *e, hint, depth + 1)?,
            u32::try_from(*n).map_err(|_| error(0, "array length"))?,
        )
        .map_err(wrap)?,
        Type::Struct { elements, .. } => {
            // HLSL vectors are structs of one LLVM vector in some reflection modules.
            if let [single] = elements.as_slice()
                && matches!(m.types.get(*single), Some(Type::Vector(..)))
                && fields(m, t)?.is_empty()
            {
                return constant_type(m, *single, hint, depth + 1);
            }
            let members = fields(m, t)?
                .into_iter()
                .zip(elements)
                .map(|(note, &e)| {
                    Ok(Member {
                        ty: constant_type(m, e, note.hint(), depth + 1)?,
                        name: note.name,
                        offset: note.offset,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            ConstantType::Struct { members }
        }
        other => match scalar(other)? {
            Some(s) => ConstantType::Scalar(s),
            None => return Err(error(0, "unsupported constant-buffer field type")),
        },
    })
}

/// One field of a struct type, from its annotation.
struct Note {
    name: String,
    /// Byte offset in the constant buffer.
    offset: u32,
    /// Its `DxilComponentType`: LLVM integers carry no signedness, the annotation does.
    component: Option<u32>,
    /// A matrix field's rows, columns and orientation (1 row-major, 2 column-major): LLVM lowers a matrix to an array
    /// of its lines, so only the annotation says it is one.
    matrix: Option<(u32, u32, u32)>,
}
/// What an annotation says of the field whose type is being read.
#[derive(Clone, Copy)]
struct Hint {
    component: Option<u32>,
    matrix: Option<(u32, u32, u32)>,
}
impl Note {
    fn hint(&self) -> Hint {
        Hint {
            component: self.component,
            matrix: self.matrix,
        }
    }
}

/// Fields of a struct type from `dx.typeAnnotations` (field tag 6 the name, 3 the constant-buffer offset, 7 the
/// component type). Empty when the module has no annotation for it.
fn fields(m: &Module, t: TypeId) -> Result<Vec<Note>> {
    let Some(annotations) = named(m, "dx.typeAnnotations") else {
        return Ok(Vec::new());
    };
    for &a in annotations {
        let a = node(m, a, "type annotations")?;
        // Struct annotations: kind 0, then (struct type, annotation) pairs.
        if int(m, operand(a, 0, "type annotations")?, "annotation kind")? != 0 {
            continue;
        }
        let (pairs, rest) = a[1..].as_chunks::<2>();
        if !rest.is_empty() {
            return Err(error(0, "struct annotations without their pair"));
        }
        for pair in pairs {
            let ty = match pair[0].and_then(|i| m.metadata.get(i)) {
                Some(Metadata::Value { ty, .. }) => *ty,
                _ => continue,
            };
            if ty != t {
                continue;
            }
            let fields = node(
                m,
                pair[1].ok_or_else(|| error(0, "null struct annotation"))?,
                "struct annotation",
            )?;
            return fields[1..]
                .iter()
                .map(|f| {
                    let f = node(m, f.ok_or_else(|| error(0, "null field"))?, "field annotation")?;
                    let (mut name, mut offset, mut component, mut matrix) = (String::new(), 0, None, None);
                    let (tags, rest) = f.as_chunks::<2>();
                    if !rest.is_empty() {
                        return Err(error(0, "field annotation tag without its value"));
                    }
                    for tag in tags {
                        match int(m, tag[0], "field tag")? {
                            6 => name = string(m, tag[1]),
                            3 => {
                                offset = u32::try_from(int(m, tag[1], "field offset")?)
                                    .map_err(|_| error(0, "field offset"))?
                            }
                            2 => {
                                let ops = node(
                                    m,
                                    tag[1].ok_or_else(|| error(0, "null matrix annotation"))?,
                                    "matrix annotation",
                                )?;
                                let [rows, columns, orientation] = [0, 1, 2].map(|i| {
                                    int(m, operand(ops, i, "matrix annotation")?, "matrix annotation")
                                        .and_then(|v| {
                                            u32::try_from(v).map_err(|_| error(0, "matrix annotation"))
                                        })
                                });
                                matrix = Some((rows?, columns?, orientation?));
                            }
                            7 => {
                                component = Some(
                                    u32::try_from(int(m, tag[1], "field component type")?)
                                        .map_err(|_| error(0, "field component type"))?,
                                )
                            }
                            _ => {}
                        }
                    }
                    Ok(Note {
                        name,
                        offset,
                        component,
                        matrix,
                    })
                })
                .collect();
        }
    }
    Ok(Vec::new())
}

/// D3D signature elements of an `ISG1`/`OSG1` part, mapped for `stage`.
fn signature(part: &[u8], stage: Stage, output: bool) -> Result<Vec<Varying>> {
    let v = primase_bytes::View::new(part);
    let word = |at: usize| -> Result<u32> {
        v.array::<4>(at)
            .map(u32::from_le_bytes)
            .map_err(|e| error(at * 8, e.to_string()))
    };
    let n = word(0)? as usize;
    let first = word(4)? as usize;
    let mut out = Vec::with_capacity(n.min(part.len() / 32));
    for i in 0..n {
        let e = i
            .checked_mul(32)
            .and_then(|o| o.checked_add(first))
            .ok_or_else(|| error(0, "signature element offset"))?;
        let name_at = word(e + 4)? as usize;
        let tail = part
            .get(name_at..)
            .ok_or_else(|| error(name_at * 8, "signature name offset"))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .filter(|&n| n <= 256)
            .ok_or_else(|| error(name_at * 8, "signature name"))?;
        let mask = v.array::<2>(e + 24).map_err(|x| error(e * 8, x.to_string()))?;
        let name = String::from_utf8_lossy(&tail[..end]);
        let element = d3d::Element {
            name: &name,
            index: word(e + 8)?,
            system_value: word(e + 12)?,
            component_type: word(e + 16)?,
            register: word(e + 20)?,
            mask: mask[0],
            read_write_mask: mask[1],
        };
        out.push(d3d::varying(stage, output, &element).map_err(|detail| error(e * 8, detail))?);
    }
    Ok(out)
}

/// A `DxilComponentType` as the scalar type a view of it returns.
fn element_scalar(name: &str, tag: Option<u32>) -> Result<ScalarType> {
    match tag {
        // f16, f32, snorm and unorm f16 and f32
        Some(8 | 9 | 11..=14) => Ok(ScalarType::Float),
        Some(2 | 4) => Ok(ScalarType::Sint),
        Some(3 | 5) => Ok(ScalarType::Uint),
        Some(other) => Err(error(0, format!("{name}: element type {other} is not mapped"))),
        None => Err(error(0, format!("{name}: the resource states no element type"))),
    }
}

/// What a texture or buffer range (SRV or UAV) is, from its `DxilResourceKind`, and its element type tags.
fn view(range: &Range, name: &str, ty: Option<TypeId>, m: &Module) -> Result<View> {
    // texture shapes: dimensionality, array, multisampled
    let texture = |dim, arrayed, multisampled| {
        Ok(View::Texture {
            dim,
            arrayed,
            multisampled,
            scalar: element_scalar(name, range.element)?,
        })
    };
    match range.extra {
        1 => texture(TextureDim::D1, false, false),
        2 => texture(TextureDim::D2, false, false),
        3 => texture(TextureDim::D2, false, true),
        4 => texture(TextureDim::D3, false, false),
        5 => texture(TextureDim::Cube, false, false),
        6 => texture(TextureDim::D1, true, false),
        7 => texture(TextureDim::D2, true, false),
        8 => texture(TextureDim::D2, true, true),
        9 => texture(TextureDim::Cube, true, false),
        10 => Ok(View::Buffer(BufferKind::Typed(Lanes {
            scalar: element_scalar(name, range.element)?,
            // a module that states no width leaves the four lanes the lifter carries a typed buffer's element as
            width: typed_width(m, ty).unwrap_or(4),
        }))),
        11 => Ok(View::Buffer(BufferKind::Raw)),
        16 => Ok(View::AccelerationStructure),
        12 => match range.stride {
            Some(stride) if stride > 0 => Ok(View::Buffer(BufferKind::Structured { stride })),
            _ => Err(error(0, format!("{name}: a structured buffer without a stride"))),
        },
        other => Err(error(
            0,
            format!("{name}: DxilResourceKind {other} is not mapped"),
        )),
    }
}

/// The width of a typed buffer's element: the lanes of the one vector (or scalar) its handle type holds.
fn typed_width(m: &Module, ty: Option<TypeId>) -> Option<u8> {
    let Type::Struct { elements, .. } = m.types.get(ty?)? else {
        return None;
    };
    match m.types.get(*elements.first()?)? {
        Type::Vector(n, _) => u8::try_from(*n).ok().filter(|n| (1..=4).contains(n)),
        Type::Float | Type::Half | Type::Double | Type::Integer(_) => Some(1),
        _ => None,
    }
}

enum View {
    AccelerationStructure,
    Texture {
        dim: TextureDim,
        arrayed: bool,
        multisampled: bool,
        scalar: ScalarType,
    },
    Buffer(BufferKind),
}

fn sample_type(scalar: ScalarType) -> SampleType {
    match scalar {
        ScalarType::Sint => SampleType::Sint,
        ScalarType::Uint => SampleType::Uint,
        _ => SampleType::Float,
    }
}

/// The program module of a container's part of this kind (`DXIL`, or `STAT` for the reflection module), if it has one.
pub fn module(chunks: &[Chunk<'_>], kind: &[u8; 4]) -> Result<Option<Module>> {
    let Some(c) = chunks.iter().find(|c| &c.kind == kind) else {
        return Ok(None);
    };
    let part = crate::program(c.data)?;
    let blocks = crate::bitstream::read(part.bitcode, *b"BC\xc0\xde")?;
    Module::read(&blocks).map(Some)
}

/// The interface of a DXIL container read alone, without checking that its program is within the evaluated subset
/// ([`crate::Shader::parse`] does): stage, varyings with interpolation and resources.
pub fn interface(container: &[u8]) -> Result<Interface> {
    let chunks = crate::chunks(container)?;
    let program = module(&chunks, b"DXIL")?.ok_or_else(|| error(0, "no DXIL part"))?;
    let named = module(&chunks, b"STAT")?;
    reflection(&chunks, &program, named.as_ref())
}

fn tess_domain(code: u32) -> Result<TessDomain> {
    match code {
        1 => Ok(TessDomain::Isoline),
        2 => Ok(TessDomain::Triangle),
        3 => Ok(TessDomain::Quad),
        other => Err(error(0, format!("tessellator domain {other} is not mapped"))),
    }
}

/// The resource a range of the module's resource lists declares, numbered `id`: `name` and `ty` are the record's
/// variable name and handle type.
pub(crate) fn resource_of(
    id: ResourceId,
    range: &Range,
    name: String,
    ty: Option<TypeId>,
    source: &Module,
) -> Result<Resource> {
    let (kind, class) = match range.class {
        0 | 1 => {
            let uav = range.class == 1;
            let kind = match view(range, &name, ty, source)? {
                View::Texture {
                    dim,
                    arrayed,
                    multisampled,
                    scalar,
                } if !uav => ResourceKind::Texture {
                    dim,
                    arrayed,
                    multisampled,
                    sample: sample_type(scalar),
                },
                View::Texture {
                    multisampled: true, ..
                } => {
                    return Err(error(0, format!("{name}: a multisampled unordered-access view")));
                }
                View::Texture {
                    dim, arrayed, scalar, ..
                } => ResourceKind::Storage {
                    target: StorageTarget::Image {
                        dim,
                        arrayed,
                        sample: sample_type(scalar),
                    },
                    access: Access::ReadWrite,
                },
                View::AccelerationStructure if !uav => ResourceKind::AccelerationStructure,
                View::AccelerationStructure => {
                    return Err(error(
                        0,
                        format!("{name}: an unordered-access acceleration structure"),
                    ));
                }
                View::Buffer(buffer) if !uav => ResourceKind::Buffer(buffer),
                View::Buffer(buffer) => ResourceKind::Storage {
                    target: StorageTarget::Buffer(buffer),
                    access: Access::ReadWrite,
                },
            };
            (
                kind,
                if uav {
                    D3dClass::UnorderedAccess
                } else {
                    D3dClass::ShaderResource
                },
            )
        }
        2 => {
            let constants = match ty {
                Some(t) => {
                    let names = fields(source, t)?;
                    let elements = match source.types.get(t) {
                        Some(Type::Struct { elements, .. }) => elements.clone(),
                        _ => Vec::new(),
                    };
                    names
                        .into_iter()
                        .zip(elements)
                        .map(|(note, e)| {
                            Ok(BufferConstant {
                                ty: constant_type(source, e, note.hint(), 0)?,
                                name: note.name,
                                offset: note.offset,
                                default: None,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                }
                None => Vec::new(),
            };
            (
                ResourceKind::ConstantBuffer {
                    layout: BufferLayout {
                        size: range.extra,
                        constants,
                    },
                },
                D3dClass::ConstantBuffer,
            )
        }
        _ => (
            ResourceKind::Sampler {
                comparison: match range.extra {
                    0 => false,
                    1 => true,
                    other => {
                        return Err(error(0, format!("{name}: sampler kind {other} is not mapped")));
                    }
                },
            },
            D3dClass::Sampler,
        ),
    };
    if range.count == 0 {
        return Err(error(0, format!("{name}: a range of no bindings")));
    }
    Ok(Resource {
        id,
        kind,
        count: range.count,
        provenance: ResourceProvenance {
            name,
            d3d: Some(D3dBinding {
                class,
                register: range.lower,
                space: range.space,
            }),
            counter_of: None,
        },
    })
}

/// The interface of a container: stage, signatures, resources, and each pixel input's interpolation from the entry
/// point's signature metadata. `program` is the DXIL module; `named` the STAT part's reflection module when the
/// container has one.
pub fn reflection(chunks: &[Chunk<'_>], program: &Module, named: Option<&Module>) -> Result<Interface> {
    let part = |kind: &[u8; 4]| chunks.iter().find(|c| &c.kind == kind).map(|c| c.data);
    let kind = crate::program(part(b"DXIL").ok_or_else(|| error(0, "no DXIL part"))?)?.program_version >> 16;
    // a library states its entry point's own kind
    let kind = if kind == 6 { library_kind(program)? } else { kind };
    let stage = match kind {
        0 => Stage::Pixel,
        1 => Stage::Vertex,
        2 => Stage::Geometry,
        3 => Stage::Hull,
        4 => Stage::Domain,
        5 => Stage::Compute,
        7 => Stage::RayGeneration,
        8 => Stage::Intersection,
        9 => Stage::AnyHit,
        10 => Stage::ClosestHit,
        11 => Stage::Miss,
        12 => Stage::Callable,
        13 => return Err(error(0, "mesh shader programs are not lifted")),
        14 => return Err(error(0, "amplification shader programs are not lifted")),
        kind => return Err(error(0, format!("program kind {kind} is not mapped"))),
    };
    let source = named.unwrap_or(program);
    let mut resources = Vec::new();
    for (i, (range, name, ty)) in ranges(source)?.into_iter().enumerate() {
        resources.push(resource_of(ResourceId(i as u32), &range, name, ty, source)?);
    }
    let mut inputs = part(b"ISG1")
        .map(|p| signature(p, stage, false))
        .transpose()?
        .unwrap_or_default();
    // a pixel input's interpolation is the entry point's own signature element's, found by register and lanes
    let (elements, _) = elements(program)?;
    for input in &mut inputs {
        let Some(d3d) = input.provenance.d3d else {
            continue;
        };
        let found = elements.iter().find(|e| {
            let lanes = ((1u32 << e.columns) - 1) << e.column;
            e.placed
                && (e.row..e.row + e.rows).contains(&d3d.register)
                && u32::from(d3d.mask) & lanes == u32::from(d3d.mask)
        });
        // only a pixel program's inputs are interpolated
        if let Some(e) = found.filter(|e| e.interpolation != 0 && stage == Stage::Pixel) {
            input.interp = Some(
                d3d::interpolation(u32::from(e.interpolation))
                    .map_err(|detail| error(0, format!("{}{}: {detail}", e.name, e.index)))?,
            );
        }
    }
    let patch = |output: bool| -> Result<Vec<Varying>> {
        Ok(part(b"PSG1")
            .map(|p| signature(p, stage, output))
            .transpose()?
            .unwrap_or_default())
    };
    let facts = match stage {
        Stage::Hull => {
            let h = hull_state(program)?;
            StageFacts::Hull(HullFacts {
                domain: tess_domain(h.domain)?,
                partitioning: match h.partitioning {
                    1 => Partitioning::Integer,
                    2 => Partitioning::Pow2,
                    3 => Partitioning::FractionalOdd,
                    4 => Partitioning::FractionalEven,
                    other => {
                        return Err(error(
                            0,
                            format!("tessellator partitioning {other} is not mapped"),
                        ));
                    }
                },
                topology: match h.output_primitive {
                    1 => TessTopology::Point,
                    2 => TessTopology::Line,
                    3 => TessTopology::TriangleCw,
                    4 => TessTopology::TriangleCcw,
                    other => {
                        return Err(error(
                            0,
                            format!("tessellator output primitive {other} is not mapped"),
                        ));
                    }
                },
                input_control_points: h.input_control_points,
                output_control_points: h.output_control_points,
                max_tess_factor_bits: h.max_tess_factor.to_bits(),
                patch_constants: patch(true)?,
            })
        }
        Stage::Domain => {
            let (domain, input_control_points) = domain_state(program)?;
            StageFacts::Domain(DomainFacts {
                domain: tess_domain(domain)?,
                input_control_points,
                patch_constants: patch(false)?,
            })
        }
        Stage::Geometry => StageFacts::Geometry(geometry_facts(program)?),
        stage if stage.is_ray_tracing() => {
            let function = entry(program)?;
            let export = program
                .name(function)
                .map(crate::rdat::export_name)
                .ok_or_else(|| error(0, "a ray tracing entry point without a name"))?;
            let subobjects = match part(b"RDAT") {
                Some(rdat) => crate::rdat::subobjects(rdat)?,
                None => Vec::new(),
            };
            StageFacts::RayTracing(primase_shader::RayTracingFacts { export, subobjects })
        }
        _ => StageFacts::None,
    };
    Ok(Interface {
        stage,
        inputs,
        facts,
        outputs: part(b"OSG1")
            .map(|p| signature(p, stage, true))
            .transpose()?
            .unwrap_or_default(),
        resources,
    })
}
