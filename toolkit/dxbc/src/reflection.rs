//! SM5 RDEF and signature metadata, mapped into primase's interface vocabulary; no material or binding policy.
//! D3D's enum numbers stop here: a value the mapping does not know is an error naming it.
use crate::{Result, chunks, error};
use primase_shader::d3d;
use primase_shader::{
    Access, BufferKind, BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, Interface, Majorness,
    Member, Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, ScalarType, Stage,
    StorageTarget, TextureDim, Value, Varying,
};

struct View<'a>(&'a [u8]);
impl View<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8]> {
        let end = at
            .checked_add(len)
            .ok_or_else(|| error(at / 4, "reflection range overflow"))?;
        self.0
            .get(at..end)
            .ok_or_else(|| error(at / 4, "reflection range exceeds chunk"))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(at, 4)?.try_into().expect("four bytes"),
        ))
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(at, 2)?.try_into().expect("two bytes"),
        ))
    }
    fn string(&self, at: usize) -> Result<String> {
        let tail = self
            .0
            .get(at..)
            .ok_or_else(|| error(at / 4, "invalid string offset"))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error(at / 4, "unterminated reflection string"))?;
        if end > 4096 {
            return Err(error(at / 4, "reflection string exceeds bound"));
        }
        Ok(std::str::from_utf8(&tail[..end])
            .map_err(|_| error(at / 4, "non-UTF8 reflection string"))?
            .to_owned())
    }
    fn table(&self, at: usize, count: usize, stride: usize) -> Result<std::ops::Range<usize>> {
        if count > 65536 {
            return Err(error(at / 4, "reflection table exceeds count bound"));
        }
        self.bytes(
            at,
            count
                .checked_mul(stride)
                .ok_or_else(|| error(at / 4, "table overflow"))?,
        )?;
        Ok(0..count)
    }
    fn ty(&self, at: usize, stack: &mut Vec<usize>, budget: &mut usize) -> Result<ConstantType> {
        if stack.len() >= 32 || stack.contains(&at) || *budget == 0 {
            return Err(error(at / 4, "cyclic or excessive reflection types"));
        }
        *budget -= 1;
        self.bytes(at, 36)?;
        stack.push(at);
        let count = self.u16(at + 10)? as usize;
        let offset = self.u32(at + 12)? as usize;
        let mut members = Vec::new();
        for i in self.table(offset, count, 12)? {
            let p = offset + i * 12;
            members.push(Member {
                name: self.string(self.u32(p)? as usize)?,
                offset: self.u32(p + 8)?,
                ty: self.ty(self.u32(p + 4)? as usize, stack, budget)?,
            });
        }
        stack.pop();
        let (class, scalar) = (self.u16(at)?, u32::from(self.u16(at + 2)?));
        let (rows, columns) = (u32::from(self.u16(at + 4)?), u32::from(self.u16(at + 6)?));
        let fail = |detail: String| error(at / 4, detail);
        let element = match class {
            0..=3 => {
                let scalar = d3d::scalar_type(scalar).map_err(fail)?;
                match class {
                    0 if (rows, columns) == (1, 1) => ConstantType::Scalar(scalar),
                    1 if rows == 1 => d3d::vector(scalar, columns).map_err(fail)?,
                    2 => d3d::matrix(scalar, rows, columns, Majorness::Row).map_err(fail)?,
                    3 => d3d::matrix(scalar, rows, columns, Majorness::Column).map_err(fail)?,
                    _ => return Err(fail(format!("a {rows} by {columns} variable of class {class}"))),
                }
            }
            5 => ConstantType::Struct { members },
            4 => {
                return Err(fail(
                    "a constant-buffer variable of an object type (D3D_SVC_OBJECT)".into(),
                ));
            }
            c => return Err(fail(format!("variable class {c} is not mapped"))),
        };
        match self.u16(at + 8)? {
            0 => Ok(element),
            n => d3d::array(element, u32::from(n)).map_err(fail),
        }
    }
}
/// One element of a signature as stored, before any mapping to the vocabulary.
#[derive(Clone, Debug)]
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
    /// Where the record starts, in words of the chunk.
    pub at: usize,
}
impl SignatureElement {
    /// The element in the vocabulary, or the reason it is not mapped.
    pub fn varying(&self, stage: Stage, output: bool) -> Result<Varying> {
        let element = d3d::Element {
            name: &self.name,
            index: self.index,
            system_value: self.system_value,
            component_type: self.component_type,
            register: self.register,
            mask: self.mask,
            read_write_mask: self.read_write_mask,
        };
        d3d::varying(stage, output, &element).map_err(|detail| error(self.at, detail))
    }
}

/// A signature chunk's records. Layouts: `ISGN`/`OSGN` (name, index, system value, component type, register, masks),
/// `ISG1`/`OSG1` (a stream and a minimum precision besides) and `OSG5` (a stream besides).
fn signature_elements_of(bytes: &[u8], kind: &[u8; 4]) -> Result<Vec<SignatureElement>> {
    let v = View(bytes);
    let count = v.u32(0)? as usize;
    if v.u32(4)? != 8 {
        return Err(error(1, "unsupported signature directory offset"));
    }
    // bytes before the name offset, and the record size
    let (skip, record) = match kind {
        b"ISG1" | b"OSG1" => (4, 32),
        b"OSG5" => (4, 28),
        _ => (0, 24),
    };
    let mut result = Vec::new();
    for i in v.table(8, count, record)? {
        let p = 8 + i * record;
        let q = p + skip;
        let lanes = v.bytes(q + 20, 4)?;
        result.push(SignatureElement {
            name: v.string(v.u32(q)? as usize)?,
            index: v.u32(q + 4)?,
            system_value: v.u32(q + 8)?,
            component_type: v.u32(q + 12)?,
            register: v.u32(q + 16)?,
            mask: lanes[0],
            read_write_mask: lanes[1],
            at: p / 4,
        });
    }
    Ok(result)
}

fn signature(bytes: &[u8], kind: &[u8; 4], stage: Stage, output: bool) -> Result<Vec<Varying>> {
    signature_elements_of(bytes, kind)?
        .iter()
        .map(|e| e.varying(stage, output))
        .collect()
}

/// A hull or domain program's patch-constant signature elements as stored (the `PCSG` chunk).
pub fn patch_signature_elements(bytes: &[u8]) -> Result<Vec<SignatureElement>> {
    let directory = chunks(bytes)?;
    signature_elements_of(chunk(&directory, b"PCSG")?, b"PCSG")
}

/// A container's input and output signature elements as stored, and its stage: for a consumer that maps only the
/// elements it uses ([`signatures`] maps all and refuses the container for one it cannot map, used or not).
pub fn signature_elements(bytes: &[u8]) -> Result<(Stage, Vec<SignatureElement>, Vec<SignatureElement>)> {
    let directory = chunks(bytes)?;
    let stage = stage(&directory)?;
    let read = |tags: &[&[u8; 4]]| -> Result<Vec<SignatureElement>> {
        let tag = tags
            .iter()
            .find(|t| directory.iter().any(|c| &&c.kind == *t))
            .unwrap_or(&tags[0]);
        signature_elements_of(chunk(&directory, tag)?, tag)
    };
    Ok((
        stage,
        read(&[b"ISGN", b"ISG1"])?,
        read(&[b"OSGN", b"OSG1", b"OSG5"])?,
    ))
}

/// The signature in whichever of these chunks the container has.
fn signature_of(
    directory: &[crate::Chunk<'_>],
    tags: &[&[u8; 4]],
    stage: Stage,
    output: bool,
) -> Result<Vec<Varying>> {
    let tag = tags
        .iter()
        .find(|t| directory.iter().any(|c| &&c.kind == *t))
        .unwrap_or(&tags[0]);
    signature(chunk(directory, tag)?, tag, stage, output)
}

/// A container's one chunk of kind `tag`.
fn chunk<'a>(directory: &[crate::Chunk<'a>], tag: &[u8; 4]) -> Result<&'a [u8]> {
    let mut matching = directory.iter().filter(|c| &c.kind == tag);
    let data = matching
        .next()
        .ok_or_else(|| {
            error(
                0,
                format!("missing reflection chunk {}", String::from_utf8_lossy(tag)),
            )
        })?
        .data;
    if matching.next().is_some() {
        return Err(error(0, "duplicate reflection chunk"));
    }
    Ok(data)
}

/// The stage of a container's program: its code chunk's version token names the program type in the high 16 bits.
fn stage(directory: &[crate::Chunk<'_>]) -> Result<Stage> {
    let code = directory
        .iter()
        .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
        .ok_or_else(|| error(0, "missing shader code chunk"))?;
    let token = View(code.data).u32(0)?;
    match token >> 16 {
        0 => Ok(Stage::Pixel),
        1 => Ok(Stage::Vertex),
        2 => Ok(Stage::Geometry),
        3 => Ok(Stage::Hull),
        4 => Ok(Stage::Domain),
        5 => Ok(Stage::Compute),
        kind => Err(error(
            0,
            format!(
                "program type {kind} ({}) is not mapped",
                match kind {
                    2 => "geometry",
                    3 => "hull",
                    4 => "domain",
                    5 => "compute",
                    _ => "unknown",
                }
            ),
        )),
    }
}

/// Read a container's ISGN/OSGN signatures alone, for a container whose RDEF was stripped (as some engines ship
/// them): no resources, so the caller states the constant layout from its own records. Interpolation is the
/// program's to state: [`crate::Shader::parse_stripped`] reads it from the declarations.
pub fn signatures(bytes: &[u8]) -> Result<Interface> {
    let directory = chunks(bytes)?;
    let stage = stage(&directory)?;
    Ok(Interface {
        stage,
        inputs: signature_of(&directory, &[b"ISGN", b"ISG1"], stage, false)?,
        outputs: signature_of(&directory, &[b"OSGN", b"OSG1", b"OSG5"], stage, true)?,
        resources: Vec::new(),
        facts: Default::default(),
    })
}

/// `D3D_SHADER_INPUT_TYPE`'s name for a message.
fn input_type_name(kind: u32) -> &'static str {
    match kind {
        0 => "D3D_SIT_CBUFFER",
        1 => "D3D_SIT_TBUFFER",
        2 => "D3D_SIT_TEXTURE",
        3 => "D3D_SIT_SAMPLER",
        4 => "D3D_SIT_UAV_RWTYPED",
        5 => "D3D_SIT_STRUCTURED",
        6 => "D3D_SIT_UAV_RWSTRUCTURED",
        7 => "D3D_SIT_BYTEADDRESS",
        8 => "D3D_SIT_UAV_RWBYTEADDRESS",
        9 => "D3D_SIT_UAV_APPEND_STRUCTURED",
        10 => "D3D_SIT_UAV_CONSUME_STRUCTURED",
        11 => "D3D_SIT_UAV_RWSTRUCTURED_WITH_COUNTER",
        12 => "D3D_SIT_RTACCELERATIONSTRUCTURE",
        13 => "D3D_SIT_UAV_FEEDBACKTEXTURE",
        _ => "an unknown resource type",
    }
}

/// One RDEF resource record, as stored.
struct Binding {
    name: String,
    /// `D3D_SHADER_INPUT_TYPE`.
    kind: u32,
    /// `D3D_RESOURCE_RETURN_TYPE`.
    returns: u32,
    /// `D3D_SRV_DIMENSION`.
    dimension: u32,
    /// A multisampled texture's sample count; a structured buffer's stride.
    samples: u32,
    register: u32,
    count: u32,
    /// `D3D_SHADER_INPUT_FLAGS`.
    flags: u32,
}
impl Binding {
    /// A texture's or typed view's value type, from its return type (normalized formats return floats).
    fn scalar(&self) -> std::result::Result<ScalarType, String> {
        match self.returns {
            1 | 2 | 5 => Ok(ScalarType::Float),
            3 => Ok(ScalarType::Sint),
            4 => Ok(ScalarType::Uint),
            r => Err(format!("{}: resource return type {r} is not mapped", self.name)),
        }
    }
    fn sample(&self) -> std::result::Result<SampleType, String> {
        Ok(match self.scalar()? {
            ScalarType::Sint => SampleType::Sint,
            ScalarType::Uint => SampleType::Uint,
            _ => SampleType::Float,
        })
    }
    /// A texture dimension's shape: dimensionality, array, multisampled.
    fn shape(&self) -> std::result::Result<(TextureDim, bool, bool), String> {
        Ok(match self.dimension {
            2 => (TextureDim::D1, false, false),
            3 => (TextureDim::D1, true, false),
            4 => (TextureDim::D2, false, false),
            5 => (TextureDim::D2, true, false),
            6 => (TextureDim::D2, false, true),
            7 => (TextureDim::D2, true, true),
            8 => (TextureDim::D3, false, false),
            9 => (TextureDim::Cube, false, false),
            10 => (TextureDim::Cube, true, false),
            d => return Err(format!("{}: resource dimension {d} is not mapped", self.name)),
        })
    }
    /// A typed buffer's element: its scalar type and, from the flags' component bits, its width.
    fn element(&self) -> std::result::Result<Value, String> {
        Ok(Value {
            scalar: self.scalar()?,
            width: ((self.flags >> 2) & 3) as u8 + 1,
        })
    }
    fn stride(&self) -> std::result::Result<BufferKind, String> {
        match self.samples {
            0 => Err(format!("{}: a structured buffer without a stride", self.name)),
            stride => Ok(BufferKind::Structured { stride }),
        }
    }
    /// What the record stands for, leaving constant buffers (whose layout is a table apart) to the caller.
    fn kind(&self) -> std::result::Result<(ResourceKind, D3dClass), String> {
        let storage = |target| ResourceKind::Storage {
            target,
            access: Access::ReadWrite,
        };
        Ok(match self.kind {
            2 if self.dimension == 1 => (
                ResourceKind::Buffer(BufferKind::Typed(self.element()?)),
                D3dClass::ShaderResource,
            ),
            2 => {
                let (dim, arrayed, multisampled) = self.shape()?;
                (
                    ResourceKind::Texture {
                        dim,
                        arrayed,
                        multisampled,
                        sample: self.sample()?,
                    },
                    D3dClass::ShaderResource,
                )
            }
            3 => (
                ResourceKind::Sampler {
                    comparison: self.flags & 2 != 0,
                },
                D3dClass::Sampler,
            ),
            4 if self.dimension == 1 => (
                storage(StorageTarget::Buffer(BufferKind::Typed(self.element()?))),
                D3dClass::UnorderedAccess,
            ),
            4 => {
                let (dim, arrayed, multisampled) = self.shape()?;
                if multisampled {
                    return Err(format!("{}: a multisampled unordered-access view", self.name));
                }
                (
                    storage(StorageTarget::Image {
                        dim,
                        arrayed,
                        sample: self.sample()?,
                    }),
                    D3dClass::UnorderedAccess,
                )
            }
            5 => (ResourceKind::Buffer(self.stride()?), D3dClass::ShaderResource),
            6 => (
                storage(StorageTarget::Buffer(self.stride()?)),
                D3dClass::UnorderedAccess,
            ),
            7 => (ResourceKind::Buffer(BufferKind::Raw), D3dClass::ShaderResource),
            8 => (
                storage(StorageTarget::Buffer(BufferKind::Raw)),
                D3dClass::UnorderedAccess,
            ),
            kind => {
                return Err(format!(
                    "{}: {} ({kind}) is not mapped",
                    self.name,
                    input_type_name(kind)
                ));
            }
        })
    }
}

/// Read a container's RDEF resource and constant-buffer metadata and its ISGN/OSGN signatures.
pub fn reflection(bytes: &[u8]) -> Result<Interface> {
    let directory = chunks(bytes)?;
    let get = |tag: &[u8; 4]| chunk(&directory, tag);
    let v = View(get(b"RDEF")?);
    // SM5.0 pixel or vertex shader (the stage is the RDEF's own target, so reflection reads a container without
    // its code) and RD11 record-size directory.
    let program_stage = match v.u32(16)? {
        0xffff_0500 => Stage::Pixel,
        0xfffe_0500 => Stage::Vertex,
        _ => return Err(error(4, "expected SM5.0 pixel or vertex RDEF")),
    };
    if v.bytes(28, 4)? != b"RD11" {
        return Err(error(4, "expected SM5.0 pixel or vertex RDEF"));
    }
    for (i, size) in [60, 24, 32, 40, 36, 12].into_iter().enumerate() {
        if v.u32(32 + i * 4)? != size {
            return Err(error(8 + i, "unsupported RDEF record size"));
        }
    }
    let mut bindings = Vec::new();
    let offset = v.u32(12)? as usize;
    for i in v.table(offset, v.u32(8)? as usize, 32)? {
        let p = offset + i * 32;
        let binding = Binding {
            name: v.string(v.u32(p)? as usize)?,
            kind: v.u32(p + 4)?,
            returns: v.u32(p + 8)?,
            dimension: v.u32(p + 12)?,
            samples: v.u32(p + 16)?,
            register: v.u32(p + 20)?,
            count: v.u32(p + 24)?,
            flags: v.u32(p + 28)?,
        };
        if binding.count == 0 || binding.flags & !0x1f != 0 {
            return Err(error(
                p / 4,
                format!(
                    "{}: bind count {} and flags {:#x} are not mapped",
                    binding.name, binding.count, binding.flags
                ),
            ));
        }
        bindings.push((p, binding));
    }
    // the constant-buffer table: the layouts of cbuffers (type 0) by name; a structured buffer's element layout
    // (type 3) is not retained, the vocabulary names a structured buffer by its stride
    let mut layouts: Vec<(String, BufferLayout, bool)> = Vec::new();
    let offset = v.u32(4)? as usize;
    let mut budget = 65536;
    for i in v.table(offset, v.u32(0)? as usize, 24)? {
        let p = offset + i * 24;
        let name = v.string(v.u32(p)? as usize)?;
        match v.u32(p + 20)? {
            0 => {}
            3 => continue,
            kind => {
                return Err(error(
                    p / 4,
                    format!(
                        "{name}: buffer type {kind} ({}) is not mapped",
                        match kind {
                            1 => "tbuffer",
                            2 => "interface pointers",
                            _ => "unknown",
                        }
                    ),
                ));
            }
        }
        let size = v.u32(p + 12)?;
        if size > 65536 {
            return Err(error(p / 4, "constant-buffer size exceeds bound"));
        }
        let mut constants = Vec::new();
        let entries = v.u32(p + 8)? as usize;
        for j in v.table(entries, v.u32(p + 4)? as usize, 40)? {
            let q = entries + j * 40;
            let start = v.u32(q + 4)?;
            let len = v.u32(q + 8)?;
            if start.checked_add(len).is_none_or(|end| end > size) {
                return Err(error(q / 4, "variable exceeds constant buffer"));
            }
            let default = v.u32(q + 20)? as usize;
            let ty = v.ty(v.u32(q + 16)? as usize, &mut Vec::new(), &mut budget)?;
            let variable = v.string(v.u32(q)? as usize)?;
            if ty.size() != Some(len) {
                return Err(error(
                    q / 4,
                    format!(
                        "variable {variable} is {len} bytes, its type packs to {:?}",
                        ty.size()
                    ),
                ));
            }
            constants.push(Constant {
                name: variable,
                offset: start,
                ty,
                default: if default == 0 {
                    None
                } else {
                    Some(v.bytes(default, len as usize)?.to_vec())
                },
            });
        }
        if layouts.iter().any(|(n, ..)| *n == name) {
            return Err(error(p / 4, format!("constant buffer {name} is listed twice")));
        }
        layouts.push((name, BufferLayout { size, constants }, false));
    }
    let mut resources = Vec::new();
    for (i, (p, binding)) in bindings.iter().enumerate() {
        let fail = |detail: String| error(p / 4, detail);
        let (kind, class) = if binding.kind == 0 {
            let (_, layout, claimed) = layouts
                .iter_mut()
                .find(|(name, ..)| *name == binding.name)
                .ok_or_else(|| fail(format!("{}: a constant buffer without its layout", binding.name)))?;
            if std::mem::replace(claimed, true) {
                return Err(fail(format!(
                    "{}: two bindings of one constant buffer",
                    binding.name
                )));
            }
            (
                ResourceKind::ConstantBuffer {
                    layout: layout.clone(),
                },
                D3dClass::ConstantBuffer,
            )
        } else {
            binding.kind().map_err(fail)?
        };
        resources.push(Resource {
            id: ResourceId(i as u32),
            kind,
            count: binding.count,
            provenance: ResourceProvenance {
                name: binding.name.clone(),
                d3d: Some(D3dBinding {
                    class,
                    register: binding.register,
                    space: 0,
                }),
            },
        });
    }
    if let Some((name, ..)) = layouts.iter().find(|(.., claimed)| !claimed) {
        return Err(error(
            0,
            format!("constant buffer {name} has no resource binding"),
        ));
    }
    Ok(Interface {
        stage: program_stage,
        inputs: signature_of(&directory, &[b"ISGN", b"ISG1"], program_stage, false)?,
        outputs: signature_of(&directory, &[b"OSGN", b"OSG1", b"OSG5"], program_stage, true)?,
        resources,
        facts: Default::default(),
    })
}
