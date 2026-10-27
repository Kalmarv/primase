//! A tessellated patch on the CPU: the hull program for each output control point and for the patch's constants, the
//! fixed-function tessellator ([`super::tessellator`]) over the factors the hull program wrote, and the domain program
//! for each point it made. WGSL has no tessellation stages, so this is how the lab draws a patch.
//!
//! The programs are the function-form [`Program`]s of [`iridia::stage_functions`], run one invocation at a time with
//! [`Executable::function`]. A control point (the vertex program's output, the hull program's input) is a
//! [`Vertex`](super::geometry::Vertex): one register per input varying of the hull program, in the order of its
//! interface ([`TessellationShader::inputs`]). The hull program's outputs reach the domain program by label (the
//! control points' varyings and the patch constants), so the two programs need not number them alike.

use super::geometry::Vertex;
use super::pixel::SlotBackend;
use super::stage::{Bindings, Constants};
use super::tessellator::{
    Factors, OutputPrimitive, Tessellation, tessellate_isolines, tessellate_quad, tessellate_triangle,
};
use super::{Bound, Executable, Runner, Scratch};
use crate::Sampler;
use iridia::{
    DomainFacts, FunctionSig, HullFacts, Program, StageFacts, TessDomain, TessTopology, stage_functions,
};

/// What the tessellator made of a patch, with the programs' results.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// The hull program's output control points: one register per output varying ([`TessellationShader::control_outputs`]).
    pub control_points: Vec<Vertex>,
    /// The tessellation factors the hull program wrote.
    pub factors: Factors,
    /// The domain program's result for each point the tessellator made: one register per output varying of the domain
    /// program ([`TessellationShader::outputs`]).
    pub vertices: Vec<Vertex>,
    /// The domain locations of the points (u, v; a triangle's third coordinate is `1 - u - v`).
    pub locations: Vec<[f32; 2]>,
    /// What `indices` make: points, lines or triangles (clockwise or counter-clockwise in the domain, as the hull
    /// program says).
    pub topology: TessTopology,
    /// Point numbers, one per point of a point list, two per line, three per triangle.
    pub indices: Vec<u32>,
}

/// A hull program and the domain program it feeds, ready to run.
#[derive(Debug)]
pub struct TessellationShader {
    hull: Program,
    hull_bindings: Bindings,
    control: Executable,
    control_sig: FunctionSig,
    constant: Executable,
    hull_facts: HullFacts,
    domain: Program,
    domain_bindings: Bindings,
    domain_exe: Executable,
    domain_sig: FunctionSig,
    domain_facts: DomainFacts,
    /// The patch constants (hull results of the constant phase) that are the edge factors, then the inside factors.
    edge_factors: Vec<usize>,
    inside_factors: Vec<usize>,
    /// For each patch constant the domain program reads, the hull result it is.
    constant_map: Vec<usize>,
    /// For each input varying of the domain program, the output varying of the hull program it is.
    control_map: Vec<usize>,
}

fn find(sig: &FunctionSig, name: &str) -> Option<usize> {
    sig.result.iter().position(|r| r.name == name)
}

impl TessellationShader {
    /// Prepare `hull` and the `domain` program it feeds. The programs must agree on the domain and on the number of
    /// control points between them, and the domain program's inputs must be varyings the hull program writes.
    pub fn new(hull: Program, domain: Program) -> Result<Self, String> {
        let StageFacts::Hull(hull_facts) = hull.interface.facts.clone() else {
            return Err(format!("a {:?} program is not a hull program", hull.stage));
        };
        let StageFacts::Domain(domain_facts) = domain.interface.facts.clone() else {
            return Err(format!("a {:?} program is not a domain program", domain.stage));
        };
        if hull_facts.domain != domain_facts.domain {
            return Err(format!(
                "the hull program tessellates a {:?} domain, the domain program is for a {:?} one",
                hull_facts.domain, domain_facts.domain
            ));
        }
        if hull_facts.output_control_points != domain_facts.input_control_points {
            return Err(format!(
                "the hull program writes {} control points, the domain program reads {}",
                hull_facts.output_control_points, domain_facts.input_control_points
            ));
        }
        let mut hull_sigs =
            stage_functions(&hull.interface)?.ok_or("a hull program without its functions")?;
        let constant_sig = hull_sigs
            .iter()
            .position(|s| s.name == "patch_constant")
            .map(|i| hull_sigs.swap_remove(i))
            .ok_or("a hull program without its patch-constant function")?;
        let control_sig = hull_sigs
            .into_iter()
            .find(|s| s.name == "main")
            .ok_or("a hull program without its control-point function")?;
        let domain_sig = stage_functions(&domain.interface)?
            .and_then(|mut f| f.pop())
            .ok_or("a domain program without its function")?;
        let control = Executable::function(&hull, "main").map_err(|e| e.to_string())?;
        let constant = Executable::function(&hull, "patch_constant").map_err(|e| e.to_string())?;
        let domain_exe = Executable::function(&domain, "main").map_err(|e| e.to_string())?;

        let (edges, insides) = match hull_facts.domain {
            TessDomain::Triangle => (3, 1),
            TessDomain::Quad => (4, 2),
            TessDomain::Isoline => (2, 0),
        };
        let factor = |name: &str, k: usize| {
            let name = format!("{name}{k}");
            find(&constant_sig, &name)
                .ok_or_else(|| format!("the hull program's patch constants have no {name}"))
        };
        let edge_factors = (0..edges)
            .map(|k| factor("SV_TessFactor", k))
            .collect::<Result<Vec<_>, _>>()?;
        let inside_factors = (0..insides)
            .map(|k| factor("SV_InsideTessFactor", k))
            .collect::<Result<Vec<_>, _>>()?;

        // the domain program's arguments: its patch constants, then its input varyings as arrays of control points
        let constants_read = domain_facts.patch_constants.len();
        let constant_map = domain_sig.arguments[..constants_read]
            .iter()
            .map(|a| {
                find(&constant_sig, &a.name).ok_or_else(|| {
                    format!(
                        "the domain program reads the patch constant {} the hull program does not write",
                        a.name
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let control_map = domain_sig.arguments[constants_read..constants_read + domain.interface.inputs.len()]
            .iter()
            .map(|a| {
                find(&control_sig, &a.name).ok_or_else(|| {
                    format!("the domain program reads the control-point varying {} the hull program does not write", a.name)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            hull_bindings: Bindings::new(&hull)?,
            domain_bindings: Bindings::new(&domain)?,
            hull,
            control,
            control_sig,
            constant,
            hull_facts,
            domain,
            domain_exe,
            domain_sig,
            domain_facts,
            edge_factors,
            inside_factors,
            constant_map,
            control_map,
        })
    }

    pub fn hull(&self) -> &Program {
        &self.hull
    }

    pub fn domain(&self) -> &Program {
        &self.domain
    }

    /// What the hull program states: the domain, the partitioning, the output, the control point counts.
    pub fn hull_facts(&self) -> &HullFacts {
        &self.hull_facts
    }

    /// What the domain program states.
    pub fn domain_facts(&self) -> &DomainFacts {
        &self.domain_facts
    }

    /// The names of the hull program's input varyings: the registers of the control points given to
    /// [`TessellationShader::tessellate`], in order.
    pub fn inputs(&self) -> impl Iterator<Item = &str> {
        let n = self.hull.interface.inputs.len();
        self.control_sig.arguments[..n].iter().map(|a| a.name.as_str())
    }

    /// The names of the hull program's output varyings: the registers of [`Patch::control_points`].
    pub fn control_outputs(&self) -> impl Iterator<Item = &str> {
        let n = self.hull.interface.outputs.len();
        self.control_sig.result[..n].iter().map(|a| a.name.as_str())
    }

    /// The names of the domain program's output varyings: the registers of [`Patch::vertices`].
    pub fn outputs(&self) -> impl Iterator<Item = &str> {
        self.domain_sig.result.iter().map(|a| a.name.as_str())
    }

    /// Tessellate the patch of `control_points` (as many as the hull program reads), the patch numbered `primitive`
    /// of the draw. `hull_constants` and `domain_constants` hold the rows of the constant buffers each program reads,
    /// by (register, space); image and buffer access of both go to `sampler` by bind slot.
    pub fn tessellate(
        &self,
        hull_constants: &Constants,
        domain_constants: &Constants,
        sampler: &mut dyn Sampler,
        control_points: &[Vertex],
        primitive: u32,
    ) -> Result<Patch, String> {
        let input_count = self.hull_facts.input_control_points as usize;
        if control_points.len() != input_count {
            return Err(format!(
                "{} control points given, the hull program reads {input_count}",
                control_points.len()
            ));
        }
        let inputs = self.hull.interface.inputs.len();
        let mut inputs_as_arrays = Vec::with_capacity(inputs * input_count);
        for varying in 0..inputs {
            for (k, point) in control_points.iter().enumerate() {
                inputs_as_arrays.push(*point.get(varying).ok_or_else(|| {
                    format!(
                        "control point {k} has {} registers, the hull program reads {inputs} ({})",
                        point.len(),
                        self.inputs().collect::<Vec<_>>().join(", ")
                    )
                })?);
            }
        }

        // the hull program: each output control point, then the patch constants
        let hull_bound = self.hull_bindings.bind(&self.control, hull_constants)?;
        let constant_bound = self.hull_bindings.bind(&self.constant, hull_constants)?;
        let mut hull_backend = self.hull_bindings.backend(sampler);
        let output_count = self.hull_facts.output_control_points as usize;
        let outputs = self.hull.interface.outputs.len();
        let mut control_outputs: Vec<Vertex> = Vec::with_capacity(output_count);
        let mut scratch = Scratch::new(&self.control);
        for id in 0..output_count {
            let mut arguments = inputs_as_arrays.clone();
            arguments.push([id as u32, 0, 0, 0]);
            let (values, kept) = call(&self.control, &hull_bound, &mut hull_backend, scratch, &arguments)?;
            scratch = kept;
            control_outputs.push(values[..outputs].to_vec());
        }
        let mut arguments = inputs_as_arrays;
        for varying in 0..outputs {
            for point in &control_outputs {
                arguments.push(point[varying]);
            }
        }
        arguments.push([primitive, 0, 0, 0]);
        let (constants, _) = call(
            &self.constant,
            &constant_bound,
            &mut hull_backend,
            Scratch::new(&self.constant),
            &arguments,
        )?;
        let float = |k: usize| f32::from_bits(constants[k][0]);
        let mut factors = Factors {
            edges: [0.0; 4],
            inside: [0.0; 2],
        };
        for (slot, &k) in self.edge_factors.iter().enumerate() {
            factors.edges[slot] = float(k);
        }
        for (slot, &k) in self.inside_factors.iter().enumerate() {
            factors.inside[slot] = float(k);
        }

        // the tessellator
        let partitioning = self.hull_facts.partitioning;
        let output = match self.hull_facts.topology {
            TessTopology::Point => OutputPrimitive::Point,
            TessTopology::Line => OutputPrimitive::Line,
            TessTopology::TriangleCw => OutputPrimitive::TriangleCw,
            TessTopology::TriangleCcw => OutputPrimitive::TriangleCcw,
        };
        let Tessellation { points, indices } = match self.hull_facts.domain {
            TessDomain::Triangle => tessellate_triangle(partitioning, output, factors),
            TessDomain::Quad => tessellate_quad(partitioning, output, factors),
            TessDomain::Isoline => {
                tessellate_isolines(partitioning, output, factors.edges[0], factors.edges[1])
            }
        };

        // the domain program for each point
        let domain_bound = self.domain_bindings.bind(&self.domain_exe, domain_constants)?;
        let mut domain_backend = self.domain_bindings.backend(sampler);
        let mut fixed: Vec<[u32; 4]> = self.constant_map.iter().map(|&k| constants[k]).collect();
        for varying in &self.control_map {
            for point in &control_outputs {
                fixed.push(point[*varying]);
            }
        }
        let triangle = self.domain_facts.domain == TessDomain::Triangle;
        let mut vertices = Vec::with_capacity(points.len());
        let mut scratch = Scratch::new(&self.domain_exe);
        for &[u, v] in &points {
            let mut arguments = fixed.clone();
            let w = if triangle { 1.0 - u - v } else { 0.0 };
            arguments.push([u.to_bits(), v.to_bits(), w.to_bits(), 0]);
            arguments.push([primitive, 0, 0, 0]);
            let (values, kept) = call(
                &self.domain_exe,
                &domain_bound,
                &mut domain_backend,
                scratch,
                &arguments,
            )?;
            scratch = kept;
            vertices.push(values);
        }
        Ok(Patch {
            control_points: control_outputs,
            factors,
            vertices,
            locations: points,
            topology: self.hull_facts.topology,
            indices,
        })
    }
}

/// Run `executable` once on `arguments`, returning its result's registers and the registers kept for the next run.
fn call(
    executable: &Executable,
    bound: &Bound<'_>,
    backend: &mut SlotBackend<'_>,
    scratch: Scratch<1>,
    arguments: &[[u32; 4]],
) -> Result<(Vec<[u32; 4]>, Scratch<1>), String> {
    let mut runner = Runner::<1>::with_scratch(executable, bound, backend, None, scratch);
    let [out] = runner.run([arguments])?;
    Ok((out.values, runner.into_scratch()))
}
