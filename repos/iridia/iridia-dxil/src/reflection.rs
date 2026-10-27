//! DXIL's reflection records ([`dxil_reader`]'s, as the file stores them) mapped into the interface vocabulary: the
//! stage, signatures from the `ISG1`/`OSG1`/`PSG1` parts with each pixel input's interpolation from the entry point's
//! signature metadata, resources and constant-buffer layouts from `dx.resources` and `dx.typeAnnotations` (of the
//! `STAT` part's reflection module when present: the program's own metadata carries no names), what a stage's entry
//! point states, and a library's subobjects. D3D's and DXIL's enum numbers stop here: a value the mapping does not know
//! is an error naming it.
use crate::{Result, error, read};
use dxcontainer::Chunk;
use dxil_reader::module::{Module, Type, TypeId};
use dxil_reader::rdat;
use dxil_reader::reflection::{self as records, FieldAnnotation, Range};
use iridia::d3d;
use iridia::{
    Access, BufferKind, BufferLayout, Constant as BufferConstant, ConstantType, D3dBinding, D3dClass,
    DomainFacts, HitGroup, HitGroupKind, HullFacts, Interface, Majorness, Member, Partitioning, Resource,
    ResourceId, ResourceKind, ResourceProvenance, SampleType, ScalarType, Stage, StageFacts, StorageTarget,
    Subobject, TessDomain, TessTopology, TextureDim, Value as Lanes, Varying,
};

/// What a geometry program's entry point states, mapped: the primitive it reads, how many vertices it emits at most, what
/// its stream makes and how many invocations run for a primitive. Only stream 0 is carried: another stream is refused by
/// name.
fn geometry_facts(m: &Module) -> Result<iridia::GeometryFacts> {
    use iridia::{GeometryFacts, GsInput, GsOutput};
    let state = records::geometry_state(m).map_err(read)?;
    let streams = state.streams;
    if streams != 1 {
        return Err(error(
            0,
            format!("a geometry program that writes streams {streams:#b} (only stream 0 is lifted)"),
        ));
    }
    Ok(GeometryFacts {
        input: match state.input {
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
        output: match state.topology {
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
        max_vertices: state.max_vertices,
        instances: state.instances.max(1),
    })
}

/// What a mesh program's entry point states, mapped: the most vertices and primitives it writes and what its primitives
/// are. A payload read from a task program is refused by name.
fn mesh_facts(m: &Module) -> Result<iridia::MeshFacts> {
    use iridia::{MeshFacts, MeshTopology};
    let state = records::mesh_state(m).map_err(read)?;
    let payload = state.payload;
    if payload != 0 {
        return Err(error(
            0,
            format!(
                "a mesh program that reads a task payload of {payload} bytes (amplification programs are not lifted)"
            ),
        ));
    }
    Ok(MeshFacts {
        topology: match state.topology {
            1 => MeshTopology::Line,
            2 => MeshTopology::Triangle,
            other => return Err(error(0, format!("mesh output topology {other} is not mapped"))),
        },
        max_vertices: state.max_vertices,
        max_primitives: state.max_primitives,
    })
}

/// The constant type of an LLVM type, with struct members from the type annotations and D3D's packing.
fn constant_type(
    m: &Module,
    t: TypeId,
    hint: Hint,
    depth: usize,
    packing: d3d::Packing,
) -> Result<ConstantType> {
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
            packing.matrix(scalar, rows, columns, major).map_err(wrap)?
        }
        Type::Array(n, e) => packing
            .array(
                constant_type(m, *e, hint, depth + 1, packing)?,
                u32::try_from(*n).map_err(|_| error(0, "array length"))?,
            )
            .map_err(wrap)?,
        Type::Struct { elements, .. } => {
            // HLSL vectors are structs of one LLVM vector in some reflection modules.
            if let [single] = elements.as_slice()
                && matches!(m.types.get(*single), Some(Type::Vector(..)))
                && fields(m, t)?.is_empty()
            {
                return constant_type(m, *single, hint, depth + 1, packing);
            }
            let notes = fields(m, t)?;
            if packing == d3d::Packing::Structured && notes.len() < elements.len() {
                return Err(error(0, "structured element has no complete member annotations"));
            }
            let mut end = 0u32;
            let members = notes
                .into_iter()
                .zip(elements)
                .map(|(note, &e)| {
                    let ty = constant_type(m, e, Hint::of(&note), depth + 1, packing)?;
                    // STAT annotation offsets describe constant registers even for host-layout storage types.
                    // Storage member order comes from LLVM; its alignment comes from scalar components.
                    let offset = if packing == d3d::Packing::Structured {
                        let offset = end
                            .checked_next_multiple_of(packing.alignment(&ty))
                            .ok_or_else(|| error(0, "structured member alignment overflow"))?;
                        end = offset
                            .checked_add(ty.size().ok_or_else(|| error(0, "structured member size"))?)
                            .and_then(|n| n.checked_next_multiple_of(packing.alignment(&ty)))
                            .ok_or_else(|| error(0, "structured member extent overflow"))?;
                        offset
                    } else {
                        note.offset
                    };
                    Ok(Member {
                        ty,
                        name: note.name,
                        offset,
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

/// What an annotation says of the field whose type is being read.
#[derive(Clone, Copy)]
struct Hint {
    component: Option<u32>,
    matrix: Option<(u32, u32, u32)>,
}
impl Hint {
    fn of(note: &FieldAnnotation) -> Self {
        Hint {
            component: note.component,
            matrix: note.matrix,
        }
    }
}

/// The annotated fields of a struct type (empty where the module has no annotation for it).
fn fields(m: &Module, t: TypeId) -> Result<Vec<FieldAnnotation>> {
    records::fields(m, t).map_err(read)
}

/// The subobjects of the library `rdat` (an `RDAT` part's bytes), mapped.
pub fn subobjects(rdat: &[u8]) -> Result<Vec<Subobject>> {
    rdat::subobjects(rdat)
        .map_err(read)?
        .into_iter()
        .map(|s| {
            Ok(match s {
                rdat::Subobject::StateObjectConfig { name, flags } => {
                    Subobject::StateObjectConfig { name, flags }
                }
                rdat::Subobject::GlobalRootSignature { name, signature } => {
                    Subobject::GlobalRootSignature { name, signature }
                }
                rdat::Subobject::LocalRootSignature { name, signature } => {
                    Subobject::LocalRootSignature { name, signature }
                }
                rdat::Subobject::ExportsAssociation {
                    name,
                    subobject,
                    exports,
                } => Subobject::ExportsAssociation {
                    name,
                    subobject,
                    exports,
                },
                rdat::Subobject::ShaderConfig {
                    name,
                    max_payload_bytes,
                    max_attribute_bytes,
                } => Subobject::ShaderConfig {
                    name,
                    max_payload_bytes,
                    max_attribute_bytes,
                },
                rdat::Subobject::PipelineConfig {
                    name,
                    max_recursion_depth,
                } => Subobject::PipelineConfig {
                    name,
                    max_recursion_depth,
                    flags: None,
                },
                rdat::Subobject::PipelineConfig1 {
                    name,
                    max_recursion_depth,
                    flags,
                } => Subobject::PipelineConfig {
                    name,
                    max_recursion_depth,
                    flags: Some(flags),
                },
                rdat::Subobject::HitGroup {
                    name,
                    kind,
                    any_hit,
                    closest_hit,
                    intersection,
                } => Subobject::HitGroup(HitGroup {
                    name,
                    kind: match kind {
                        0 => HitGroupKind::Triangles,
                        1 => HitGroupKind::ProceduralPrimitives,
                        other => return Err(error(0, format!("RDAT: hit group type {other}"))),
                    },
                    any_hit,
                    closest_hit,
                    intersection,
                }),
            })
        })
        .collect()
}

/// D3D signature elements of an `ISG1`/`OSG1`/`PSG1` part, mapped for `stage`.
fn signature(part: &[u8], stage: Stage, output: bool) -> Result<Vec<Varying>> {
    records::signature(part)
        .map_err(read)?
        .iter()
        .map(|e| {
            let element = d3d::Element {
                name: &e.name,
                index: e.index,
                system_value: e.system_value,
                component_type: e.component_type,
                register: e.register,
                mask: e.mask,
                read_write_mask: e.read_write_mask,
            };
            d3d::varying(stage, output, &element).map_err(|detail| error(e.bit, detail))
        })
        .collect()
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

/// The interface of a DXIL container read alone, without lifting its program ([`crate::lift`] does): stage,
/// varyings with interpolation and resources.
pub fn interface(container: &[u8]) -> Result<Interface> {
    let chunks = dxil_reader::parts(container).map_err(read)?;
    let program = dxil_reader::module(&chunks, b"DXIL")
        .map_err(read)?
        .ok_or_else(|| error(0, "no DXIL part"))?;
    let named = dxil_reader::module(&chunks, b"STAT").map_err(read)?;
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
                                ty: constant_type(source, e, Hint::of(&note), 0, d3d::Packing::Constant)?,
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
    let layout = match &kind {
        ResourceKind::Buffer(BufferKind::Structured { stride })
        | ResourceKind::Storage {
            target: StorageTarget::Buffer(BufferKind::Structured { stride }),
            ..
        } => structured_layout(source, ty, *stride)?,
        _ => None,
    };
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
            layout,
        },
    })
}

/// A structured resource's wrapper has one element field. Names and orientation come from STAT annotations;
/// its storage offsets do not: annotation tag 3 contains the cbuffer layout even for host-layout LLVM types.
fn structured_layout(source: &Module, ty: Option<TypeId>, stride: u32) -> Result<Option<BufferLayout>> {
    let Some(t) = ty else { return Ok(None) };
    let notes = fields(source, t)?;
    if notes.is_empty() {
        return Ok(None);
    }
    let Some(Type::Struct { elements, .. }) = source.types.get(t) else {
        return Err(error(0, "structured resource has no element wrapper"));
    };
    let [element] = elements.as_slice() else {
        return Err(error(0, "structured resource wrapper has more than one element"));
    };
    let ty = constant_type(source, *element, Hint::of(&notes[0]), 0, d3d::Packing::Structured)?;
    let size = ty
        .size()
        .and_then(|n| n.checked_next_multiple_of(d3d::Packing::Structured.alignment(&ty)))
        .ok_or_else(|| error(0, "structured element extent overflow"))?;
    if size != stride {
        return Err(error(
            0,
            format!("structured layout size {size} disagrees with stride {stride}"),
        ));
    }
    Ok(Some(BufferLayout {
        size: stride,
        constants: vec![BufferConstant {
            name: "$Element".into(),
            offset: 0,
            ty,
            default: None,
        }],
    }))
}

/// The interface of a container: stage, signatures, resources, and each pixel input's interpolation from the entry
/// point's signature metadata. `program` is the DXIL module; `named` the STAT part's reflection module when the
/// container has one.
pub fn reflection(chunks: &[Chunk<'_>], program: &Module, named: Option<&Module>) -> Result<Interface> {
    let part = |kind: &[u8; 4]| chunks.iter().find(|c| &c.kind == kind).map(|c| c.data);
    let kind = dxil_reader::program(part(b"DXIL").ok_or_else(|| error(0, "no DXIL part"))?)
        .map_err(read)?
        .program_version
        >> 16;
    // a library states its entry point's own kind
    let kind = if kind == 6 {
        records::library_kind(program).map_err(read)?
    } else {
        kind
    };
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
        13 => Stage::Mesh,
        14 => return Err(error(0, "amplification shader programs are not lifted")),
        kind => return Err(error(0, format!("program kind {kind} is not mapped"))),
    };
    let source = named.unwrap_or(program);
    let mut resources = Vec::new();
    for (i, (range, name, ty)) in records::ranges(source).map_err(read)?.into_iter().enumerate() {
        resources.push(resource_of(ResourceId(i as u32), &range, name, ty, source)?);
    }
    // every program of a stage states its signatures (an empty one is a part with no elements); only a library, whose
    // entry points have none, has no such part
    let signature_part = |kind: &[u8; 4]| -> Result<Option<&[u8]>> {
        match part(kind) {
            None if !stage.is_ray_tracing() => Err(error(
                0,
                format!(
                    "a {stage:?} program without its {} part",
                    String::from_utf8_lossy(kind)
                ),
            )),
            found => Ok(found),
        }
    };
    let mut inputs = signature_part(b"ISG1")?
        .map(|p| signature(p, stage, false))
        .transpose()?
        .unwrap_or_default();
    // a pixel input's interpolation is the entry point's own signature element's, found by register and lanes
    let (elements, _) = records::elements(program).map_err(read)?;
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
        Ok(signature_part(b"PSG1")?
            .map(|p| signature(p, stage, output))
            .transpose()?
            .unwrap_or_default())
    };
    let facts = match stage {
        Stage::Hull => {
            let h = records::hull_state(program).map_err(read)?;
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
            let (domain, input_control_points) = records::domain_state(program).map_err(read)?;
            StageFacts::Domain(DomainFacts {
                domain: tess_domain(domain)?,
                input_control_points,
                patch_constants: patch(false)?,
            })
        }
        Stage::Geometry => StageFacts::Geometry(geometry_facts(program)?),
        Stage::Mesh => StageFacts::Mesh(mesh_facts(program)?),
        stage if stage.is_ray_tracing() => {
            let function = records::entry(program).map_err(read)?;
            let export = program
                .name(function)
                .map(rdat::export_name)
                .ok_or_else(|| error(0, "a ray tracing entry point without a name"))?;
            let subobjects = match part(b"RDAT") {
                Some(data) => subobjects(data)?,
                None => Vec::new(),
            };
            StageFacts::RayTracing(iridia::RayTracingFacts { export, subobjects })
        }
        _ => StageFacts::None,
    };
    Ok(Interface {
        stage,
        inputs,
        facts,
        outputs: signature_part(b"OSG1")?
            .map(|p| signature(p, stage, true))
            .transpose()?
            .unwrap_or_default(),
        resources,
    })
}

#[cfg(test)]
mod structured_tests {
    use iridia::{ConstantType, Majorness, ScalarType};
    const BYTES: &[u8] = include_bytes!("../tests/fixtures/structured_layout.dxil");
    // SDK 10.0.26100.0 compiler dumps: entries stride 96, indices 4, lookup 12.
    // The fixture distinguishes cbuffer offsets (values 16, rows 64) from storage offsets (4, 16).
    #[test]
    fn structured_layout_survives_reflection_and_lifting() {
        check(&super::interface(BYTES).expect("reflection"));
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
    fn stripped_annotations_do_not_invent_element_members() {
        let mut bytes = BYTES.to_vec();
        let count = u32::from_le_bytes(bytes[28..32].try_into().expect("count")) as usize;
        for i in 0..count {
            let p = 32 + i * 4;
            let at = u32::from_le_bytes(bytes[p..p + 4].try_into().expect("offset")) as usize;
            if &bytes[at..at + 4] == b"STAT" {
                bytes[at..at + 4].copy_from_slice(b"PRIV");
            }
        }
        let reflected = super::interface(&bytes).expect("stripped reflection");
        assert!(reflected.resources.iter().all(|r| r.provenance.layout.is_none()));
    }
    #[test]
    fn structured_tail_padding_is_kept_in_the_element_stride() {
        // Both compiler dumps give 16 bytes although the last data member ends at byte 12.
        let bytes = include_bytes!("../tests/fixtures/structured_tail.dxil");
        let reflected = super::interface(bytes).expect("tail reflection");
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
