//! A [`Program`] lifted from D3D bytecode behind [`PixelShader`]: inputs and outputs by signature register, constant
//! buffers and samplers by bind slot, as every bake already drives a shader. A vertex program has the same register view
//! (its inputs the vertex attributes by register, its outputs the varyings by register; the position and clip distances,
//! which have no output register, are not in it), so a caller can run one per vertex through [`PixelShader::execute`].

use std::collections::BTreeMap;

use super::{
    Backend, Element, Executable, ImageAtomic, ImageLoad, ImageQuery, ImageSample, ImageStore, Runner,
    Scratch, error,
};
use crate::{AtomicAdd, Error, Execution, PixelShader, Sampler, Store};
use primase_shader::{
    BufferKind, D3dClass, Interface, Program, ResourceId, ResourceKind, Stage, StorageTarget, SystemValue,
    TextureDim,
};
use std::sync::Mutex;

/// One entry argument and where its lanes come from in the register file.
#[derive(Debug)]
struct Argument {
    register: usize,
    /// The register lanes it takes, in order.
    lanes: Vec<u8>,
    /// A front-facing flag: the register word is non-zero for a front face.
    boolean: bool,
    used: bool,
}

/// One member of the result and the register lanes it is written to.
#[derive(Debug)]
struct Member {
    register: usize,
    lanes: Vec<u8>,
    /// The lanes the program writes, as its signature states.
    written: u8,
}

/// A pixel program with the register view the D3D readers' callers use.
#[derive(Debug)]
pub struct ProgramShader {
    program: Program,
    executable: Executable,
    arguments: Vec<Argument>,
    members: Vec<Member>,
    input_count: usize,
    output_count: usize,
    /// Each constant buffer: bind slot, resource, rows.
    constants: Vec<(u32, ResourceId, u32)>,
    constant_buffers: Vec<(u32, u32)>,
    textures: Vec<u32>,
    samplers: Vec<u32>,
    /// The bind slot of every resource that has one.
    slots: BTreeMap<ResourceId, u32>,
    buffers: BTreeMap<ResourceId, BufferKind>,
    /// Registers kept between runs, one set per concurrent run.
    scalar_scratch: Mutex<Vec<Scratch<1>>>,
    quad_scratch: Mutex<Vec<Scratch<4>>>,
    lane_scratch: Mutex<Vec<Scratch<LANES>>>,
}

/// The invocations [`PixelShader::execute_lanes`] runs in lockstep at a time.
const LANES: usize = 16;

fn lanes_of(mask: u8) -> Vec<u8> {
    (0..4).filter(|l| mask >> l & 1 != 0).collect()
}

/// The lanes the module carries for an input: the pipeline fixes some, the interface states the rest.
fn width_of(v: &primase_shader::Varying) -> usize {
    match v.system {
        Some(SystemValue::FragCoord) => 4,
        Some(
            SystemValue::FrontFacing
            | SystemValue::VertexIndex
            | SystemValue::InstanceIndex
            | SystemValue::PrimitiveIndex
            | SystemValue::SampleIndex
            | SystemValue::Coverage,
        ) => 1,
        _ => v.value.width as usize,
    }
}

impl ProgramShader {
    /// Compile `program`. Its inputs and outputs need their D3D registers (the lifter keeps them in provenance),
    /// and every resource a D3D bind slot.
    pub fn new(program: Program) -> Result<Self, Error> {
        if !matches!(program.stage, Stage::Pixel | Stage::Vertex) {
            return Err(error("not a pixel or vertex program"));
        }
        let executable = Executable::new(&program)?;
        let interface = &program.interface;
        let mut arguments = Vec::new();
        let mut input_count = 0;
        // the module's arguments: the interface's inputs the entry point carries, in order
        let io = primase_shader::entry_io(interface).map_err(error)?;
        for item in &io.arguments {
            let v = &interface.inputs[item.varyings[0]];
            let name = Program::varying_name(&v.label);
            // the primitive and coverage inputs have no register in the signature: the register view cannot carry
            // them, they read as 0
            if v.provenance.d3d.is_none()
                && matches!(
                    v.system,
                    Some(SystemValue::PrimitiveIndex | SystemValue::Coverage | SystemValue::SampleIndex)
                )
            {
                arguments.push(Argument {
                    register: usize::MAX,
                    lanes: Vec::new(),
                    boolean: false,
                    used: false,
                });
                continue;
            }
            let sig = v
                .provenance
                .d3d
                .ok_or_else(|| error(format!("input {name} has no D3D register")))?;
            let lanes = lanes_of(sig.mask);
            let width = width_of(v);
            if lanes.len() < width {
                return Err(error(format!(
                    "input {name} has {width} lanes but its register mask {:#x} marks {}",
                    sig.mask,
                    lanes.len()
                )));
            }
            if v.used {
                input_count = input_count.max(sig.register as usize + 1);
            }
            arguments.push(Argument {
                register: sig.register as usize,
                lanes: lanes[..width].to_vec(),
                boolean: v.system == Some(SystemValue::FrontFacing),
                used: v.used,
            });
        }
        let mut members = Vec::new();
        let mut output_count = 0;
        for v in &interface.outputs {
            let name = Program::varying_name(&v.label);
            // depth, coverage and stencil have no output register
            if !matches!(v.system, None | Some(SystemValue::Target(_))) {
                members.push(Member {
                    register: usize::MAX,
                    lanes: Vec::new(),
                    written: 0,
                });
                continue;
            }
            let sig = v
                .provenance
                .d3d
                .ok_or_else(|| error(format!("output {name} has no D3D register")))?;
            output_count = output_count.max(sig.register as usize + 1);
            members.push(Member {
                register: sig.register as usize,
                lanes: lanes_of(sig.mask),
                // an element the program does not write (the signature lists it, `used` is false) has no lane written
                written: if v.used {
                    sig.mask & !sig.read_write_mask
                } else {
                    0
                },
            });
        }
        let mut constants = Vec::new();
        let (mut textures, mut samplers) = (Vec::new(), Vec::new());
        // the textures and samplers the compiled code reaches, not every one the program declares
        let (sampled_textures, sampled_samplers) = executable.sampled();
        let mut slots = BTreeMap::new();
        let mut buffers = BTreeMap::new();
        for r in &interface.resources {
            let binding = r
                .provenance
                .d3d
                .ok_or_else(|| error(format!("resource {} has no D3D bind slot", r.provenance.name)))?;
            slots.insert(r.id, binding.register);
            match &r.kind {
                ResourceKind::ConstantBuffer { .. } => {
                    let rows = rows_of(&program, r.id).ok_or_else(|| {
                        error(format!(
                            "constant buffer {} has no module array",
                            r.provenance.name
                        ))
                    })?;
                    constants.push((binding.register, r.id, rows));
                }
                // the 2D textures (and arrays of them) the code reaches: what `Sampler::sample` answers, and what the
                // evaluators' `textures()` always listed; a volume or a cube is reached through `sample_image`
                ResourceKind::Texture {
                    dim: TextureDim::D2, ..
                } if binding.class == D3dClass::ShaderResource && sampled_textures.contains(&r.id) => {
                    textures.push(binding.register);
                }
                ResourceKind::Sampler { .. } if sampled_samplers.contains(&r.id) => {
                    samplers.push(binding.register);
                }
                ResourceKind::Buffer(kind)
                | ResourceKind::Storage {
                    target: StorageTarget::Buffer(kind),
                    ..
                } => {
                    buffers.insert(r.id, *kind);
                }
                _ => {}
            }
        }
        textures.sort_unstable();
        textures.dedup();
        samplers.sort_unstable();
        samplers.dedup();
        let constant_buffers = constants.iter().map(|&(slot, _, rows)| (slot, rows)).collect();
        Ok(Self {
            program,
            executable,
            arguments,
            members,
            input_count,
            output_count,
            constants,
            constant_buffers,
            textures,
            samplers,
            slots,
            buffers,
            scalar_scratch: Mutex::new(Vec::new()),
            quad_scratch: Mutex::new(Vec::new()),
            lane_scratch: Mutex::new(Vec::new()),
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Whether a caller that wants this program's exact results must run it as a quad ([`Executable::needs_quad`]).
    pub fn needs_quad(&self) -> bool {
        self.executable.needs_quad()
    }

    /// The entry arguments for one invocation's input registers.
    fn arguments_of(&self, inputs: &[[u32; 4]]) -> Result<Vec<[u32; 4]>, Error> {
        if inputs.len() < self.input_count {
            return Err(error(format!(
                "{} input registers given, {} needed",
                inputs.len(),
                self.input_count
            )));
        }
        let mut out = Vec::with_capacity(self.arguments.len());
        for a in &self.arguments {
            let mut value = [0u32; 4];
            if let Some(register) = inputs.get(a.register) {
                for (i, lane) in a.lanes.iter().enumerate() {
                    value[i] = register[*lane as usize];
                }
            } else if a.used {
                return Err(error(format!("input register {} not given", a.register)));
            }
            if a.boolean {
                value[0] = u32::from(value[0] != 0);
            }
            out.push(value);
        }
        Ok(out)
    }

    fn derivatives_of(&self, derivatives: &[[[u32; 4]; 2]]) -> Vec<[[u32; 4]; 2]> {
        self.arguments
            .iter()
            .map(|a| {
                let mut value = [[0u32; 4]; 2];
                if let Some(d) = derivatives.get(a.register) {
                    for axis in 0..2 {
                        for (i, lane) in a.lanes.iter().enumerate() {
                            value[axis][i] = d[axis][*lane as usize];
                        }
                    }
                }
                value
            })
            .collect()
    }

    fn execution(&self, o: super::Output) -> Execution {
        let mut outputs = vec![[0u32; 4]; self.output_count];
        let mut output_masks = vec![0u8; self.output_count];
        for (m, value) in self.members.iter().zip(&o.values) {
            if m.register == usize::MAX {
                continue;
            }
            for (i, lane) in m.lanes.iter().enumerate() {
                outputs[m.register][*lane as usize] = value[i];
            }
            output_masks[m.register] |= m.written;
        }
        Execution {
            outputs,
            output_masks,
            instructions: usize::try_from(o.steps).unwrap_or(usize::MAX),
            samples: usize::try_from(o.samples).unwrap_or(usize::MAX),
            discarded: o.discarded,
        }
    }

    fn bound<'a>(&self, buffers: &[(u32, &'a [[u32; 4]])]) -> Result<super::Bound<'a>, Error> {
        for &(slot, _, rows) in &self.constants {
            let given = buffers
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, v)| *v)
                .ok_or_else(|| error(format!("no constant buffer cb{slot}")))?;
            if given.len() < rows as usize {
                return Err(error(format!(
                    "cb{slot}: {} rows given, {rows} needed",
                    given.len()
                )));
            }
        }
        // the buffers were all found above, by slot
        self.executable
            .bind(
                |id| {
                    let (slot, _, _) = self.constants.iter().find(|(_, c, _)| *c == id)?;
                    buffers.iter().find(|(s, _)| s == slot).map(|(_, v)| *v)
                },
                |_| None,
            )
            .map_err(error)
    }

    fn backend<'a>(&'a self, sampler: &'a mut dyn Sampler) -> SlotBackend<'a> {
        SlotBackend {
            sampler,
            slots: &self.slots,
            buffers: &self.buffers,
        }
    }
}

/// A set of registers from the pool, if one is free.
fn take<const N: usize>(pool: &Mutex<Vec<Scratch<N>>>) -> Option<Scratch<N>> {
    pool.lock().ok()?.pop()
}

fn give_back<const N: usize>(pool: &Mutex<Vec<Scratch<N>>>, scratch: Scratch<N>) {
    if let Ok(mut pool) = pool.lock() {
        pool.push(scratch);
    }
}

/// The rows of a constant buffer's array in the module.
fn rows_of(program: &Program, id: ResourceId) -> Option<u32> {
    let global = program
        .module
        .global_variables
        .try_get(program.global(id)?)
        .ok()?;
    match &program.module.types[global.ty].inner {
        naga::TypeInner::Array {
            size: naga::ArraySize::Constant(n),
            ..
        } => Some(n.get()),
        _ => None,
    }
}

/// The program's requests in the sampler's vocabulary: bind slots instead of resource ids, and a buffer's words as the
/// element and byte offset its view addresses.
pub(super) struct SlotBackend<'a> {
    pub(super) sampler: &'a mut dyn Sampler,
    pub(super) slots: &'a BTreeMap<ResourceId, u32>,
    pub(super) buffers: &'a BTreeMap<ResourceId, BufferKind>,
}
impl SlotBackend<'_> {
    fn slot(&self, id: ResourceId) -> Result<u32, String> {
        self.slots
            .get(&id)
            .copied()
            .ok_or_else(|| format!("resource {} has no bind slot", id.0))
    }
    /// The bind slot of an element: a slot answers one binding, not an array's.
    fn slot_of(&self, e: Element) -> Result<u32, String> {
        if e.index != 0 {
            return Err(format!(
                "resource {}: element {} of an array, which a bind-slot sampler does not answer",
                e.id.0, e.index
            ));
        }
        self.slot(e.id)
    }
    /// A buffer view's address of word `word`: (element, byte offset in the element).
    fn address(&self, id: ResourceId, word: u32) -> [u32; 3] {
        match self.buffers.get(&id) {
            Some(BufferKind::Structured { stride }) if *stride >= 4 => {
                let per = stride / 4;
                [word / per, word % per * 4, 0]
            }
            // raw and typed views are addressed by the word
            _ => [word, 0, 0],
        }
    }
}
impl Backend for SlotBackend<'_> {
    fn sample(&mut self, r: ImageSample<Element>) -> Result<[f32; 4], String> {
        let (texture, sampler) = (self.slot_of(r.texture)?, self.slot_of(r.sampler)?);
        self.sampler.sample_image(ImageSample {
            texture,
            sampler,
            dim: r.dim,
            coord: r.coord,
            layer: r.layer,
            lod: r.lod,
            offset: r.offset,
            gather: r.gather,
            depth_ref: r.depth_ref,
            depth: r.depth,
        })
    }
    fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
        let texture = self.slot_of(r.texture)?;
        self.sampler.load_image(ImageLoad {
            texture,
            dim: r.dim,
            storage: r.storage,
            texel: r.texel,
            layer: r.layer,
            sample: r.sample,
            mip: r.mip,
        })
    }
    fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
        let texture = self.slot_of(r.texture)?;
        self.sampler.query_image(ImageQuery {
            texture,
            dim: r.dim,
            kind: r.kind,
            level: r.level,
        })
    }
    fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
        let view = self.slot_of(Element {
            id: r.view,
            index: r.index,
        })?;
        self.sampler.store_image(ImageStore {
            view,
            index: 0,
            dim: r.dim,
            texel: r.texel,
            layer: r.layer,
            value: r.value,
            lanes: r.lanes,
        })
    }
    fn atomic_image(&mut self, r: ImageAtomic<ResourceId>) -> Result<u32, String> {
        let view = self.slot_of(Element {
            id: r.view,
            index: r.index,
        })?;
        self.sampler.atomic_image(ImageAtomic {
            view,
            index: 0,
            dim: r.dim,
            texel: r.texel,
            layer: r.layer,
            op: r.op,
            signed: r.signed,
            value: r.value,
        })
    }
    fn store_buffer(&mut self, view: ResourceId, word: u32, value: [u32; 4], mask: u8) -> Result<(), String> {
        let address = self.address(view, word);
        let view = self.slot(view)?;
        self.sampler.store(Store {
            view,
            address,
            value,
            mask,
        })
    }
    fn atomic_add(&mut self, view: ResourceId, word: u32, value: u32) -> Result<u32, String> {
        let address = self.address(view, word);
        let view = self.slot(view)?;
        self.sampler.atomic_add(AtomicAdd { view, address, value })
    }
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        let slot = self.slot(view)?;
        let mut out = [0u32; 4];
        match self.buffers.get(&view) {
            // a typed view's element is four lanes, one load answers all of them
            Some(BufferKind::Typed(_)) => {
                let (mut at, end) = (word, word + u32::from(count));
                while at < end {
                    let element = at / 4;
                    let lanes = self.sampler.load(crate::Load {
                        texture: slot,
                        texel: [i32::try_from(element).unwrap_or(i32::MAX), 0, 0],
                        mip: 0,
                    })?;
                    while at < end && at / 4 == element {
                        out[(at - word) as usize] = lanes[(at % 4) as usize];
                        at += 1;
                    }
                }
            }
            _ => {
                for (i, w) in out.iter_mut().take(usize::from(count)).enumerate() {
                    let [element, offset, _] = self.address(view, word + i as u32);
                    *w = self.sampler.structured(slot, element, offset)?;
                }
            }
        }
        Ok(out)
    }
    fn buffer_words(&mut self, view: ResourceId) -> Result<u32, String> {
        let slot = self.slot(view)?;
        // without the sampler saying how long the buffer is, no read is out of range: its own error stops it
        let Some(elements) = self.sampler.buffer_elements(slot) else {
            return Ok(u32::MAX);
        };
        let per = match self.buffers.get(&view) {
            Some(BufferKind::Typed(_)) => 4,
            Some(BufferKind::Structured { stride }) => (stride / 4).max(1),
            _ => 1,
        };
        Ok(elements.saturating_mul(per))
    }
}

impl PixelShader for ProgramShader {
    fn interface(&self) -> &Interface {
        &self.program.interface
    }
    fn input_count(&self) -> usize {
        self.input_count
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &self.constant_buffers
    }
    fn textures(&self) -> &[u32] {
        &self.textures
    }
    fn samplers(&self) -> &[u32] {
        &self.samplers
    }

    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        self.run_scalar(inputs, None, buffers, sampler)
    }

    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        self.run_scalar(inputs, Some(derivatives), buffers, sampler)
    }

    fn execute_lanes(
        &self,
        inputs: &[&[[u32; 4]]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Vec<Execution>, (usize, Error)> {
        let mut out = Vec::with_capacity(inputs.len());
        for (k, chunk) in inputs.chunks(LANES).enumerate() {
            match self.execute_lockstep(chunk, derivatives, buffers, sampler) {
                Ok(done) => out.extend(done),
                // run again one invocation after another: the error is then the first failing one's, as alone
                Err(_) => {
                    for (i, one) in chunk.iter().enumerate() {
                        let done = self
                            .run_scalar(one, Some(derivatives), buffers, sampler)
                            .map_err(|e| (k * LANES + i, e))?;
                        out.push(done);
                    }
                }
            }
        }
        Ok(out)
    }

    fn execute_quad(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<[Execution; 4], Error> {
        let bound = self.bound(buffers)?;
        let arguments = [
            self.arguments_of(inputs[0])?,
            self.arguments_of(inputs[1])?,
            self.arguments_of(inputs[2])?,
            self.arguments_of(inputs[3])?,
        ];
        let mut backend = self.backend(sampler);
        let scratch = take(&self.quad_scratch).unwrap_or_else(|| Scratch::new(&self.executable));
        let mut runner = Runner::<4>::with_scratch(&self.executable, &bound, &mut backend, None, scratch);
        let outputs = runner.run([&arguments[0], &arguments[1], &arguments[2], &arguments[3]]);
        give_back(&self.quad_scratch, runner.into_scratch());
        Ok(outputs.map_err(error)?.map(|o| self.execution(o)))
    }
}

impl ProgramShader {
    /// [`PixelShader::execute_lanes`] without its second run: the invocations in lockstep, 16 at a time, and on an
    /// error the one the lockstep run met (of whichever invocation met one first in lockstep, which need not be the
    /// first to fail alone).
    pub fn execute_lockstep(
        &self,
        inputs: &[&[[u32; 4]]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Vec<Execution>, Error> {
        let derivatives = self.derivatives_of(derivatives);
        let mut out = Vec::with_capacity(inputs.len());
        for chunk in inputs.chunks(LANES) {
            out.extend(self.run_lanes(chunk, &derivatives, buffers, sampler)?);
        }
        Ok(out)
    }

    /// Up to [`LANES`] invocations in lockstep, sharing `derivatives` (the entry arguments').
    fn run_lanes(
        &self,
        inputs: &[&[[u32; 4]]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Vec<Execution>, Error> {
        let bound = self.bound(buffers)?;
        let arguments = inputs
            .iter()
            .map(|one| self.arguments_of(one))
            .collect::<Result<Vec<_>, _>>()?;
        let count = arguments.len();
        let Some(last) = arguments.last() else {
            return Ok(Vec::new());
        };
        // the lanes past the invocations are given the last one's arguments and not run
        let args: [&[[u32; 4]]; LANES] = std::array::from_fn(|l| arguments.get(l).unwrap_or(last).as_slice());
        let mut backend = self.backend(sampler);
        let scratch = take(&self.lane_scratch).unwrap_or_else(|| Scratch::new(&self.executable));
        let mut runner = Runner::<LANES, u16>::with_scratch(
            &self.executable,
            &bound,
            &mut backend,
            Some(derivatives),
            scratch,
        );
        let outputs = runner.run_first(args, count);
        give_back(&self.lane_scratch, runner.into_scratch());
        Ok(outputs
            .map_err(error)?
            .into_iter()
            .take(count)
            .map(|o| self.execution(o))
            .collect())
    }

    fn run_scalar(
        &self,
        inputs: &[[u32; 4]],
        derivatives: Option<&[[[u32; 4]; 2]]>,
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        let bound = self.bound(buffers)?;
        let arguments = self.arguments_of(inputs)?;
        let derivatives = derivatives.map(|d| self.derivatives_of(d));
        let mut backend = self.backend(sampler);
        let scratch = take(&self.scalar_scratch).unwrap_or_else(|| Scratch::new(&self.executable));
        let mut runner = Runner::<1>::with_scratch(
            &self.executable,
            &bound,
            &mut backend,
            derivatives.as_deref(),
            scratch,
        );
        let output = runner.run([&arguments]);
        give_back(&self.scalar_scratch, runner.into_scratch());
        let [output] = output.map_err(error)?;
        Ok(self.execution(output))
    }
}
