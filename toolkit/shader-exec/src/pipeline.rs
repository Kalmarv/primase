//! The ray tracing pipeline on the CPU: `DispatchRays` runs a ray generation program per position, and `TraceRay`
//! traverses the acceleration structure the caller supplies, running the hit group's any-hit and intersection programs
//! for the candidates the ray meets, the closest-hit program for the hit it keeps and the miss program when it has none;
//! a callable program runs for `CallShader`. The programs are the function-form [`Program`]s of a ray tracing library
//! (see [`primase_shader::stage_functions`]), run one invocation at a time with [`Executable::function`]; their payloads,
//! attributes and parameters are passed by value in and out, which is what a pointer to a structure of function memory
//! is when nothing else touches it.
//!
//! What a library states (its hit groups: which programs make a group, of triangles or of procedural primitives) comes
//! from its subobjects ([`primase_shader::Subobject`]). What the application states, the shader table, is the caller's: which
//! hit group a hit index selects, which program a miss index and a callable index select. The hit group index of a hit
//! is DXR's: the ray's offset, plus its stride times the geometry's index in the bottom level, plus the instance's
//! contribution (section "Indexing into shader tables" of the specification,
//! https://microsoft.github.io/DirectX-Specs/d3d/Raytracing.html).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::pixel::SlotBackend;
use super::rayquery::{Built, Hit, Query, Scene, flags, transform_point, transform_vector};
use super::{
    Backend, CallShader, Decision, Element, Executable, ImageAtomic, ImageLoad, ImageQuery, ImageSample,
    ImageStore, ReportHit, Resources, Runner, TraceRay,
};
use crate::Sampler;
use primase_shader::{
    BufferKind, D3dClass, FunctionSig, HitGroup, IoType, Program, ResourceId, ResourceKind, StageFacts,
    StorageTarget, Subobject, SystemValue, stage_functions,
};

/// The pipeline flags the pipeline runs: they are added to the flags of every `TraceRay`, as the specification says of
/// `D3D12_RAYTRACING_PIPELINE_FLAG_SKIP_TRIANGLES` ("add in the RAY_FLAG_SKIP_TRIANGLES Ray flag") and
/// `SKIP_PROCEDURAL_PRIMITIVES`.
const SUPPORTED_PIPELINE_FLAGS: u32 = flags::SKIP_TRIANGLES | flags::SKIP_PROCEDURAL;

/// The deepest maximum recursion a pipeline may declare.
const MAX_DECLARABLE_RECURSION: u32 = 31;

/// The application's shader table: the program each index selects, by the export name the library gives it.
#[derive(Clone, Debug, Default)]
pub struct ShaderTable {
    /// The miss programs, by miss index.
    pub miss: Vec<String>,
    /// The hit groups, by hit group index.
    pub hit_groups: Vec<String>,
    /// The callable programs, by shader index.
    pub callable: Vec<String>,
}

/// What a dispatch binds besides the programs: the constant buffers of the root signature, by register and space, and
/// the acceleration structure all the programs trace into.
#[derive(Clone, Debug, Default)]
pub struct PipelineResources {
    pub constants: BTreeMap<(u32, u32), Vec<[u32; 4]>>,
    pub scene: Scene,
}

/// What a dispatch did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DispatchStats {
    /// Programs run, of every kind.
    pub programs: u64,
    /// Rays traced.
    pub rays: u64,
}

/// A program ready to run, with the register view its resources have.
#[derive(Debug)]
struct Entry {
    program: Program,
    executable: Executable,
    signature: FunctionSig,
    /// The bind slot of each resource (its register), the kind of each buffer.
    slots: BTreeMap<ResourceId, u32>,
    buffers: BTreeMap<ResourceId, BufferKind>,
}

/// A ray tracing pipeline: the programs of a library, its hit groups and the application's shader table.
#[derive(Debug)]
pub struct Pipeline {
    programs: BTreeMap<String, Entry>,
    groups: BTreeMap<String, HitGroup>,
    table: ShaderTable,
    /// The deepest `TraceRay` recursion the pipeline declares; none until a library's subobjects or
    /// [`Pipeline::with_max_recursion`] state it.
    max_recursion: Option<u32>,
    /// The pipeline flags (`D3D12_RAYTRACING_PIPELINE_CONFIG1`): ray flags every `TraceRay` of the pipeline gets added.
    config_flags: u32,
}

impl Pipeline {
    /// A pipeline of `programs` (each a ray tracing program, named by its export), the hit groups the programs'
    /// libraries state, and `table`. The pipeline's maximum recursion depth is the one a library's pipeline config
    /// subobject states; libraries that state none leave it to [`Pipeline::with_max_recursion`], and a dispatch
    /// without one is refused (the DXR specification: "A raytracing pipeline needs one raytracing pipeline
    /// configuration", section "D3D12_RAYTRACING_PIPELINE_CONFIG").
    pub fn new(programs: Vec<Program>, table: ShaderTable) -> Result<Self, String> {
        let mut entries = BTreeMap::new();
        let mut groups = BTreeMap::new();
        let mut max_recursion = None;
        let mut config_flags = None;
        for program in programs {
            let StageFacts::RayTracing(facts) = program.interface.facts.clone() else {
                return Err(format!(
                    "a {:?} program is not a ray tracing program",
                    program.stage
                ));
            };
            for sub in &facts.subobjects {
                match sub {
                    Subobject::HitGroup(g) => {
                        groups.insert(g.name.clone(), g.clone());
                    }
                    Subobject::PipelineConfig {
                        max_recursion_depth,
                        flags,
                        ..
                    } => {
                        // a config without flags (`D3D12_RAYTRACING_PIPELINE_CONFIG`) has none set
                        let flags = flags.unwrap_or(0);
                        if config_flags.is_some_and(|f| f != flags) {
                            return Err(format!(
                                "the libraries state different pipeline flags ({:#x} and {flags:#x})",
                                config_flags.unwrap_or_default()
                            ));
                        }
                        config_flags = Some(flags);
                        // "If multiple [...] configurations are present [...] they must all match"
                        if max_recursion.is_some_and(|d| d != *max_recursion_depth) {
                            return Err(format!(
                                "the libraries state different maximum recursion depths ({} and {max_recursion_depth})",
                                max_recursion.unwrap_or_default()
                            ));
                        }
                        max_recursion = Some(*max_recursion_depth);
                    }
                    _ => {}
                }
            }
            let signature = stage_functions(&program.interface)?
                .and_then(|mut f| f.pop())
                .ok_or("a ray tracing program without its function")?;
            let executable =
                Executable::function(&program, "main").map_err(|e| format!("{}: {e}", facts.export))?;
            let mut slots = BTreeMap::new();
            let mut buffers = BTreeMap::new();
            for r in &program.interface.resources {
                let Some(binding) = r.provenance.d3d else {
                    return Err(format!("resource {} has no D3D bind slot", r.provenance.name));
                };
                slots.insert(r.id, binding.register);
                if let ResourceKind::Buffer(kind)
                | ResourceKind::Storage {
                    target: StorageTarget::Buffer(kind),
                    ..
                } = &r.kind
                {
                    buffers.insert(r.id, *kind);
                }
            }
            entries.insert(
                facts.export.clone(),
                Entry {
                    program,
                    executable,
                    signature,
                    slots,
                    buffers,
                },
            );
        }
        Ok(Self {
            programs: entries,
            groups,
            table,
            max_recursion,
            config_flags: config_flags.unwrap_or(0),
        })
    }

    /// State the maximum recursion depth, the pipeline config an application gives when its libraries do not (the
    /// depth the application declares in `D3D12_RAYTRACING_PIPELINE_CONFIG`): replaces the libraries' own.
    pub fn with_max_recursion(mut self, depth: u32) -> Self {
        self.max_recursion = Some(depth);
        self
    }

    /// State the pipeline flags (`D3D12_RAYTRACING_PIPELINE_CONFIG1`'s `Flags`: the SKIP_TRIANGLES and
    /// SKIP_PROCEDURAL ray flags), as an application gives them when its libraries state none: replaces the
    /// libraries' own.
    pub fn with_pipeline_flags(mut self, flags: u32) -> Self {
        self.config_flags = flags;
        self
    }

    /// Add or replace hit groups the libraries do not state.
    pub fn with_hit_groups(mut self, groups: impl IntoIterator<Item = HitGroup>) -> Self {
        for g in groups {
            self.groups.insert(g.name.clone(), g);
        }
        self
    }

    /// Run the ray generation program `raygen` at every position of `dimensions`, x fastest. Image writes and buffer
    /// access go to `sampler` by bind slot (the register), as for a pixel program.
    pub fn dispatch_rays(
        &self,
        raygen: &str,
        dimensions: [u32; 3],
        resources: &PipelineResources,
        sampler: &mut dyn Sampler,
    ) -> Result<DispatchStats, String> {
        let Some(depth) = self.max_recursion else {
            return Err(
                "the pipeline has no maximum recursion depth: a raytracing pipeline needs one raytracing pipeline \
                configuration (DXR specification, D3D12_RAYTRACING_PIPELINE_CONFIG); state it in a library's \
                subobjects or with Pipeline::with_max_recursion"
                    .into(),
            );
        };
        if depth > MAX_DECLARABLE_RECURSION {
            return Err(format!(
                "the pipeline declares a maximum recursion depth of {depth}, past the {MAX_DECLARABLE_RECURSION} DXR allows \
                (\"Ray recursion limit\": a depth in the range [0..31])"
            ));
        }
        if self.config_flags & !SUPPORTED_PIPELINE_FLAGS != 0 {
            return Err(format!(
                "the pipeline has flags {:#x}; only RAYTRACING_PIPELINE_FLAG_SKIP_TRIANGLES (0x100) and \
                SKIP_PROCEDURAL_PRIMITIVES (0x200) are run",
                self.config_flags
            ));
        }
        if self.config_flags & SUPPORTED_PIPELINE_FLAGS == SUPPORTED_PIPELINE_FLAGS {
            return Err(
                "the pipeline has both RAYTRACING_PIPELINE_FLAG_SKIP_TRIANGLES and SKIP_PROCEDURAL_PRIMITIVES: \
                every ray would skip all geometry, which the ray flags forbid"
                    .into(),
            );
        }
        let entry = self
            .programs
            .get(raygen)
            .ok_or_else(|| format!("no program {raygen}"))?;
        if entry.program.stage != primase_shader::Stage::RayGeneration {
            return Err(format!(
                "{raygen} is a {:?} program, not a ray generation program",
                entry.program.stage
            ));
        }
        let built = Arc::new(Built::new(&resources.scene)?);
        let mut bound = BTreeMap::new();
        for (name, entry) in &self.programs {
            let mut r = Resources::default();
            for res in &entry.program.interface.resources {
                let Some(binding) = res.provenance.d3d else {
                    continue;
                };
                if matches!(res.kind, ResourceKind::ConstantBuffer { .. })
                    && binding.class == D3dClass::ConstantBuffer
                    && let Some(rows) = resources.constants.get(&(binding.register, binding.space))
                {
                    r.constants.insert(res.id, rows.clone());
                }
            }
            bound.insert(name.as_str(), r);
        }
        let mut host = Host {
            pipe: self,
            built,
            resources: bound,
            sampler,
            stack: Vec::new(),
            active: Vec::new(),
            stats: DispatchStats::default(),
            depth: 0,
        };
        for z in 0..dimensions[2] {
            for y in 0..dimensions[1] {
                for x in 0..dimensions[0] {
                    let env = Env {
                        index: [x, y, z],
                        dimensions,
                        ..Env::default()
                    };
                    host.run(raygen, &env, &mut [], &[])
                        .map_err(|e| format!("dispatch ({x}, {y}, {z}): {e}"))?;
                }
            }
        }
        Ok(host.stats)
    }

    fn group_of(&self, built: &Built, offset: u32, stride: u32, hit: &Hit) -> Option<&HitGroup> {
        let index = offset
            .wrapping_add(stride.wrapping_mul(hit.geometry))
            .wrapping_add(built.instance(hit.instance).contribution);
        self.table
            .hit_groups
            .get(index as usize)
            .and_then(|name| self.groups.get(name))
    }
}

/// The ray state the system values of a program read.
#[derive(Clone, Debug, Default)]
struct Env {
    index: [u32; 3],
    dimensions: [u32; 3],
    world_origin: [f32; 3],
    world_direction: [f32; 3],
    t_min: f32,
    t_current: f32,
    flags: u32,
    hit: Option<HitEnv>,
}

/// The hit a program runs for.
#[derive(Clone, Debug)]
struct HitEnv {
    instance: u32,
    id: u32,
    geometry: u32,
    primitive: u32,
    kind: u32,
    object_to_world: [[f32; 4]; 3],
    world_to_object: [[f32; 4]; 3],
}

/// A `TraceRay` in progress.
struct Active {
    query: Query,
    offset: u32,
    stride: u32,
    flags: u32,
    origin: [f32; 3],
    direction: [f32; 3],
    /// The payload as the programs so far left it.
    payload: Vec<u32>,
    /// The attributes and kind of the last procedural hit committed (the query holds the hit itself, a triangle's
    /// attributes being its barycentrics).
    procedural: Option<(Vec<u32>, u32)>,
    /// The group of the procedural candidate whose intersection program is running.
    group: Option<HitGroup>,
    ended: bool,
}

struct Host<'p, 's> {
    pipe: &'p Pipeline,
    built: Arc<Built>,
    resources: BTreeMap<&'p str, Resources>,
    sampler: &'s mut dyn Sampler,
    /// The programs running, innermost last: whose resources a request names.
    stack: Vec<&'p Entry>,
    active: Vec<Active>,
    stats: DispatchStats,
    depth: u32,
}

fn pack(words: &[u32]) -> Vec<[u32; 4]> {
    words
        .chunks(4)
        .map(|c| std::array::from_fn(|i| c.get(i).copied().unwrap_or(0)))
        .collect()
}

impl<'p> Host<'p, '_> {
    /// Run program `name` once with `env`, its memory arguments the words of `memory` (the payload, then the
    /// attributes; or the parameter), which it leaves as it ends. What the program decided about the hit comes back.
    fn run(
        &mut self,
        name: &str,
        env: &Env,
        memory: &mut [&mut Vec<u32>],
        attributes: &[u32],
    ) -> Result<Decision, String> {
        let pipe: &'p Pipeline = self.pipe;
        let (key, entry) = pipe
            .programs
            .get_key_value(name)
            .ok_or_else(|| format!("the shader table names {name}, which the library does not export"))?;
        if self.stack.len() as u32
            > pipe
                .max_recursion
                .unwrap_or(0)
                .saturating_mul(4)
                .saturating_add(8)
        {
            return Err("the programs call each other too deeply".into());
        }
        let interface = &entry.program.interface;
        let mut registers: Vec<[u32; 4]> = Vec::new();
        let mut taken = 0;
        for item in &entry.signature.arguments {
            match item.ty {
                IoType::Value(_) => {
                    let v = item
                        .varying
                        .and_then(|i| interface.inputs.get(i))
                        .ok_or_else(|| format!("{name}: the argument {} is no input", item.name))?;
                    let system = v
                        .system
                        .ok_or_else(|| format!("{name}: the input {} is no system value", item.name))?;
                    registers.push(system_value(system, v.label.index, env)?);
                }
                IoType::Memory => {
                    // payload (or parameter) first, the hit attributes second
                    let words = match taken {
                        0 if memory.is_empty() => Vec::new(),
                        0 => memory[0].clone(),
                        _ => attributes.to_vec(),
                    };
                    let size = entry
                        .executable
                        .pointer_words(taken)
                        .ok_or_else(|| format!("{name}: no pointer argument {taken}"))?;
                    let mut words = words;
                    words.resize(size as usize, 0);
                    registers.extend(pack(&words));
                    taken += 1;
                }
                _ => {
                    return Err(format!(
                        "{name}: the argument {} is of a kind a ray tracing program does not take",
                        item.name
                    ));
                }
            }
        }
        let resources = self.resources[key.as_str()].clone();
        let mut bound = entry
            .executable
            .bind_resources(&resources)
            .map_err(|e| format!("{name}: {e}"))?;
        for r in &interface.resources {
            if matches!(r.kind, ResourceKind::AccelerationStructure) {
                bound = bound.with_scene(r.id, self.built.clone());
            }
        }
        self.stack.push(entry);
        self.depth += 1;
        self.stats.programs += 1;
        let result = {
            let mut runner = Runner::<1>::new(&entry.executable, &bound, &mut *self, None);
            runner.run([&registers])
        };
        self.depth -= 1;
        self.stack.pop();
        let [out] = result.map_err(|e| format!("{name}: {e}"))?;
        for (k, m) in memory.iter_mut().enumerate() {
            if let Some(words) = out.pointees.get(k) {
                m.clear();
                m.extend_from_slice(words);
            }
        }
        Ok(out.decision)
    }

    fn top(&mut self) -> Result<&mut Active, String> {
        self.active
            .last_mut()
            .ok_or_else(|| "a hit reported outside a ray".to_owned())
    }

    fn last(&self) -> Result<&Active, String> {
        self.active
            .last()
            .ok_or_else(|| "a hit reported outside a ray".to_owned())
    }

    /// The environment a program of a hit runs in.
    fn env_for(built: &Built, a: &Active, hit: &Hit, t_current: f32, kind: u32) -> Env {
        let instance = built.instance(hit.instance);
        let inverse = *built.inverse(hit.instance);
        Env {
            world_origin: a.origin,
            world_direction: a.direction,
            t_min: a.query.tmin,
            t_current,
            flags: a.flags,
            hit: Some(HitEnv {
                instance: hit.instance,
                id: instance.id & 0x00ff_ffff,
                geometry: hit.geometry,
                primitive: hit.primitive,
                kind,
                object_to_world: instance.transform,
                world_to_object: inverse,
            }),
            ..Env::default()
        }
    }

    fn trace(&mut self, r: TraceRay<'_>) -> Result<(), String> {
        if self.active.len() as u32 >= self.pipe.max_recursion.unwrap_or(0) {
            return Err(format!(
                "TraceRay recurses deeper than the pipeline's maximum of {}",
                self.pipe.max_recursion.unwrap_or(0)
            ));
        }
        self.stats.rays += 1;
        let ray_flags = r.flags | self.pipe.config_flags;
        let query = Query::new(
            r.scene,
            ray_flags,
            r.mask,
            r.t_min,
            r.t_max,
            r.origin,
            r.direction,
        );
        self.active.push(Active {
            query,
            offset: r.hit_group_offset,
            stride: r.hit_group_stride,
            flags: ray_flags,
            origin: r.origin,
            direction: r.direction,
            payload: r.payload.to_vec(),
            procedural: None,
            group: None,
            ended: false,
        });
        let outcome = self.trace_active(r.miss_index);
        let done = self.active.pop();
        outcome?;
        if let Some(a) = done {
            r.payload
                .copy_from_slice(&a.payload[..r.payload.len().min(a.payload.len())]);
        }
        Ok(())
    }

    fn trace_active(&mut self, miss_index: u32) -> Result<(), String> {
        let built = self.built.clone();
        loop {
            let (more, offset, stride) = {
                let a = self.top()?;
                (a.query.proceed(&built), a.offset, a.stride)
            };
            if !more {
                break;
            }
            let candidate = self.top()?.query.candidate.ok_or("a candidate that is gone")?;
            let group = self.pipe.group_of(&built, offset, stride, &candidate).cloned();
            match candidate.kind {
                1 => {
                    let attributes = vec![
                        candidate.barycentrics[0].to_bits(),
                        candidate.barycentrics[1].to_bits(),
                    ];
                    let decision = match group.as_ref().and_then(|g| g.any_hit.clone()) {
                        Some(name) => self.run_hit_program(&name, &candidate, candidate.t, &attributes)?,
                        None => Decision::None,
                    };
                    let a = self.top()?;
                    match decision {
                        Decision::Ignore => {}
                        Decision::None | Decision::AcceptAndEnd => {
                            a.query.confirm()?;
                            if decision == Decision::AcceptAndEnd {
                                a.query.terminate();
                            }
                        }
                    }
                }
                3 => {
                    let Some(name) = group.as_ref().and_then(|g| g.intersection.clone()) else {
                        continue;
                    };
                    self.top()?.group = group;
                    let tcurrent = self.top()?.query.tmax();
                    let env = { Self::env_for(&self.built, self.last()?, &candidate, tcurrent, 0) };
                    let mut none: [&mut Vec<u32>; 0] = [];
                    self.run(&name, &env, &mut none, &[])?;
                    let a = self.top()?;
                    a.group = None;
                    if a.ended {
                        break;
                    }
                }
                other => return Err(format!("a candidate of kind {other}")),
            }
        }
        // the closest hit, or the miss
        let (committed, ray_flags, offset, stride) = {
            let a = self.top()?;
            let hit = a.query.committed;
            let (attributes, kind) = match hit {
                Some(h) if h.kind == 1 => (
                    vec![h.barycentrics[0].to_bits(), h.barycentrics[1].to_bits()],
                    if h.front { 254 } else { 255 },
                ),
                _ => a.procedural.clone().unwrap_or_default(),
            };
            (hit.map(|h| (h, attributes, kind)), a.flags, a.offset, a.stride)
        };
        match committed {
            Some((hit, attributes, kind)) => {
                if ray_flags & flags::SKIP_CLOSEST_HIT == 0
                    && let Some(name) = self
                        .pipe
                        .group_of(&built, offset, stride, &hit)
                        .and_then(|g| g.closest_hit.clone())
                {
                    let env = Self::env_for(&self.built, self.last()?, &hit, hit.t, kind);
                    let mut payload = std::mem::take(&mut self.top()?.payload);
                    let result = self.run(&name, &env, &mut [&mut payload], &attributes);
                    self.top()?.payload = payload;
                    result?;
                }
            }
            None => {
                if let Some(name) = self.pipe.table.miss.get(miss_index as usize).cloned() {
                    let env = {
                        let a = self.top()?;
                        Env {
                            world_origin: a.origin,
                            world_direction: a.direction,
                            t_min: a.query.tmin,
                            t_current: a.query.tmax(),
                            flags: a.flags,
                            ..Env::default()
                        }
                    };
                    let mut payload = std::mem::take(&mut self.top()?.payload);
                    let result = self.run(&name, &env, &mut [&mut payload], &[]);
                    self.top()?.payload = payload;
                    result?;
                }
            }
        }
        Ok(())
    }

    /// Run an any-hit program for a candidate with the payload of the ray in progress.
    fn run_hit_program(
        &mut self,
        name: &str,
        hit: &Hit,
        t_current: f32,
        attributes: &[u32],
    ) -> Result<Decision, String> {
        let kind = if hit.kind == 1 {
            if hit.front { 254 } else { 255 }
        } else {
            0
        };
        let env = { Self::env_for(&self.built, self.last()?, hit, t_current, kind) };
        let mut payload = std::mem::take(&mut self.top()?.payload);
        let result = self.run(name, &env, &mut [&mut payload], attributes);
        self.top()?.payload = payload;
        result
    }

    fn slot_backend(&mut self) -> Result<SlotBackend<'_>, String> {
        let entry = *self.stack.last().ok_or("a request outside a program")?;
        Ok(SlotBackend {
            sampler: &mut *self.sampler,
            slots: &entry.slots,
            buffers: &entry.buffers,
        })
    }
}

/// A system value as the register a program takes it in.
fn system_value(system: SystemValue, index: u32, env: &Env) -> Result<[u32; 4], String> {
    let f3 = |v: [f32; 3]| [v[0].to_bits(), v[1].to_bits(), v[2].to_bits(), 0];
    let hit = || {
        env.hit
            .as_ref()
            .ok_or("a hit's value in a program that runs without a hit")
    };
    let object = |point: bool| -> Result<[u32; 4], String> {
        let h = hit()?;
        let (o, d) = (
            transform_point(&h.world_to_object, env.world_origin),
            transform_vector(&h.world_to_object, env.world_direction),
        );
        Ok(f3(if point { o } else { d }))
    };
    Ok(match system {
        SystemValue::DispatchRaysIndex => [env.index[0], env.index[1], env.index[2], 0],
        SystemValue::DispatchRaysDimensions => [env.dimensions[0], env.dimensions[1], env.dimensions[2], 0],
        SystemValue::WorldRayOrigin => f3(env.world_origin),
        SystemValue::WorldRayDirection => f3(env.world_direction),
        SystemValue::ObjectRayOrigin => object(true)?,
        SystemValue::ObjectRayDirection => object(false)?,
        SystemValue::RayTMin => [env.t_min.to_bits(), 0, 0, 0],
        SystemValue::RayTCurrent => [env.t_current.to_bits(), 0, 0, 0],
        SystemValue::RayFlags => [env.flags, 0, 0, 0],
        SystemValue::InstanceId => [hit()?.id, 0, 0, 0],
        SystemValue::InstanceIndex => [hit()?.instance, 0, 0, 0],
        SystemValue::GeometryIndex => [hit()?.geometry, 0, 0, 0],
        SystemValue::PrimitiveIndex => [hit()?.primitive, 0, 0, 0],
        SystemValue::HitKind => [hit()?.kind, 0, 0, 0],
        SystemValue::ObjectToWorld => hit()?
            .object_to_world
            .get(index as usize)
            .map(|r| r.map(f32::to_bits))
            .ok_or("a transform row past the third")?,
        SystemValue::WorldToObject => hit()?
            .world_to_object
            .get(index as usize)
            .map(|r| r.map(f32::to_bits))
            .ok_or("a transform row past the third")?,
        other => {
            return Err(format!(
                "the system value {other:?} is not one a ray tracing program has"
            ));
        }
    })
}

impl Backend for Host<'_, '_> {
    fn sample(&mut self, r: ImageSample<Element>) -> Result<[f32; 4], String> {
        self.slot_backend()?.sample(r)
    }
    fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
        self.slot_backend()?.load(r)
    }
    fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
        self.slot_backend()?.query(r)
    }
    fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
        self.slot_backend()?.store_image(r)
    }
    fn atomic_image(&mut self, r: ImageAtomic<ResourceId>) -> Result<u32, String> {
        self.slot_backend()?.atomic_image(r)
    }
    fn store_buffer(&mut self, view: ResourceId, word: u32, value: [u32; 4], mask: u8) -> Result<(), String> {
        self.slot_backend()?.store_buffer(view, word, value, mask)
    }
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        self.slot_backend()?.read_buffer(view, word, count)
    }
    fn buffer_words(&mut self, view: ResourceId) -> Result<u32, String> {
        self.slot_backend()?.buffer_words(view)
    }
    fn atomic_add(&mut self, view: ResourceId, word: u32, value: u32) -> Result<u32, String> {
        self.slot_backend()?.atomic_add(view, word, value)
    }

    fn trace_ray(&mut self, r: TraceRay<'_>) -> Result<(), String> {
        self.trace(r)
    }

    fn call_shader(&mut self, r: CallShader<'_>) -> Result<(), String> {
        let name = self
            .pipe
            .table
            .callable
            .get(r.index as usize)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "CallShader({}): the shader table has no callable program there",
                    r.index
                )
            })?;
        let env = Env::default();
        let mut parameter = r.parameter.to_vec();
        self.run(&name, &env, &mut [&mut parameter], &[])?;
        let n = r.parameter.len().min(parameter.len());
        r.parameter[..n].copy_from_slice(&parameter[..n]);
        Ok(())
    }

    fn report_hit(&mut self, r: ReportHit<'_>) -> Result<bool, String> {
        let (candidate, group, in_range) = {
            let a = self.top()?;
            let candidate = a
                .query
                .candidate
                .ok_or("ReportHit outside an intersection program")?;
            // section "ReportHit": a hit is rejected if t is outside the current ray interval
            (
                candidate,
                a.group.clone(),
                a.query.tmin <= r.t && r.t <= a.query.tmax(),
            )
        };
        if !in_range {
            return Ok(false);
        }
        let attributes = r.attributes.to_vec();
        let mut hit = candidate;
        hit.t = r.t;
        // a non-opaque geometry offers the hit to the group's any-hit program first
        let decision = match group.as_ref().and_then(|g| g.any_hit.clone()) {
            Some(name) if !candidate.opaque => self.run_hit_program(&name, &hit, r.t, &attributes)?,
            _ => Decision::None,
        };
        if decision == Decision::Ignore {
            return Ok(false);
        }
        let a = self.top()?;
        a.query.generate(r.t)?;
        a.procedural = Some((attributes, r.hit_kind));
        if decision == Decision::AcceptAndEnd {
            a.ended = true;
            a.query.terminate();
        }
        Ok(true)
    }
}
