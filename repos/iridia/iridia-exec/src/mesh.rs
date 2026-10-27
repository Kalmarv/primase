//! A mesh program on the CPU: a thread group that writes one meshlet. [`MeshShader::draw`] runs the thread groups of a
//! dispatch one after another and returns the meshlet each wrote: its vertices (one register per output varying, as a
//! geometry program's emitted vertices are, [`MeshShader::outputs`] names them) and its primitives (the indices of their
//! vertices). What the rasterizer does with them is the pipeline's.
//!
//! The program is the entry point of [`iridia::build::Builder::finish_mesh`]: the group's invocations run in lockstep
//! like a compute program's, and what they write is the module's output variable, thread-group memory that is read when
//! the group has finished. The counts the program set are the group's (D3D requires every invocation to set the same),
//! and a count past the most the program declares, or an index past the vertices it counted, is an error of the run:
//! D3D leaves both undefined.

use super::geometry::Vertex;
use super::stage::{Bindings, Constants};
use super::{Executable, error};
use crate::{Error, Sampler};
use iridia::{IoType, MeshFacts, MeshTopology, Program, StageFacts, mesh_vertex_io};

/// What one thread group wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meshlet {
    /// The thread group's position in the dispatch.
    pub group: [u32; 3],
    /// The vertices the program counted, one register per output varying.
    pub vertices: Vec<Vertex>,
    /// The primitives the program counted, each as the indices of its vertices (three for a triangle, two for a line).
    pub primitives: Vec<Vec<u32>>,
}

/// A mesh program ready to run.
#[derive(Debug)]
pub struct MeshShader {
    program: Program,
    executable: Executable,
    facts: MeshFacts,
    /// The output varyings: their names and widths, in the order of the registers of a vertex.
    outputs: Vec<(String, usize)>,
    bindings: Bindings,
}

impl MeshShader {
    /// Prepare `program`, a mesh program, to run.
    pub fn new(program: Program) -> Result<Self, Error> {
        let StageFacts::Mesh(facts) = program.interface.facts else {
            return Err(error(format!(
                "a {:?} program is not a mesh program",
                program.stage
            )));
        };
        let outputs = mesh_vertex_io(&program.interface)
            .map_err(error)?
            .into_iter()
            .map(|item| match item.ty {
                IoType::Value(v) => Ok((item.name, usize::from(v.width))),
                _ => Err(error(format!(
                    "{}: a vertex output that is not a value",
                    item.name
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if outputs.len() != program.interface.outputs.len() {
            return Err(error(
                "a mesh program whose clip or cull distances are carried together",
            ));
        }
        let executable = Executable::new(&program)?;
        let (_, words) = executable
            .mesh_output
            .ok_or_else(|| error("a mesh program without its output variable"))?;
        let shader = Self {
            bindings: Bindings::new(&program).map_err(error)?,
            program,
            executable,
            facts,
            outputs,
        };
        let layout = shader.layout();
        if words as usize != layout.total {
            return Err(error(format!(
                "the output variable has {words} words, its vertices and primitives make {}",
                layout.total
            )));
        }
        Ok(shader)
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// What the program states: the kind of primitive it writes and the most vertices and primitives a group writes.
    pub fn facts(&self) -> MeshFacts {
        self.facts
    }

    /// The thread group's invocations along each axis.
    pub fn group_size(&self) -> [u32; 3] {
        self.executable.workgroup.unwrap_or([1, 1, 1])
    }

    /// The names of the output varyings: the registers of the vertices of a [`Meshlet`], in order.
    pub fn outputs(&self) -> impl Iterator<Item = &str> {
        self.outputs.iter().map(|(name, _)| name.as_str())
    }

    /// Where the parts of the output variable lie, in the executor's packed words.
    fn layout(&self) -> Layout {
        let vertex: usize = self.outputs.iter().map(|(_, width)| width).sum();
        let index = match self.facts.topology {
            MeshTopology::Triangle => 3,
            MeshTopology::Line => 2,
        };
        let (max_vertices, max_primitives) = (
            self.facts.max_vertices as usize,
            self.facts.max_primitives as usize,
        );
        Layout {
            vertex,
            index,
            vertices_at: 2,
            primitives_at: 2 + max_vertices * vertex,
            total: 2 + max_vertices * vertex + max_primitives * index,
        }
    }

    /// Run `groups` thread groups (`wave` the wave width the wave operations report) and return the meshlet each wrote,
    /// in the order of the dispatch (x, then y, then z). `constants` holds the rows of the constant buffers by
    /// (register, space); image and buffer access go to `sampler` by bind slot.
    pub fn draw(
        &self,
        constants: &Constants,
        sampler: &mut dyn Sampler,
        groups: [u32; 3],
        wave: u32,
    ) -> Result<Vec<Meshlet>, Error> {
        let bound = self.bindings.bind(&self.executable, constants).map_err(error)?;
        let mut backend = self.bindings.backend(sampler);
        let layout = self.layout();
        let mut meshlets = Vec::new();
        self.executable
            .dispatch_mesh(&bound, &mut backend, groups, wave, &mut |group, words| {
                meshlets.push(self.read(&layout, group, words)?);
                Ok(())
            })
            .map_err(error)?;
        Ok(meshlets)
    }

    /// The meshlet in the words of the output variable of a finished group.
    fn read(&self, layout: &Layout, group: [u32; 3], words: &[u32]) -> Result<Meshlet, String> {
        if words.len() != layout.total {
            return Err(format!(
                "{} words of output, expected {}",
                words.len(),
                layout.total
            ));
        }
        let (vertex_count, primitive_count) = (words[0] as usize, words[1] as usize);
        if vertex_count > self.facts.max_vertices as usize {
            return Err(format!(
                "the group set {vertex_count} vertices, past the {} it declares",
                self.facts.max_vertices
            ));
        }
        if primitive_count > self.facts.max_primitives as usize {
            return Err(format!(
                "the group set {primitive_count} primitives, past the {} it declares",
                self.facts.max_primitives
            ));
        }
        let vertices = (0..vertex_count)
            .map(|k| {
                let mut at = layout.vertices_at + k * layout.vertex;
                self.outputs
                    .iter()
                    .map(|&(_, width)| {
                        let mut register = [0u32; 4];
                        register[..width].copy_from_slice(&words[at..at + width]);
                        at += width;
                        register
                    })
                    .collect()
            })
            .collect();
        let mut primitives = Vec::with_capacity(primitive_count);
        for k in 0..primitive_count {
            let at = layout.primitives_at + k * layout.index;
            let indices = words[at..at + layout.index].to_vec();
            if let Some(bad) = indices.iter().find(|&&i| i as usize >= vertex_count) {
                return Err(format!(
                    "primitive {k} names vertex {bad}, past the {vertex_count} the group set"
                ));
            }
            primitives.push(indices);
        }
        Ok(Meshlet {
            group,
            vertices,
            primitives,
        })
    }
}

/// The output variable's words: its two counts, then the vertices and the primitives, each packed.
struct Layout {
    /// Words of a vertex.
    vertex: usize,
    /// Words of a primitive.
    index: usize,
    vertices_at: usize,
    primitives_at: usize,
    total: usize,
}
