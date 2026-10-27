//! The entry point's metadata, resource records, type annotations and signature parts of an SM6 container, as the
//! records the file stores: the entry point and what its properties state (thread group, geometry, mesh, hull and
//! domain state), its signature elements and their packing, the bound resource ranges of `dx.resources` with their
//! tags, the field annotations of `dx.typeAnnotations`, and the rows of an `ISG1`/`OSG1`/`PSG1` part, each with DXIL's
//! and D3D's own numbers. What the numbers mean for a graphics API is the caller's to decide.
use crate::module::{Constant, Metadata, MetadataId, Module, Type, TypeId, Value, ValueId};
use crate::{Result, error};

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
    pub variable: Option<ValueId>,
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
pub fn library_kind(m: &Module) -> Result<u32> {
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
pub fn entry(m: &Module) -> Result<ValueId> {
    let entry = &the_entry(m)?;
    match operand(node(m, *entry, "entry point")?, 0, "entry point")?.and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => Ok(*value),
        _ => Err(error(0, "entry point names no function")),
    }
}

/// The entry point's thread group size (`numthreads`): the value of the properties' tag 4 (a mesh program's, the first
/// field of its state, tag 9). `None` when the entry
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
        let tag = int(m, *tag, "property tag")?;
        if tag != 4 && tag != 9 {
            continue;
        }
        let mut size = node(
            m,
            value.ok_or_else(|| error(0, "numthreads without a value"))?,
            "numthreads",
        )?;
        // a mesh program states them first in its state
        if tag == 9 {
            size = node(
                m,
                operand(size, 0, "mesh state")?.ok_or_else(|| error(0, "a mesh state without numthreads"))?,
                "numthreads",
            )?;
        }
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
    pub patch_constant: ValueId,
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
/// What a geometry program's entry point states (the properties' tag 1), as stored: the primitive it reads (`input`, a
/// `D3D_PRIMITIVE`: 1 point, 2 line, 3 triangle, 6 line and 7 triangle with adjacency), how many vertices it emits at
/// most, the streams it writes (a mask), the topology its stream makes (a `D3D_PRIMITIVE_TOPOLOGY`: 1 point list, 3
/// line strip, 5 triangle strip) and how many invocations run for a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryState {
    pub input: u32,
    pub max_vertices: u32,
    pub streams: u32,
    pub topology: u32,
    pub instances: u32,
}

/// A geometry program's state.
pub fn geometry_state(m: &Module) -> Result<GeometryState> {
    let state = entry_property(m, 1)?.ok_or_else(|| error(0, "a geometry program without its state"))?;
    let field = |k: usize| -> Result<u32> {
        u32::try_from(int(m, operand(state, k, "geometry state")?, "geometry state")?)
            .map_err(|_| error(0, "geometry state field"))
    };
    Ok(GeometryState {
        input: field(0)?,
        max_vertices: field(1)?,
        streams: field(2)?,
        topology: field(3)?,
        instances: field(4)?,
    })
}

/// What a mesh program's entry point states (the properties' tag 9) after its thread group, as stored: the most
/// vertices and primitives it writes, the topology of its primitives (1 line, 2 triangle) and the size in bytes of the
/// payload it reads from a task program. A null field is zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshState {
    pub max_vertices: u32,
    pub max_primitives: u32,
    pub topology: u32,
    pub payload: u32,
}

/// A mesh program's state.
pub fn mesh_state(m: &Module) -> Result<MeshState> {
    let state = entry_property(m, 9)?.ok_or_else(|| error(0, "a mesh program without its state"))?;
    // a null field is zero
    let field = |k: usize| -> Result<u32> {
        match operand(state, k, "mesh state")? {
            None => Ok(0),
            some => u32::try_from(int(m, some, "mesh state")?).map_err(|_| error(0, "mesh state field")),
        }
    };
    Ok(MeshState {
        max_vertices: field(1)?,
        max_primitives: field(2)?,
        topology: field(3)?,
        payload: field(4)?,
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

/// The module global a resource record's variable names: the global itself, or the one it is a cast of.
fn variable_of(m: &Module, id: Option<MetadataId>) -> Option<ValueId> {
    let Some(Metadata::Value { value, .. }) = id.and_then(|i| m.metadata.get(i)) else {
        return None;
    };
    match m.values.get(*value)? {
        Value::Constant(Constant::Cast { value, .. }) => Some(*value),
        Value::Global { .. } => Some(*value),
        _ => None,
    }
}

/// The bound resource ranges of `dx.resources` (SRVs, UAVs, constant buffers, samplers), each with its variable's name
/// and the type its variable points to (the resource's handle type).
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

/// One field of a struct type, as its annotation in `dx.typeAnnotations` states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldAnnotation {
    pub name: String,
    /// Byte offset in the constant buffer.
    pub offset: u32,
    /// Its `DxilComponentType`: LLVM integers carry no signedness, the annotation does.
    pub component: Option<u32>,
    /// A matrix field's rows, columns and orientation (1 row-major, 2 column-major): LLVM lowers a matrix to an array
    /// of its lines, so only the annotation says it is one.
    pub matrix: Option<(u32, u32, u32)>,
}

/// Fields of a struct type from `dx.typeAnnotations` (field tag 6 the name, 3 the constant-buffer offset, 7 the
/// component type). Empty when the module has no annotation for it.
pub fn fields(m: &Module, t: TypeId) -> Result<Vec<FieldAnnotation>> {
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
                    Ok(FieldAnnotation {
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

/// One element of an `ISG1`/`OSG1`/`PSG1` part as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureElement {
    pub name: String,
    pub index: u32,
    /// `D3D_NAME`.
    pub system_value: u32,
    /// `D3D_REGISTER_COMPONENT_TYPE`.
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    pub read_write_mask: u8,
    /// Where the record starts, in bits of the part.
    pub bit: usize,
}

/// The elements of a signature part (`ISG1`, `OSG1` or `PSG1`): a count and the offset of the first record, then
/// records of 32 bytes (stream, name offset, semantic index, system value, component type, register, mask and
/// read-write mask, minimum precision). A name ends within 256 bytes.
pub fn signature(part: &[u8]) -> Result<Vec<SignatureElement>> {
    let v = primase_foundations::bytes::View::new(part);
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
        out.push(SignatureElement {
            name: String::from_utf8_lossy(&tail[..end]).into_owned(),
            index: word(e + 8)?,
            system_value: word(e + 12)?,
            component_type: word(e + 16)?,
            register: word(e + 20)?,
            mask: mask[0],
            read_write_mask: mask[1],
            bit: e * 8,
        });
    }
    Ok(out)
}
