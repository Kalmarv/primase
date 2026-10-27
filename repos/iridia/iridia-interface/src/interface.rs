//! A program's interface: the varyings it reads and writes, and the system values among them.

use serde::Serialize;

use super::{BufferKind, BufferLayout, D3dClass, Resource, ResourceId, ResourceKind, Stage, StorageTarget};

/// The scalar type of a value or constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    /// A boolean, 32 bits wide wherever it is stored (constant buffers, signatures).
    Bool,
    Sint,
    Uint,
    Float,
    Double,
}
impl ScalarType {
    /// Bytes one scalar takes in a constant buffer.
    pub fn bytes(self) -> u32 {
        match self {
            Self::Double => 8,
            _ => 4,
        }
    }
}

/// A scalar type and a width of 1 to 4 lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Value {
    pub scalar: ScalarType,
    pub width: u8,
}

/// A varying's name in its source: the semantic name and its index (DXBC `TEXCOORD` and 5, SPIR-V `Location` and 3,
/// GLSL `vUv` and 0). Labels are not interpreted: games put anything in `TEXCOORDn`, and what a label means is a
/// game resolve pass's business. Stages match by label.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Label {
    pub name: String,
    pub index: u32,
}

/// The system values, one vocabulary for every API: what the pipeline itself supplies to or takes from a stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemValue {
    /// A vertex stage's clip-space position output.
    ClipPosition,
    /// A pixel stage's window-space position input (x, y, depth, 1/w).
    FragCoord,
    FrontFacing,
    VertexIndex,
    /// A vertex stage's instance number; in a ray tracing hit program, the instance's index in the acceleration
    /// structure.
    InstanceIndex,
    /// A pixel stage's primitive number; in a ray tracing hit program, the hit primitive's index in its geometry.
    PrimitiveIndex,
    SampleIndex,
    RenderTargetArrayIndex,
    ViewportArrayIndex,
    /// Clip distances; the label's index says which array element.
    ClipDistance,
    /// Cull distances; the label's index says which array element.
    CullDistance,
    /// Colour output `n`.
    Target(u32),
    /// A pixel stage's depth output, written freely.
    Depth,
    /// Depth output the program promises is not less than the rasterized depth.
    DepthGreaterEqual,
    /// Depth output the program promises is not greater than the rasterized depth.
    DepthLessEqual,
    StencilRef,
    /// Coverage mask, as an input or an output.
    Coverage,
    /// Coverage of the pixel by the primitive alone (conservative rasterization).
    InnerCoverage,
    /// A compute stage's thread group (workgroup) number, three components.
    GroupId,
    /// A compute invocation's position inside its thread group, three components.
    GroupThreadId,
    /// A compute invocation's position in the whole dispatch (group number times the group size plus its position in
    /// the group), three components.
    DispatchThreadId,
    /// A compute invocation's index in its thread group, row-major over the group's three axes.
    GroupIndex,
    /// The invocation's lane in its wave (subgroup).
    LaneIndex,
    /// How many lanes the wave (subgroup) has.
    LaneCount,
    /// A ray tracing program's position in the dispatch, three components.
    DispatchRaysIndex,
    /// The size of the dispatch a ray tracing program runs in, three components.
    DispatchRaysDimensions,
    /// The traced ray's origin and direction in world space, three components each.
    WorldRayOrigin,
    WorldRayDirection,
    /// The same in the space of the instance the ray hit (an intersection, any-hit or closest-hit program).
    ObjectRayOrigin,
    ObjectRayDirection,
    /// The ray's start and the distance of the closest accepted hit so far (or the candidate being tested, in an
    /// any-hit program), along the ray.
    RayTMin,
    RayTCurrent,
    /// The traced ray's flags.
    RayFlags,
    /// The hit instance's own identifier (the 24 bits the acceleration structure was built with).
    InstanceId,
    /// The geometry's index within the hit instance's bottom-level acceleration structure.
    GeometryIndex,
    /// What the intersection program reported the hit as (`ReportHit`'s kind).
    HitKind,
    /// The hit instance's 3 by 4 transform to world space and back. One varying for each row used, the label's index
    /// naming the row, its four lanes the row's four columns.
    ObjectToWorld,
    WorldToObject,
}

/// How a varying is interpolated across a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpMode {
    /// Perspective-correct (the default for an interpolant).
    Perspective,
    /// Linear in screen space (HLSL `noperspective`).
    Linear,
    /// The provoking vertex's value, not interpolated.
    Flat,
}

/// Where in a pixel a varying is sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpSampling {
    Center,
    Centroid,
    /// Per sample.
    Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Interpolation {
    pub mode: InterpMode,
    pub sampling: InterpSampling,
}

/// Where a D3D signature element sits: its register (four 32-bit lanes each) and the lanes of it the element
/// occupies, as a mask (bit k is lane k, x first), and the signature's read/write mask as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct D3dSignature {
    pub register: u32,
    pub mask: u8,
    /// The signature's second mask as stored: for an input, the lanes the program always reads; for an output, the
    /// lanes it never writes.
    pub read_write_mask: u8,
}

/// The source's own address of a varying; one field per API that has a notion of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct VaryingProvenance {
    pub d3d: Option<D3dSignature>,
}

/// One interface variable: a value passed between stages (or between a stage and the pipeline), one per label. D3D
/// packs several into one register; they are separate here, with the register and lanes kept in provenance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Varying {
    /// The source's name for it, kept for system values too (`SV_Target` and 1).
    pub label: Label,
    /// What the pipeline itself supplies or takes here, when it is a system value.
    pub system: Option<SystemValue>,
    pub value: Value,
    /// How it is interpolated, where the source states it for this varying: a pixel input the program declares.
    /// `None` for outputs, for stages that do not interpolate, and for an input the program never declares (a source
    /// that carries interpolation in the program body leaves it out of a signature read alone).
    pub interp: Option<Interpolation>,
    /// Whether the program reads (an input) or writes (an output) at least one lane, as the source's signature
    /// states.
    pub used: bool,
    pub provenance: VaryingProvenance,
}

/// The shape a tessellator gives a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TessDomain {
    Isoline,
    Triangle,
    Quad,
}

/// How the tessellator spaces the vertices it adds along an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Partitioning {
    Integer,
    Pow2,
    FractionalOdd,
    FractionalEven,
}

/// What the tessellator emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TessTopology {
    Point,
    Line,
    TriangleCw,
    TriangleCcw,
}

/// What a hull program states about its patch beyond its varyings. The patch-constant varyings are its outputs per
/// patch (`SV_TessFactor`, `SV_InsideTessFactor`, whatever the domain program reads); `outputs` of the interface are
/// its control points'.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HullFacts {
    pub domain: TessDomain,
    pub partitioning: Partitioning,
    pub topology: TessTopology,
    pub input_control_points: u32,
    pub output_control_points: u32,
    /// The largest tessellation factor, as the bits of an `f32` (the interface is hashed).
    pub max_tess_factor_bits: u32,
    pub patch_constants: Vec<Varying>,
}

/// What a domain program states about its patch: the same domain as the hull program that feeds it, how many control
/// points it reads, and the patch-constant varyings it reads (`inputs` of the interface are its control points').
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct DomainFacts {
    pub domain: TessDomain,
    pub input_control_points: u32,
    pub patch_constants: Vec<Varying>,
}

/// The primitive a geometry program reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GsInput {
    Point,
    Line,
    Triangle,
    LineAdjacency,
    TriangleAdjacency,
}
impl GsInput {
    /// How many vertices the primitive has.
    pub fn vertices(self) -> u32 {
        match self {
            Self::Point => 1,
            Self::Line => 2,
            Self::Triangle => 3,
            Self::LineAdjacency => 4,
            Self::TriangleAdjacency => 6,
        }
    }
}

/// What a geometry program's emitted vertices make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GsOutput {
    PointList,
    LineStrip,
    TriangleStrip,
}

/// What a geometry program states: the primitive it reads, what its emitted vertices make, the most vertices an
/// invocation emits and how many invocations run for a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct GeometryFacts {
    pub input: GsInput,
    pub output: GsOutput,
    pub max_vertices: u32,
    pub instances: u32,
}

/// What a mesh program's primitives are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshTopology {
    Line,
    Triangle,
}
impl MeshTopology {
    /// How many vertex indices a primitive has.
    pub fn indices(self) -> u32 {
        match self {
            Self::Line => 2,
            Self::Triangle => 3,
        }
    }
}

/// What a mesh program states: what its primitives are and the most vertices and primitives it writes. Its thread group
/// size is the entry point's, as a compute program's is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct MeshFacts {
    pub topology: MeshTopology,
    pub max_vertices: u32,
    pub max_primitives: u32,
}

/// What a hit group runs for the geometry it is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitGroupKind {
    /// Triangles: the built-in intersection test, an optional any-hit and closest-hit program.
    Triangles,
    /// Procedural primitives: an intersection program of the library, an optional any-hit and closest-hit program.
    ProceduralPrimitives,
}

/// A hit group of a ray tracing library: the programs it runs, named by the library's exports (none where the group has
/// no such program).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct HitGroup {
    pub name: String,
    pub kind: HitGroupKind,
    pub any_hit: Option<String>,
    pub closest_hit: Option<String>,
    pub intersection: Option<String>,
}

/// A state object subobject of a ray tracing library, as the library declares it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Subobject {
    /// The state object's flags (D3D12's `D3D12_STATE_OBJECT_FLAGS`).
    StateObjectConfig {
        name: String,
        flags: u32,
    },
    /// A root signature as D3D serialised it, for every program of the state object.
    GlobalRootSignature {
        name: String,
        signature: Vec<u8>,
    },
    /// A root signature as D3D serialised it, for the programs an association names.
    LocalRootSignature {
        name: String,
        signature: Vec<u8>,
    },
    /// The exports (programs and hit groups, by name) another subobject applies to.
    ExportsAssociation {
        name: String,
        subobject: String,
        exports: Vec<String>,
    },
    /// The largest payload and hit attributes the state object's programs use.
    ShaderConfig {
        name: String,
        max_payload_bytes: u32,
        max_attribute_bytes: u32,
    },
    /// The deepest `TraceRay` recursion, and the pipeline flags where the library states them.
    PipelineConfig {
        name: String,
        max_recursion_depth: u32,
        flags: Option<u32>,
    },
    HitGroup(HitGroup),
}

/// What a ray tracing program's library states: the program's own export name (as the subobjects name it) and every
/// subobject of the library, in the order it stores them. A pipeline is built from a library's programs and these.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct RayTracingFacts {
    pub export: String,
    pub subobjects: Vec<Subobject>,
}

/// Facts about a program a stage's varyings and resources do not carry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StageFacts {
    #[default]
    None,
    /// A vertex program that writes no clip-space position: its outputs feed a hull program (or a stream-output
    /// stage), so the program is a function (`main`) and not an entry point.
    VertexWithoutPosition,
    Hull(HullFacts),
    Domain(DomainFacts),
    Geometry(GeometryFacts),
    /// A ray tracing program of a library (see [`RayTracingFacts`]).
    RayTracing(RayTracingFacts),
    /// A mesh program.
    Mesh(MeshFacts),
}

/// What a program exposes: its stage, the varyings it takes and gives, and the resources it binds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Interface {
    pub stage: Stage,
    pub inputs: Vec<Varying>,
    pub outputs: Vec<Varying>,
    pub resources: Vec<Resource>,
    pub facts: StageFacts,
}
impl Interface {
    /// The facts agree with the stage: a hull or domain program states its patch, nothing else does, and a vertex
    /// program that feeds a tessellator is a vertex program.
    fn verify_facts(&self) -> Result<(), String> {
        let patch = |what: &str, count: u32, constants: &[Varying]| -> Result<(), String> {
            if !(1..=32).contains(&count) {
                return Err(format!("{what}: {count} control points, not 1 to 32"));
            }
            for (i, v) in constants.iter().enumerate() {
                if !(1..=4).contains(&v.value.width) {
                    return Err(format!(
                        "patch constant {}{} has a width of {}, not 1 to 4",
                        v.label.name, v.label.index, v.value.width
                    ));
                }
                if constants[..i].iter().any(|other| other.label == v.label) {
                    return Err(format!(
                        "patch constant {}{} is listed twice",
                        v.label.name, v.label.index
                    ));
                }
            }
            Ok(())
        };
        match (&self.stage, &self.facts) {
            (Stage::Hull, StageFacts::Hull(h)) => {
                patch("hull input", h.input_control_points, &h.patch_constants)?;
                patch("hull output", h.output_control_points, &[])?;
                let factor = f32::from_bits(h.max_tess_factor_bits);
                if !(1.0..=64.0).contains(&factor) {
                    return Err(format!("a maximum tessellation factor of {factor}"));
                }
                Ok(())
            }
            (Stage::Domain, StageFacts::Domain(d)) => {
                patch("domain", d.input_control_points, &d.patch_constants)
            }
            (Stage::Geometry, StageFacts::Geometry(g)) => {
                if !(1..=1024).contains(&g.max_vertices) || !(1..=32).contains(&g.instances) {
                    return Err(format!(
                        "a geometry program of {} vertices and {} instances",
                        g.max_vertices, g.instances
                    ));
                }
                Ok(())
            }
            (Stage::Vertex, StageFacts::VertexWithoutPosition) => Ok(()),
            (Stage::Hull | Stage::Domain, _) => {
                Err(format!("a {:?} program without its patch facts", self.stage))
            }
            (Stage::Mesh, StageFacts::Mesh(m)) => {
                if !(1..=256).contains(&m.max_vertices) || !(1..=256).contains(&m.max_primitives) {
                    return Err(format!(
                        "a mesh program of {} vertices and {} primitives",
                        m.max_vertices, m.max_primitives
                    ));
                }
                Ok(())
            }
            (Stage::Mesh, _) => Err("a mesh program without its facts".into()),
            (stage, StageFacts::RayTracing(r)) if stage.is_ray_tracing() => {
                if r.export.is_empty() {
                    return Err(format!("a {stage:?} program that names no export"));
                }
                Ok(())
            }
            (_, StageFacts::None) => Ok(()),
            (stage, facts) => Err(format!("{stage:?} program with {facts:?}")),
        }
    }

    /// The resource with this id.
    pub fn resource(&self, id: ResourceId) -> Option<&Resource> {
        self.resources.iter().find(|r| r.id == id)
    }

    /// The interface's own invariants: one varying per label in each direction, values of 1 to 4 lanes, a colour
    /// output `Target(n)` of a pixel stage with `n` of 0 to 7 and no such input, resource ids unique, every resource
    /// spanning at least one binding with a buffer stride (a multiple of 4, up to 2048) or element width that means
    /// something, every constant of a buffer (or of a `tbuffer` view's layout) inside it with a layout that does not
    /// overflow, a hidden counter that belongs to a storage resource and has no binding of its own, a D3D register
    /// class that fits the resource's kind, and no two resources on one register of one class and space.
    pub fn verify(&self) -> Result<(), String> {
        self.verify_facts()?;
        for (direction, varyings) in [("input", &self.inputs), ("output", &self.outputs)] {
            for (i, v) in varyings.iter().enumerate() {
                if let Some(SystemValue::Target(n)) = v.system {
                    if direction == "input" || self.stage != Stage::Pixel {
                        return Err(format!(
                            "{direction} {}{} is colour output {n} of a {:?} stage: only a pixel stage's outputs are",
                            v.label.name, v.label.index, self.stage
                        ));
                    }
                    if n > 7 {
                        return Err(format!(
                            "output {}{} is colour output {n}, not 0 to 7",
                            v.label.name, v.label.index
                        ));
                    }
                }
                if !(1..=4).contains(&v.value.width) {
                    return Err(format!(
                        "{direction} {}{} has a width of {}, not 1 to 4",
                        v.label.name, v.label.index, v.value.width
                    ));
                }
                if varyings[..i].iter().any(|other| other.label == v.label) {
                    return Err(format!(
                        "{direction} {}{} is listed twice",
                        v.label.name, v.label.index
                    ));
                }
            }
        }
        let width_ok = |w: u8| (1..=4).contains(&w);
        for (i, r) in self.resources.iter().enumerate() {
            let name = &r.provenance.name;
            if self.resources[..i].iter().any(|other| other.id == r.id) {
                return Err(format!("resource id {} ({name}) is used twice", r.id.0));
            }
            if r.count == 0 {
                return Err(format!("resource {name} spans no bindings"));
            }
            let buffer = |kind: &BufferKind| match kind {
                BufferKind::Typed(v) if !width_ok(v.width) => {
                    Err(format!("buffer {name} has elements of {} lanes", v.width))
                }
                BufferKind::Structured { stride: 0 } => Err(format!("buffer {name} has a stride of 0")),
                BufferKind::Structured { stride } if !stride.is_multiple_of(4) || *stride > 2048 => Err(
                    format!("buffer {name} has a stride of {stride}, not a multiple of 4 up to 2048"),
                ),
                _ => Ok(()),
            };
            match &r.kind {
                ResourceKind::Buffer(kind)
                | ResourceKind::Storage {
                    target: StorageTarget::Buffer(kind),
                    ..
                } => {
                    buffer(kind)?;
                }
                ResourceKind::ConstantBuffer { layout } => layout_fits(name, layout)?,
                _ => {}
            }
            if let Some(layout) = &r.provenance.layout {
                layout_fits(name, layout)?;
            }
            if let Some(target) = r.provenance.counter_of {
                let owner = self.resource(target).ok_or_else(|| {
                    format!(
                        "resource {name} is the counter of resource {}, which is not there",
                        target.0
                    )
                })?;
                if target == r.id || !matches!(owner.kind, ResourceKind::Storage { .. }) {
                    return Err(format!(
                        "resource {name} is the counter of resource {} ({}), which is not another storage resource",
                        target.0, owner.provenance.name
                    ));
                }
                if r.provenance.d3d.is_some() {
                    return Err(format!(
                        "resource {name} is a hidden counter and has a binding of its own"
                    ));
                }
            }
            if let Some(d3d) = r.provenance.d3d {
                let class = match &r.kind {
                    ResourceKind::ConstantBuffer { .. } => D3dClass::ConstantBuffer,
                    ResourceKind::Texture { .. }
                    | ResourceKind::Buffer(_)
                    | ResourceKind::AccelerationStructure => D3dClass::ShaderResource,
                    ResourceKind::Sampler { .. } => D3dClass::Sampler,
                    ResourceKind::Storage { .. } => D3dClass::UnorderedAccess,
                };
                if d3d.class != class {
                    return Err(format!(
                        "resource {name} is bound as {:?} but its kind is read through {class:?}",
                        d3d.class
                    ));
                }
            }
        }
        // two resources may not share a register: each spans `count` registers (an unbounded array, all the rest)
        let span = |r: &Resource| {
            r.provenance.d3d.map(|d| {
                let first = u64::from(d.register);
                let end = if r.is_unbounded() {
                    u64::MAX
                } else {
                    first + u64::from(r.count)
                };
                (d.class, d.space, first, end)
            })
        };
        for (i, a) in self.resources.iter().enumerate() {
            let Some((class, space, first, end)) = span(a) else {
                continue;
            };
            for b in &self.resources[..i] {
                if let Some((c, s, f, e)) = span(b)
                    && (c, s) == (class, space)
                    && first < e
                    && f < end
                {
                    return Err(format!(
                        "resources {} and {} share a {class:?} register of space {space}",
                        b.provenance.name, a.provenance.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Every constant of `layout` lies inside it, and none has a layout that overflows.
fn layout_fits(name: &str, layout: &BufferLayout) -> Result<(), String> {
    for c in &layout.constants {
        let size =
            c.ty.size()
                .ok_or_else(|| format!("constant {name}.{} overflows", c.name))?;
        if c.offset.checked_add(size).is_none_or(|end| end > layout.size) {
            return Err(format!(
                "constant {name}.{} ({size} bytes at {}) reaches past the end of the {}-byte buffer",
                c.name, c.offset, layout.size
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BufferLayout, Constant, ConstantType, D3dBinding, ResourceProvenance, SampleType, TextureDim,
    };

    fn pixel() -> Interface {
        Interface {
            stage: Stage::Pixel,
            inputs: Vec::new(),
            outputs: Vec::new(),
            resources: Vec::new(),
            facts: Default::default(),
        }
    }
    fn target(index: u32, system: Option<SystemValue>) -> Varying {
        Varying {
            label: Label {
                name: "SV_Target".into(),
                index,
            },
            system,
            value: Value {
                scalar: ScalarType::Float,
                width: 4,
            },
            interp: None,
            used: true,
            provenance: Default::default(),
        }
    }
    fn texture() -> ResourceKind {
        ResourceKind::Texture {
            dim: TextureDim::D2,
            arrayed: false,
            multisampled: false,
            sample: SampleType::Float,
        }
    }
    fn res(
        id: u32,
        name: &str,
        kind: ResourceKind,
        d3d: Option<(D3dClass, u32, u32)>,
        count: u32,
    ) -> Resource {
        Resource {
            id: ResourceId(id),
            kind,
            count,
            provenance: ResourceProvenance {
                name: name.into(),
                d3d: d3d.map(|(class, register, space)| D3dBinding {
                    class,
                    register,
                    space,
                }),
                ..ResourceProvenance::default()
            },
        }
    }
    fn storage() -> ResourceKind {
        ResourceKind::Storage {
            target: StorageTarget::Buffer(BufferKind::Raw),
            access: crate::Access::ReadWrite,
        }
    }
    fn broken(i: &Interface, expect: &str) {
        let message = i.verify().unwrap_err();
        assert!(message.contains(expect), "{expect}: {message}");
    }

    #[test]
    fn a_colour_output_is_a_pixel_outputs_target_of_0_to_7() {
        let mut i = pixel();
        i.outputs = vec![
            target(0, Some(SystemValue::Target(0))),
            target(7, Some(SystemValue::Target(7))),
        ];
        assert_eq!(i.verify(), Ok(()));
        i.outputs.push(target(8, Some(SystemValue::Target(8))));
        broken(&i, "colour output 8, not 0 to 7");
        // not an input, and not an output of another stage
        let mut i = pixel();
        i.inputs = vec![target(0, Some(SystemValue::Target(0)))];
        broken(&i, "only a pixel stage's outputs are");
        let mut i = pixel();
        i.stage = Stage::Vertex;
        i.outputs = vec![target(0, Some(SystemValue::Target(0)))];
        broken(&i, "only a pixel stage's outputs are");
    }

    #[test]
    fn a_structured_buffers_stride_is_a_multiple_of_4_up_to_2048() {
        for (stride, ok) in [
            (4, true),
            (16, true),
            (2048, true),
            (0, false),
            (6, false),
            (2052, false),
        ] {
            let mut i = pixel();
            i.resources = vec![res(
                0,
                "b",
                ResourceKind::Buffer(BufferKind::Structured { stride }),
                Some((D3dClass::ShaderResource, 0, 0)),
                1,
            )];
            assert_eq!(i.verify().is_ok(), ok, "stride {stride}");
        }
    }

    #[test]
    fn a_tbuffers_members_lie_inside_its_layout() {
        let layout = |offset| BufferLayout {
            size: 16,
            constants: vec![Constant {
                name: "m".into(),
                offset,
                ty: ConstantType::Vector(Value {
                    scalar: ScalarType::Float,
                    width: 4,
                }),
                default: None,
            }],
        };
        let mut view = res(
            0,
            "tb",
            ResourceKind::Buffer(BufferKind::Typed(Value {
                scalar: ScalarType::Float,
                width: 4,
            })),
            Some((D3dClass::ShaderResource, 0, 0)),
            1,
        );
        view.provenance.layout = Some(layout(0));
        let mut i = pixel();
        i.resources = vec![view.clone()];
        assert_eq!(i.verify(), Ok(()));
        i.resources[0].provenance.layout = Some(layout(4));
        broken(&i, "reaches past the end");
    }

    #[test]
    fn a_hidden_counter_belongs_to_a_storage_resource_and_has_no_binding() {
        let counter = |of: u32, d3d| {
            let mut c = res(1, "c", ResourceKind::Buffer(BufferKind::Raw), d3d, 1);
            c.provenance.counter_of = Some(ResourceId(of));
            c
        };
        let uav = res(0, "u", storage(), Some((D3dClass::UnorderedAccess, 0, 0)), 1);
        let mut i = pixel();
        i.resources = vec![uav.clone(), counter(0, None)];
        assert_eq!(i.verify(), Ok(()));
        i.resources = vec![uav.clone(), counter(5, None)];
        broken(&i, "which is not there");
        i.resources = vec![uav.clone(), counter(1, None)];
        broken(&i, "not another storage resource");
        // the owner is a texture
        i.resources = vec![
            res(0, "t", texture(), Some((D3dClass::ShaderResource, 0, 0)), 1),
            counter(0, None),
        ];
        broken(&i, "not another storage resource");
        // a counter with a binding of its own
        i.resources = vec![uav, counter(0, Some((D3dClass::ShaderResource, 3, 0)))];
        broken(&i, "binding of its own");
    }

    #[test]
    fn a_register_class_fits_the_kind_and_a_register_holds_one_resource() {
        let cb = ResourceKind::ConstantBuffer {
            layout: BufferLayout {
                size: 16,
                constants: Vec::new(),
            },
        };
        let kinds = [
            (cb.clone(), D3dClass::ConstantBuffer),
            (texture(), D3dClass::ShaderResource),
            (ResourceKind::Buffer(BufferKind::Raw), D3dClass::ShaderResource),
            (ResourceKind::AccelerationStructure, D3dClass::ShaderResource),
            (ResourceKind::Sampler { comparison: false }, D3dClass::Sampler),
            (storage(), D3dClass::UnorderedAccess),
        ];
        for (kind, class) in &kinds {
            let mut i = pixel();
            i.resources = vec![res(0, "r", kind.clone(), Some((*class, 0, 0)), 1)];
            assert_eq!(i.verify(), Ok(()), "{kind:?}");
            // every other class is refused
            for (_, other) in kinds.iter().filter(|(_, c)| c != class) {
                i.resources = vec![res(0, "r", kind.clone(), Some((*other, 0, 0)), 1)];
                broken(&i, "its kind is read through");
            }
        }
        let t = |id, register, space, count| {
            res(
                id,
                &format!("t{id}"),
                texture(),
                Some((D3dClass::ShaderResource, register, space)),
                count,
            )
        };
        let mut i = pixel();
        // the same register in another space, or another class, or the next register, is free
        i.resources = vec![t(0, 2, 0, 1), t(1, 2, 1, 1), t(2, 3, 0, 1)];
        assert_eq!(i.verify(), Ok(()));
        i.resources.push(res(
            3,
            "s",
            ResourceKind::Sampler { comparison: false },
            Some((D3dClass::Sampler, 2, 0)),
            1,
        ));
        assert_eq!(i.verify(), Ok(()));
        // the same register twice, an array over a register, an unbounded array over a later one
        i.resources = vec![t(0, 2, 0, 1), t(1, 2, 0, 1)];
        broken(&i, "share a ShaderResource register of space 0");
        // an array of 4 from register 2 holds registers 2 to 5: register 6 is free, register 5 is not
        i.resources = vec![t(0, 2, 0, 4), t(1, 6, 0, 1)];
        assert_eq!(i.verify(), Ok(()));
        i.resources = vec![t(0, 2, 0, 4), t(1, 6, 0, 1), t(2, 5, 0, 1)];
        broken(&i, "t0 and t2 share");
        i.resources = vec![t(0, 2, 0, Resource::UNBOUNDED), t(1, 90, 0, 1)];
        broken(&i, "share a ShaderResource");
        i.resources = vec![t(0, 2, 0, Resource::UNBOUNDED), t(1, 1, 0, 1), t(2, 2, 1, 1)];
        assert_eq!(i.verify(), Ok(()));
    }

    #[test]
    fn a_varying_serializes_with_its_provenance() {
        let v = Varying {
            label: Label {
                name: "TEXCOORD".into(),
                index: 5,
            },
            system: None,
            value: Value {
                scalar: ScalarType::Float,
                width: 2,
            },
            interp: Some(Interpolation {
                mode: InterpMode::Perspective,
                sampling: InterpSampling::Center,
            }),
            used: true,
            provenance: VaryingProvenance {
                d3d: Some(D3dSignature {
                    register: 3,
                    mask: 0b1100,
                    read_write_mask: 0b1100,
                }),
            },
        };
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({
                "label": {"name": "TEXCOORD", "index": 5},
                "system": null,
                "value": {"scalar": "float", "width": 2},
                "interp": {"mode": "perspective", "sampling": "center"},
                "used": true,
                "provenance": {"d3d": {"register": 3, "mask": 12, "read_write_mask": 12}},
            })
        );
        assert_eq!(
            serde_json::to_value(SystemValue::Target(2)).unwrap(),
            serde_json::json!({"target": 2})
        );
    }
}
