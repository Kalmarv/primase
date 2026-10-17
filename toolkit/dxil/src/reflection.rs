//! Reflection of an SM6 container: signatures from its `ISG1`/`OSG1` parts, signature packing from the entry
//! point's metadata, and resources and constant-buffer layouts from `dx.resources` and `dx.typeAnnotations` (of the
//! `STAT` part's reflection module when present: the program's own metadata carries no names).
use crate::module::{Constant, Metadata, MetadataId, Module, Type, TypeId, Value};
use crate::{Chunk, Result, error};
use primase_shader::{Buffer, Member, Reflection, Resource, Semantic, Variable};

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
    /// Constant buffers: their size in bytes; SRVs: their shape (2 a 2D texture).
    pub extra: u32,
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

/// The entry point's function value.
pub fn entry(m: &Module) -> Result<crate::module::ValueId> {
    let entries = named(m, "dx.entryPoints").ok_or_else(|| error(0, "no dx.entryPoints"))?;
    let [entry] = entries else {
        return Err(error(0, "not exactly one entry point"));
    };
    match operand(node(m, *entry, "entry point")?, 0, "entry point")?.and_then(|i| m.metadata.get(i)) {
        Some(Metadata::Value { value, .. }) => Ok(*value),
        _ => Err(error(0, "entry point names no function")),
    }
}

/// The entry point's input and output elements, by id.
pub fn elements(m: &Module) -> Result<(Vec<Element>, Vec<Element>)> {
    let entries = named(m, "dx.entryPoints").ok_or_else(|| error(0, "no dx.entryPoints"))?;
    let [entry] = entries else {
        return Err(error(0, "not exactly one entry point"));
    };
    let entry = node(m, *entry, "entry point")?;
    let Some(signatures) = operand(entry, 2, "entry point")? else {
        return Ok((Vec::new(), Vec::new()));
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
    Ok((list(0)?, list(1)?))
}

/// The bound resource ranges of `dx.resources` (SRVs, UAVs, constant buffers, samplers), with their names.
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
            let extra = match class {
                0 | 2 => {
                    u32::try_from(int(m, operand(r, 6, at)?, at)?).map_err(|_| error(0, "resource field"))?
                }
                _ => 0,
            };
            out.push((
                Range {
                    class: class as u8,
                    id: u32::try_from(int(m, operand(r, 0, at)?, at)?)
                        .map_err(|_| error(0, "resource id"))?,
                    lower: u32::try_from(int(m, operand(r, 4, at)?, at)?)
                        .map_err(|_| error(0, "lower bound"))?,
                    count: u32::try_from(int(m, operand(r, 5, at)?, at)?)
                        .map_err(|_| error(0, "range size"))?,
                    extra,
                },
                string(m, operand(r, 2, at)?),
                ty,
            ));
        }
    }
    Ok(out)
}

/// D3D reflection's type description of an LLVM type, with struct members from the type annotations.
fn d3d_type(m: &Module, t: TypeId, depth: usize) -> Result<primase_shader::Type> {
    if depth > 16 {
        return Err(error(0, "type nesting"));
    }
    let scalar = |t: &Type| match t {
        Type::Float => Some(3),
        Type::Double => Some(39),
        Type::Half => Some(3),
        Type::Integer(1) => Some(1),
        Type::Integer(_) => Some(2),
        _ => None,
    };
    let ty = m.types.get(t).ok_or_else(|| error(0, "type id"))?;
    Ok(match ty {
        Type::Vector(n, e) => primase_shader::Type {
            class: 1,
            scalar: scalar(&m.types[*e]).unwrap_or(0),
            rows: 1,
            columns: *n as u16,
            elements: 0,
            members: Vec::new(),
        },
        Type::Array(n, e) => primase_shader::Type {
            elements: *n as u16,
            ..d3d_type(m, *e, depth + 1)?
        },
        Type::Struct { elements, .. } => {
            // HLSL vectors are structs of one LLVM vector in some reflection modules.
            if let [single] = elements.as_slice()
                && matches!(m.types.get(*single), Some(Type::Vector(..)))
                && fields(m, t)?.is_empty()
            {
                return d3d_type(m, *single, depth + 1);
            }
            let members = fields(m, t)?
                .into_iter()
                .zip(elements)
                .map(|((name, offset), &e)| {
                    Ok(Member {
                        name,
                        offset,
                        ty: d3d_type(m, e, depth + 1)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            primase_shader::Type {
                class: 5,
                scalar: 0,
                rows: 1,
                columns: 1,
                elements: 0,
                members,
            }
        }
        other => match scalar(other) {
            Some(s) => primase_shader::Type {
                class: 0,
                scalar: s,
                rows: 1,
                columns: 1,
                elements: 0,
                members: Vec::new(),
            },
            None => return Err(error(0, "unsupported constant-buffer field type")),
        },
    })
}

/// Field names and byte offsets of a struct type from `dx.typeAnnotations` (field tag 6 the name, 3 the
/// constant-buffer offset). Empty when the module has no annotation for it.
fn fields(m: &Module, t: TypeId) -> Result<Vec<(String, u32)>> {
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
                    let (mut name, mut offset) = (String::new(), 0);
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
                            _ => {}
                        }
                    }
                    Ok((name, offset))
                })
                .collect();
        }
    }
    Ok(Vec::new())
}

/// D3D signature elements of an `ISG1`/`OSG1` part.
fn signature(part: &[u8]) -> Result<Vec<Semantic>> {
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
        out.push(Semantic {
            name: String::from_utf8_lossy(&tail[..end]).into_owned(),
            index: word(e + 8)?,
            system_value: word(e + 12)?,
            component_type: word(e + 16)?,
            register: word(e + 20)?,
            mask: mask[0],
            used_mask: mask[1],
        });
    }
    Ok(out)
}

/// The container's reflection. `program` is the DXIL module; `named` the STAT part's reflection module when the
/// container has one.
pub fn reflection(chunks: &[Chunk<'_>], program: &Module, named: Option<&Module>) -> Result<Reflection> {
    let part = |kind: &[u8; 4]| chunks.iter().find(|c| &c.kind == kind).map(|c| c.data);
    let source = named.unwrap_or(program);
    let mut resources = Vec::new();
    let mut buffers = Vec::new();
    for (range, name, ty) in ranges(source)? {
        let kind = match range.class {
            0 => 2,
            1 => 4,
            2 => 0,
            _ => 3,
        };
        resources.push(Resource {
            name: name.clone(),
            kind,
            return_type: if range.class == 0 { 5 } else { 0 },
            dimension: if range.class == 0 && range.extra == 2 {
                4
            } else {
                0
            },
            slot: range.lower,
            count: range.count,
        });
        if range.class == 2 {
            let variables = match ty {
                Some(t) => {
                    let names = fields(source, t)?;
                    let elements = match source.types.get(t) {
                        Some(Type::Struct { elements, .. }) => elements.clone(),
                        _ => Vec::new(),
                    };
                    names
                        .into_iter()
                        .zip(elements)
                        .map(|((name, offset), e)| {
                            let ty = d3d_type(source, e, 0)?;
                            // The bitcode states no size; D3D reflection's is the packed size.
                            let size = ty
                                .packed_words()
                                .and_then(|w| w.checked_mul(4))
                                .ok_or_else(|| error(0, format!("constant {name} has no packed size")))?;
                            Ok(Variable {
                                name,
                                offset,
                                size,
                                ty,
                                default: None,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                }
                None => Vec::new(),
            };
            buffers.push(Buffer {
                name,
                size: range.extra,
                variables,
            });
        }
    }
    Ok(Reflection {
        buffers,
        resources,
        inputs: part(b"ISG1").map(signature).transpose()?.unwrap_or_default(),
        outputs: part(b"OSG1").map(signature).transpose()?.unwrap_or_default(),
    })
}
