//! A geometry program on the CPU: what the input assembler does before it and what a draw does around it, since WGSL
//! has no geometry stage. [`Topology`] assembles a draw's vertices into the primitives the program reads (with the
//! adjacent vertices of the `*Adjacency` topologies), [`GeometryShader::draw`] runs the program for every primitive and
//! every one of its instances and returns the strips it emitted, and [`Strip::primitives`] assembles a strip's vertices
//! into the points, lines and triangles they make.
//!
//! A vertex is one register per input varying of the program, in the order of its interface (the names are
//! [`GeometryShader::inputs`]); an emitted vertex is one register per output varying ([`GeometryShader::outputs`]).
//! The program is the function-form [`Program`] of [`crate::stage_functions`], run one invocation at a time with
//! [`Executable::function`].
//!
//! The assembly rules are the ones D3D11's input assembler and stream output follow, measured with the D3D11 reference
//! of WARP (`geometry_probe` in the WARP oracle; the draw's vertices' numbers streamed out through a geometry shader
//! that writes each input vertex out): a triangle strip's odd primitives swap their last two vertices, so that every
//! triangle keeps its winding; a triangle strip with adjacency is numbered as [`Topology::TriangleStripAdjacency`]
//! says; `SV_PrimitiveID` counts the primitives of the draw from 0; the instances of a primitive run in order before the
//! next primitive's. A draw with a restart index (an index list holding the value that cuts a strip) is refused by
//! [`GeometryShader::draw_indexed`]: what the assembler makes of the vertices around a cut is not measured.

use std::collections::BTreeMap;

use super::pixel::SlotBackend;
use super::{Bound, Executable, Runner, Scratch};
use crate::{
    BufferKind, D3dClass, FunctionSig, GeometryFacts, GsInput, GsOutput, Program, ResourceId, ResourceKind,
    Sampler, StageFacts, StorageTarget, stage_functions,
};

/// The values of one vertex: a register (four 32-bit lanes) for each varying.
pub type Vertex = Vec<[u32; 4]>;

/// How a draw's vertices make primitives (D3D's primitive topologies a geometry shader can follow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
    LineListAdjacency,
    LineStripAdjacency,
    TriangleListAdjacency,
    TriangleStripAdjacency,
}

impl Topology {
    /// The primitive a geometry program reads to be drawn with this topology.
    pub fn input(self) -> GsInput {
        match self {
            Self::PointList => GsInput::Point,
            Self::LineList | Self::LineStrip => GsInput::Line,
            Self::TriangleList | Self::TriangleStrip => GsInput::Triangle,
            Self::LineListAdjacency | Self::LineStripAdjacency => GsInput::LineAdjacency,
            Self::TriangleListAdjacency | Self::TriangleStripAdjacency => GsInput::TriangleAdjacency,
        }
    }

    /// The primitives of a draw of `vertices` vertices, each as the indices of its vertices in the order the geometry
    /// program reads them (`GsInput::vertices` of them). Vertices that complete no primitive are ignored.
    ///
    /// - A strip's primitive `i` is `[i, i + 1, ...]`, a triangle strip's odd primitive `[i, i + 2, i + 1]` (its last two swapped).
    /// - A line with adjacency is `[before, start, end, after]`.
    /// - A triangle with adjacency is `[a, adjacent to a-b, b, adjacent to b-c, c, adjacent to c-a]`.
    /// - A triangle strip with adjacency is the vertices `0, 2, 4, ...` of a triangle strip, and between them the
    ///   vertices that are adjacent to its edges (an odd vertex `2i + 1` the first's `0-2` edge): for `n = (vertices -
    ///   4) / 2` primitives, primitive `i` (even) is `[2i, 2i - 2 (1 for i = 0), 2i + 2, 2i + 6, 2i + 4, 2i + 3]` and
    ///   primitive `i` (odd) `[2i, 2i + 3, 2i + 4, 2i + 6, 2i + 2, 2i - 2]`, the vertex `2i + 6` that the last
    ///   primitive does not have replaced by `2i + 5`.
    pub fn primitives(self, vertices: usize) -> Vec<Vec<usize>> {
        let list = |size: usize| -> Vec<Vec<usize>> {
            (0..vertices / size)
                .map(|p| (p * size..(p + 1) * size).collect())
                .collect()
        };
        let strip = |size: usize| -> Vec<Vec<usize>> {
            (0..(vertices + 1).saturating_sub(size))
                .map(|p| (p..p + size).collect())
                .collect()
        };
        match self {
            Self::PointList => list(1),
            Self::LineList => list(2),
            Self::LineStrip => strip(2),
            Self::TriangleList => list(3),
            Self::TriangleStrip => strip(3)
                .into_iter()
                .enumerate()
                .map(|(p, mut t)| {
                    if p % 2 == 1 {
                        t.swap(1, 2);
                    }
                    t
                })
                .collect(),
            Self::LineListAdjacency => list(4),
            Self::LineStripAdjacency => strip(4),
            Self::TriangleListAdjacency => list(6),
            Self::TriangleStripAdjacency => {
                let count = vertices.saturating_sub(4) / 2;
                (0..count)
                    .map(|i| {
                        let last = i + 1 == count;
                        let after = if last { 2 * i + 5 } else { 2 * i + 6 };
                        if i % 2 == 0 {
                            vec![
                                2 * i,
                                if i == 0 { 1 } else { 2 * i - 2 },
                                2 * i + 2,
                                after,
                                2 * i + 4,
                                2 * i + 3,
                            ]
                        } else {
                            vec![2 * i, 2 * i + 3, 2 * i + 4, after, 2 * i + 2, 2 * i - 2]
                        }
                    })
                    .collect()
            }
        }
    }
}

impl From<GsOutput> for Topology {
    /// The topology of the strips a program's output makes.
    fn from(output: GsOutput) -> Self {
        match output {
            GsOutput::PointList => Self::PointList,
            GsOutput::LineStrip => Self::LineStrip,
            GsOutput::TriangleStrip => Self::TriangleStrip,
        }
    }
}

/// The vertices an invocation emitted between two cuts, with the primitive and the instance that emitted them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strip {
    pub primitive: u32,
    pub instance: u32,
    pub vertices: Vec<Vertex>,
}

impl Strip {
    /// The primitives the strip makes under the program's output kind, as indices into [`Strip::vertices`]: every
    /// point, every pair of consecutive vertices, every triple (an odd triangle's last two swapped to keep the
    /// winding). A strip with too few vertices makes none.
    pub fn primitives(&self, output: GsOutput) -> Vec<Vec<usize>> {
        Topology::from(output).primitives(self.vertices.len())
    }
}

/// A geometry program ready to run.
#[derive(Debug)]
pub struct GeometryShader {
    program: Program,
    executable: Executable,
    signature: FunctionSig,
    facts: GeometryFacts,
    inputs: usize,
    outputs: usize,
    /// The constant buffers by resource, as (register, space).
    constants: BTreeMap<ResourceId, (u32, u32)>,
    slots: BTreeMap<ResourceId, u32>,
    buffers: BTreeMap<ResourceId, BufferKind>,
}

impl GeometryShader {
    /// Prepare `program`, a geometry program, to run.
    pub fn new(program: Program) -> Result<Self, String> {
        let StageFacts::Geometry(facts) = program.interface.facts else {
            return Err(format!("a {:?} program is not a geometry program", program.stage));
        };
        let signature = stage_functions(&program.interface)?
            .and_then(|mut f| f.pop())
            .ok_or("a geometry program without its function")?;
        let executable = Executable::function(&program, "main").map_err(|e| e.to_string())?;
        let (inputs, outputs) = (program.interface.inputs.len(), program.interface.outputs.len());
        let mut constants = BTreeMap::new();
        let mut slots = BTreeMap::new();
        let mut buffers = BTreeMap::new();
        for r in &program.interface.resources {
            let Some(binding) = r.provenance.d3d else {
                return Err(format!("resource {} has no D3D bind slot", r.provenance.name));
            };
            slots.insert(r.id, binding.register);
            if matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                && binding.class == D3dClass::ConstantBuffer
            {
                constants.insert(r.id, (binding.register, binding.space));
            }
            if let ResourceKind::Buffer(kind)
            | ResourceKind::Storage {
                target: StorageTarget::Buffer(kind),
                ..
            } = &r.kind
            {
                buffers.insert(r.id, *kind);
            }
        }
        Ok(Self {
            program,
            executable,
            signature,
            facts,
            inputs,
            outputs,
            constants,
            slots,
            buffers,
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// What the program states: the primitive it reads, what it emits, the most vertices an invocation emits and its
    /// number of instances.
    pub fn facts(&self) -> GeometryFacts {
        self.facts
    }

    /// The names of the input varyings: the registers of a [`Vertex`] given to the program, in order.
    pub fn inputs(&self) -> impl Iterator<Item = &str> {
        self.signature.arguments[..self.inputs]
            .iter()
            .map(|a| a.name.as_str())
    }

    /// The names of the output varyings: the registers of the vertices of a [`Strip`], in order.
    pub fn outputs(&self) -> impl Iterator<Item = &str> {
        self.signature.result[..self.outputs]
            .iter()
            .map(|a| a.name.as_str())
    }

    /// Run the program for the primitive of the draw numbered `primitive` whose vertices are `vertices` (as many as
    /// the program's input primitive has, in the order [`Topology::primitives`] gives), as instance `instance`, and
    /// return the strips it emitted. `constants` holds the rows of the constant buffers by (register, space); image
    /// and buffer access go to `sampler` by bind slot.
    pub fn run(
        &self,
        constants: &BTreeMap<(u32, u32), Vec<[u32; 4]>>,
        sampler: &mut dyn Sampler,
        vertices: &[&Vertex],
        primitive: u32,
        instance: u32,
    ) -> Result<Vec<Strip>, String> {
        let bound = self.bind(constants)?;
        let mut backend = self.backend(sampler);
        self.invoke(
            &bound,
            &mut backend,
            Scratch::new(&self.executable),
            vertices,
            primitive,
            instance,
        )
        .map(|(strips, _)| strips)
    }

    /// Draw `vertices` (the output of a vertex program) as `topology`: assemble the primitives, and run the program for
    /// each of them and each of its instances. The strips are in the order of the primitives, the instances of a
    /// primitive in order, then the strips an instance cut. The topology must be the kind of primitive the program
    /// reads.
    pub fn draw(
        &self,
        constants: &BTreeMap<(u32, u32), Vec<[u32; 4]>>,
        sampler: &mut dyn Sampler,
        vertices: &[Vertex],
        topology: Topology,
    ) -> Result<Vec<Strip>, String> {
        if topology.input() != self.facts.input {
            return Err(format!(
                "a draw of {topology:?} feeds the geometry program primitives of {:?}, which reads {:?}",
                topology.input(),
                self.facts.input
            ));
        }
        let bound = self.bind(constants)?;
        let mut backend = self.backend(sampler);
        let mut scratch = Scratch::new(&self.executable);
        let mut strips = Vec::new();
        for (primitive, indices) in topology.primitives(vertices.len()).iter().enumerate() {
            let primitive_vertices: Vec<&Vertex> = indices.iter().map(|&i| &vertices[i]).collect();
            for instance in 0..self.facts.instances.max(1) {
                let (emitted, kept) = self.invoke(
                    &bound,
                    &mut backend,
                    scratch,
                    &primitive_vertices,
                    u32::try_from(primitive).map_err(|e| e.to_string())?,
                    instance,
                )?;
                scratch = kept;
                strips.extend(emitted);
            }
        }
        Ok(strips)
    }

    /// [`GeometryShader::draw`] of an indexed draw: the vertices at `indices`, in order. When the draw has a restart
    /// index (`restart`, the value that cuts a strip: 0xFFFF or 0xFFFFFFFF in D3D), an index list that holds it is
    /// refused: the primitives around a cut are not assembled.
    pub fn draw_indexed(
        &self,
        constants: &BTreeMap<(u32, u32), Vec<[u32; 4]>>,
        sampler: &mut dyn Sampler,
        vertices: &[Vertex],
        indices: &[u32],
        restart: Option<u32>,
        topology: Topology,
    ) -> Result<Vec<Strip>, String> {
        if let Some(cut) = restart
            && let Some(at) = indices.iter().position(|&i| i == cut)
        {
            return Err(format!(
                "index {at} is the restart index {cut:#x}: a draw with a strip restart is not assembled (the                  primitives around a cut are not measured on the D3D reference)"
            ));
        }
        let drawn = indices
            .iter()
            .map(|&i| {
                vertices
                    .get(i as usize)
                    .cloned()
                    .ok_or_else(|| format!("index {i} names a vertex past the {} given", vertices.len()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.draw(constants, sampler, &drawn, topology)
    }

    fn bind<'a>(&self, constants: &'a BTreeMap<(u32, u32), Vec<[u32; 4]>>) -> Result<Bound<'a>, String> {
        self.executable.bind(
            |id| {
                let key = self.constants.get(&id)?;
                constants.get(key).map(Vec::as_slice)
            },
            |_| None,
        )
    }

    fn backend<'a>(&'a self, sampler: &'a mut dyn Sampler) -> SlotBackend<'a> {
        SlotBackend {
            sampler,
            slots: &self.slots,
            buffers: &self.buffers,
        }
    }

    /// One invocation: the registers of the arguments (each input varying for each vertex of the primitive, then the
    /// primitive and the instance), and the emitted vertices split into strips at the flags the program sets.
    fn invoke(
        &self,
        bound: &Bound<'_>,
        backend: &mut SlotBackend<'_>,
        scratch: Scratch<1>,
        vertices: &[&Vertex],
        primitive: u32,
        instance: u32,
    ) -> Result<(Vec<Strip>, Scratch<1>), String> {
        let size = self.facts.input.vertices() as usize;
        if vertices.len() != size {
            return Err(format!(
                "{} vertices given for a primitive of {:?}, which has {size}",
                vertices.len(),
                self.facts.input
            ));
        }
        let mut arguments = Vec::with_capacity(self.inputs * size + 2);
        for varying in 0..self.inputs {
            for (k, vertex) in vertices.iter().enumerate() {
                arguments.push(*vertex.get(varying).ok_or_else(|| {
                    format!(
                        "vertex {k} has {} registers, the program reads {} ({})",
                        vertex.len(),
                        self.inputs,
                        self.inputs().collect::<Vec<_>>().join(", ")
                    )
                })?);
            }
        }
        arguments.push([primitive, 0, 0, 0]);
        arguments.push([instance, 0, 0, 0]);
        let mut runner = Runner::<1>::with_scratch(&self.executable, bound, backend, None, scratch);
        let [out] = runner.run([&arguments])?;
        let scratch = runner.into_scratch();
        let max = self.facts.max_vertices as usize;
        let count_at = self.outputs * max;
        let want = count_at + 1 + max;
        if out.values.len() != want {
            return Err(format!(
                "the program returned {} values, its signature has {want}",
                out.values.len()
            ));
        }
        let count = out.values[count_at][0] as usize;
        if count > max {
            return Err(format!(
                "the invocation emitted {count} vertices, past the {max} it declares"
            ));
        }
        let mut strips: Vec<Strip> = Vec::new();
        for k in 0..count {
            if k == 0 || out.values[count_at + 1 + k][0] != 0 {
                strips.push(Strip {
                    primitive,
                    instance,
                    vertices: Vec::new(),
                });
            }
            let vertex = (0..self.outputs).map(|o| out.values[o * max + k]).collect();
            if let Some(strip) = strips.last_mut() {
                strip.vertices.push(vertex);
            }
        }
        Ok((strips, scratch))
    }
}
