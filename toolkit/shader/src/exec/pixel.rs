//! A [`Program`] lifted from D3D bytecode behind [`PixelShader`]: inputs and outputs by signature register, constant
//! buffers and samplers by bind slot, as every bake already drives a shader.

use std::collections::BTreeMap;

use super::{Backend, Executable, ImageLoad, ImageQuery, ImageSample, ImageStore, Runner, error};
use crate::{
    AtomicAdd, BufferKind, D3dClass, Error, Execution, Interface, PixelShader, Program, ResourceId,
    ResourceKind, Sampler, Stage, StorageTarget, Store, SystemValue,
};

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
}

fn lanes_of(mask: u8) -> Vec<u8> {
    (0..4).filter(|l| mask >> l & 1 != 0).collect()
}

/// The lanes the module carries for an input: the pipeline fixes some, the interface states the rest.
fn width_of(v: &crate::Varying) -> usize {
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
        if program.stage != Stage::Pixel {
            return Err(error("not a pixel program"));
        }
        let executable = Executable::new(&program)?;
        let interface = &program.interface;
        let mut arguments = Vec::new();
        let mut input_count = 0;
        for v in &interface.inputs {
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
                written: sig.mask & !sig.read_write_mask,
            });
        }
        let mut constants = Vec::new();
        let (mut textures, mut samplers) = (Vec::new(), Vec::new());
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
                ResourceKind::Texture { .. } if binding.class == D3dClass::ShaderResource => {
                    textures.push(binding.register);
                }
                ResourceKind::Sampler { .. } => samplers.push(binding.register),
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
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
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
        let mut found: BTreeMap<ResourceId, &'a [[u32; 4]]> = BTreeMap::new();
        for &(slot, id, rows) in &self.constants {
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
            found.insert(id, given);
        }
        self.executable
            .bind(|id| found.get(&id).copied(), |_| None)
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
struct SlotBackend<'a> {
    sampler: &'a mut dyn Sampler,
    slots: &'a BTreeMap<ResourceId, u32>,
    buffers: &'a BTreeMap<ResourceId, BufferKind>,
}
impl SlotBackend<'_> {
    fn slot(&self, id: ResourceId) -> Result<u32, String> {
        self.slots
            .get(&id)
            .copied()
            .ok_or_else(|| format!("resource {} has no bind slot", id.0))
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
    fn sample(&mut self, r: ImageSample<ResourceId>) -> Result<[f32; 4], String> {
        let (texture, sampler) = (self.slot(r.texture)?, self.slot(r.sampler)?);
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
    fn load(&mut self, r: ImageLoad<ResourceId>) -> Result<[u32; 4], String> {
        let texture = self.slot(r.texture)?;
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
    fn query(&mut self, r: ImageQuery<ResourceId>) -> Result<[u32; 4], String> {
        let texture = self.slot(r.texture)?;
        self.sampler.query_image(ImageQuery {
            texture,
            dim: r.dim,
            kind: r.kind,
            level: r.level,
        })
    }
    fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
        let view = self.slot(r.view)?;
        self.sampler.store_image(ImageStore {
            view,
            dim: r.dim,
            texel: r.texel,
            layer: r.layer,
            value: r.value,
            lanes: r.lanes,
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
    fn buffer_words(&mut self, _view: ResourceId) -> Result<u32, String> {
        // the sampler answers reads by element and cannot say how many there are: no out-of-range read is caught
        // here, the sampler's own error stops it
        Ok(u32::MAX)
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
        let mut runner = Runner::<4>::new(&self.executable, &bound, &mut backend, None);
        let outputs = runner
            .run([&arguments[0], &arguments[1], &arguments[2], &arguments[3]])
            .map_err(error)?;
        Ok(outputs.map(|o| self.execution(o)))
    }
}

impl ProgramShader {
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
        let mut runner = Runner::<1>::new(&self.executable, &bound, &mut backend, derivatives.as_deref());
        let [output] = runner.run([&arguments]).map_err(error)?;
        Ok(self.execution(output))
    }
}
