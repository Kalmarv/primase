//! SM5 RDEF and signature records ([`dxbc_reader`]'s, as the file stores them) mapped into the interface vocabulary;
//! no material or binding policy. D3D's enum numbers stop here: a value the mapping does not know is an error naming
//! it.
use crate::{Result, error, read};
use dxbc_reader::{ProgramType, RdefBinding, RdefType, SignatureElement};
use iridia::d3d;
use iridia::{
    Access, BufferKind, BufferLayout, Constant, ConstantType, D3dBinding, D3dClass, Interface, Majorness,
    Member, Resource, ResourceId, ResourceKind, ResourceProvenance, SampleType, ScalarType, Stage,
    StorageTarget, TextureDim, Value, Varying,
};

/// The pipeline stage of a program type.
pub(crate) fn stage_of(kind: ProgramType) -> Stage {
    match kind {
        ProgramType::Pixel => Stage::Pixel,
        ProgramType::Vertex => Stage::Vertex,
        ProgramType::Geometry => Stage::Geometry,
        ProgramType::Hull => Stage::Hull,
        ProgramType::Domain => Stage::Domain,
        ProgramType::Compute => Stage::Compute,
    }
}

/// A signature element in the vocabulary, or the reason it is not mapped.
pub fn varying(e: &SignatureElement, stage: Stage, output: bool) -> Result<Varying> {
    let element = d3d::Element {
        name: &e.name,
        index: e.index,
        system_value: e.system_value,
        component_type: e.component_type,
        register: e.register,
        mask: e.mask,
        read_write_mask: e.read_write_mask,
    };
    d3d::varying(stage, output, &element).map_err(|detail| error(e.at, detail))
}

/// The signature in whichever of these parts the container has, mapped.
fn signature_of(
    directory: &[dxcontainer::Chunk<'_>],
    tags: &[&[u8; 4]],
    stage: Stage,
    output: bool,
) -> Result<Vec<Varying>> {
    let tag = tags
        .iter()
        .find(|t| directory.iter().any(|c| &&c.kind == *t))
        .unwrap_or(&tags[0]);
    dxbc_reader::signature(dxbc_reader::chunk(directory, tag).map_err(read)?, tag)
        .map_err(read)?
        .iter()
        .map(|e| varying(e, stage, output))
        .collect()
}

/// Read a container's ISGN/OSGN signatures alone, for a container whose RDEF was stripped (as some engines ship
/// them): no resources, so the caller states the constant layout from its own records. Interpolation is the
/// program's to state: [`crate::lift`] reads it from the declarations.
pub fn signatures(bytes: &[u8]) -> Result<Interface> {
    let directory = dxbc_reader::parts(bytes).map_err(read)?;
    let stage = stage_of(dxbc_reader::program_type(&directory).map_err(read)?);
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

/// What one RDEF binding record means.
struct Binding<'a>(&'a RdefBinding);
impl Binding<'_> {
    /// A texture's or typed view's value type, from its return type (normalized formats return floats).
    fn scalar(&self) -> std::result::Result<ScalarType, String> {
        match self.0.returns {
            1 | 2 | 5 => Ok(ScalarType::Float),
            3 => Ok(ScalarType::Sint),
            4 => Ok(ScalarType::Uint),
            r => Err(format!("{}: resource return type {r} is not mapped", self.0.name)),
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
        Ok(match self.0.dimension {
            2 => (TextureDim::D1, false, false),
            3 => (TextureDim::D1, true, false),
            4 => (TextureDim::D2, false, false),
            5 => (TextureDim::D2, true, false),
            6 => (TextureDim::D2, false, true),
            7 => (TextureDim::D2, true, true),
            8 => (TextureDim::D3, false, false),
            9 => (TextureDim::Cube, false, false),
            10 => (TextureDim::Cube, true, false),
            d => return Err(format!("{}: resource dimension {d} is not mapped", self.0.name)),
        })
    }
    /// A typed buffer's element: its scalar type and, from the flags' component bits, its width.
    fn element(&self) -> std::result::Result<Value, String> {
        Ok(Value {
            scalar: self.scalar()?,
            width: ((self.0.flags >> 2) & 3) as u8 + 1,
        })
    }
    fn stride(&self) -> std::result::Result<BufferKind, String> {
        match self.0.samples {
            0 => Err(format!("{}: a structured buffer without a stride", self.0.name)),
            stride => Ok(BufferKind::Structured { stride }),
        }
    }
    /// What the record stands for, leaving constant buffers (whose layout is a table apart) to the caller.
    fn kind(&self) -> std::result::Result<(ResourceKind, D3dClass), String> {
        let storage = |target| ResourceKind::Storage {
            target,
            access: Access::ReadWrite,
        };
        Ok(match self.0.kind {
            // a tbuffer is read as a typed buffer of four-lane rows (float where the record states no return type)
            1 => (
                ResourceKind::Buffer(BufferKind::Typed(self.element().unwrap_or(Value {
                    scalar: ScalarType::Float,
                    width: 4,
                }))),
                D3dClass::ShaderResource,
            ),
            2 if self.0.dimension == 1 => (
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
                    comparison: self.0.flags & 2 != 0,
                },
                D3dClass::Sampler,
            ),
            4 if self.0.dimension == 1 => (
                storage(StorageTarget::Buffer(BufferKind::Typed(self.element()?))),
                D3dClass::UnorderedAccess,
            ),
            4 => {
                let (dim, arrayed, multisampled) = self.shape()?;
                if multisampled {
                    return Err(format!("{}: a multisampled unordered-access view", self.0.name));
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
                    self.0.name,
                    input_type_name(kind)
                ));
            }
        })
    }
}

/// A stored type record in the vocabulary, under the buffer's packing.
fn constant_type(ty: &RdefType, packing: d3d::Packing) -> Result<ConstantType> {
    let mut members = Vec::new();
    for m in &ty.members {
        members.push(Member {
            name: m.name.clone(),
            offset: m.offset,
            ty: constant_type(&m.ty, packing)?,
        });
    }
    let (class, scalar) = (ty.class, u32::from(ty.scalar));
    let (rows, columns) = (u32::from(ty.rows), u32::from(ty.columns));
    let fail = |detail: String| error(ty.word, detail);
    let element = match class {
        0..=3 => {
            let scalar = d3d::scalar_type(scalar).map_err(fail)?;
            match class {
                0 if (rows, columns) == (1, 1) => ConstantType::Scalar(scalar),
                1 if rows == 1 => d3d::vector(scalar, columns).map_err(fail)?,
                2 => packing
                    .matrix(scalar, rows, columns, Majorness::Row)
                    .map_err(fail)?,
                3 => packing
                    .matrix(scalar, rows, columns, Majorness::Column)
                    .map_err(fail)?,
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
    match ty.elements {
        0 => Ok(element),
        n => packing.array(element, u32::from(n)).map_err(fail),
    }
}

/// A container's reflection ([`reflection`]) where it has one this reader reads: `None` where it has no RDEF chunk, or
/// its RDEF's target isn't an SM5.0 pixel or vertex program (a compute program or an earlier model: the lift then
/// names its resources from their declarations, as without reflection). An RDEF of a target it reads that doesn't
/// parse is an error, never taken as absent.
pub fn resource_reflection(bytes: &[u8]) -> Result<Option<Interface>> {
    match dxbc_reader::rdef_target(bytes).map_err(read)? {
        Some(0xffff_0500 | 0xfffe_0500) => reflection(bytes).map(Some),
        _ => Ok(None),
    }
}

/// Read a container's RDEF resource and constant-buffer metadata and its ISGN/OSGN signatures.
pub fn reflection(bytes: &[u8]) -> Result<Interface> {
    let directory = dxbc_reader::parts(bytes).map_err(read)?;
    let rdef = dxbc_reader::rdef(bytes)
        .map_err(read)?
        .ok_or_else(|| error(0, "missing reflection chunk RDEF"))?;
    // SM5.0 pixel or vertex shader (the stage is the RDEF's own target, so reflection reads a container without
    // its code)
    let program_stage = match rdef.target {
        0xffff_0500 => Stage::Pixel,
        0xfffe_0500 => Stage::Vertex,
        _ => return Err(error(4, "expected SM5.0 pixel or vertex RDEF")),
    };
    for binding in &rdef.bindings {
        if binding.count == 0 || binding.flags & !0x1f != 0 {
            return Err(error(
                binding.word,
                format!(
                    "{}: bind count {} and flags {:#x} are not mapped",
                    binding.name, binding.count, binding.flags
                ),
            ));
        }
    }
    // RDEF buffer types: cbuffer (0), tbuffer (1), structured element (3).
    // The last uses storage packing; the other two use constant registers.
    let mut layouts: Vec<(String, BufferLayout, bool, u32)> = Vec::new();
    for buffer in &rdef.buffers {
        let table = match buffer.kind {
            kind @ (0 | 1 | 3) => kind,
            kind => {
                return Err(error(
                    buffer.word,
                    format!(
                        "{}: buffer type {kind} ({}) is not mapped",
                        buffer.name,
                        match kind {
                            2 => "interface pointers",
                            _ => "unknown",
                        }
                    ),
                ));
            }
        };
        let packing = if table == 3 {
            d3d::Packing::Structured
        } else {
            d3d::Packing::Constant
        };
        let mut constants = Vec::new();
        for variable in &buffer.variables {
            let ty = constant_type(&variable.ty, packing)?;
            let packed_size = if table == 3 {
                ty.size()
                    .and_then(|n| n.checked_next_multiple_of(d3d::Packing::Structured.alignment(&ty)))
            } else {
                ty.size()
            };
            if packed_size != Some(variable.size) {
                return Err(error(
                    variable.word,
                    format!(
                        "variable {} is {} bytes, its type packs to {:?}",
                        variable.name, variable.size, packed_size
                    ),
                ));
            }
            constants.push(Constant {
                name: variable.name.clone(),
                offset: variable.offset,
                ty,
                default: variable.default.clone(),
            });
        }
        if layouts.iter().any(|(n, ..)| *n == buffer.name) {
            return Err(error(
                buffer.word,
                format!("constant buffer {} is listed twice", buffer.name),
            ));
        }
        layouts.push((
            buffer.name.clone(),
            BufferLayout {
                size: buffer.size,
                constants,
            },
            false,
            table,
        ));
    }
    let mut resources = Vec::new();
    for (i, record) in rdef.bindings.iter().enumerate() {
        let binding = Binding(record);
        let fail = |detail: String| error(record.word, detail);
        let mut table_layout = None;
        let (kind, class) = if record.kind == 0 || record.kind == 1 {
            // a constant buffer takes a layout of type 0, a tbuffer one of type 1
            let tbuffer = record.kind == 1;
            let (_, layout, claimed, _) = layouts
                .iter_mut()
                .find(|(name, _, _, table)| *name == record.name && *table == u32::from(tbuffer))
                .ok_or_else(|| {
                    fail(format!(
                        "{}: a {} without its layout",
                        record.name,
                        if tbuffer { "tbuffer" } else { "constant buffer" }
                    ))
                })?;
            if std::mem::replace(claimed, true) {
                return Err(fail(format!(
                    "{}: two bindings of one constant buffer",
                    record.name
                )));
            }
            if tbuffer {
                table_layout = Some(layout.clone());
                binding.kind().map_err(fail)?
            } else {
                (
                    ResourceKind::ConstantBuffer {
                        layout: layout.clone(),
                    },
                    D3dClass::ConstantBuffer,
                )
            }
        } else {
            let kind = binding.kind().map_err(fail)?;
            if matches!(record.kind, 5 | 6 | 9 | 10 | 11)
                && let Some((_, layout, claimed, _)) = layouts
                    .iter_mut()
                    .find(|(name, _, _, table)| *name == record.name && *table == 3)
            {
                if std::mem::replace(claimed, true) {
                    return Err(fail(format!(
                        "{}: two bindings of one structured layout",
                        record.name
                    )));
                }
                if layout.size != record.samples {
                    return Err(fail(format!(
                        "{}: element layout {} disagrees with stride {}",
                        record.name, layout.size, record.samples
                    )));
                }
                table_layout = Some(layout.clone());
            }
            kind
        };
        resources.push(Resource {
            id: ResourceId(i as u32),
            kind,
            count: record.count,
            provenance: ResourceProvenance {
                name: record.name.clone(),
                d3d: Some(D3dBinding {
                    class,
                    register: record.register,
                    space: 0,
                }),
                counter_of: None,
                layout: table_layout,
            },
        });
    }
    if let Some((name, ..)) = layouts.iter().find(|(_, _, claimed, _)| !claimed) {
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

#[cfg(test)]
mod structured_tests {
    use iridia::{ConstantType, Majorness, ScalarType};
    const BYTES: &[u8] = include_bytes!("../tests/fixtures/structured_layout.dxbc");
    // SDK 10.0.26100.0 compiler dumps: entries stride 96, indices 4, lookup 12.
    // The fixture distinguishes cbuffer offsets (values 16, rows 64) from storage offsets (4, 16).
    #[test]
    fn structured_layout_survives_reflection_and_lifting() {
        check(&super::reflection(BYTES).expect("reflection"));
        check(&crate::lift(BYTES).expect("lift").interface);
    }

    fn check(interface: &iridia::Interface) {
        let layout = |name: &str| {
            interface
                .resources
                .iter()
                .find(|r| r.provenance.name == name)
                .expect("named resource")
                .provenance
                .layout
                .as_ref()
                .expect("element layout")
        };
        let entry = layout("entries");
        assert_eq!(entry.size, 96);
        let members = entry.constants[0].ty.members();
        assert_eq!(
            members
                .iter()
                .map(|m| (m.name.as_str(), m.offset))
                .collect::<Vec<_>>(),
            [
                ("scalar", 0),
                ("values", 4),
                ("rows", 16),
                ("columns", 40),
                ("nested", 64)
            ]
        );
        assert!(matches!(
            members[1].ty,
            ConstantType::Array {
                count: 3,
                stride: 4,
                ..
            }
        ));
        assert!(matches!(
            members[2].ty,
            ConstantType::Matrix {
                stride: 12,
                major: Majorness::Row,
                ..
            }
        ));
        assert!(matches!(
            members[3].ty,
            ConstantType::Matrix {
                stride: 8,
                major: Majorness::Column,
                ..
            }
        ));
        assert!(matches!(
            members[4].ty,
            ConstantType::Array {
                count: 2,
                stride: 16,
                ..
            }
        ));
        let nested = members[4].ty.members();
        assert_eq!(
            (nested[0].name.as_str(), nested[0].offset, nested[1].offset),
            ("direction", 0, 12)
        );
        assert_eq!(layout("indices").size, 4);
        assert_eq!(
            layout("indices").constants[0].ty,
            ConstantType::Scalar(ScalarType::Uint)
        );
        assert_eq!(layout("lookup").size, 12);
        assert_eq!(
            layout("lookup").constants[0].ty,
            ConstantType::Vector(iridia::Value {
                scalar: ScalarType::Uint,
                width: 3
            })
        );
    }
    #[test]
    fn structured_tail_padding_is_kept_in_the_element_stride() {
        // Both compiler dumps give 16 bytes although the last data member ends at byte 12.
        let bytes = include_bytes!("../tests/fixtures/structured_tail.dxbc");
        let reflected = super::reflection(bytes).expect("tail reflection");
        let layout = reflected
            .resources
            .iter()
            .find(|r| r.provenance.name == "entries")
            .expect("entries")
            .provenance
            .layout
            .as_ref()
            .expect("layout");
        assert_eq!(layout.size, 16);
        assert_eq!(layout.constants[0].ty.size(), Some(12));
        let members = layout.constants[0].ty.members();
        assert_eq!(members[0].ty, ConstantType::Scalar(ScalarType::Double));
        assert_eq!((members[0].offset, members[1].offset), (0, 8));
    }
}
