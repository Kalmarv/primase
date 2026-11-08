//! A FromSoft forward pixel program (Sekiro's DXBC, Elden Ring's DXIL) run one pixel at a time on a stated scene, as its
//! `core.shader_binding` binds it: what it does with the scene behind its surface, the depth of the water there, the view
//! and the sun, apart from what its material textures hold. What a water surface is ([`primase_core::WaterBody`]) is
//! measured from such runs ([`crate::water_derive`]).
//!
//! The scene of a run ([`ProbeScene`]): a flat surface point at the world's origin, facing up (+y) with its tangent along
//! +x, seen by a camera ([`FrameCamera`]) at a stated position looking at it, its depth along the view axis the camera's
//! distance (the point is on the view axis, at the middle of a one-pixel view). Behind it, at a stated depth along the
//! same axis, the scene has a stated colour. The frame's constants are given by their roles ([`FrozenFrame`] with that
//! camera and, where stated, a sun); the binding's own words (the material's, the source's defaults) are as bound. Each texture the
//! binding binds to the frame's scene colour reads the colour behind, each it binds to the frame's depth the device depth
//! of what is behind, a shadow map is lit everywhere, a default its value, and a material texture what the caller answers.
//! The scene has no local lights: the light tables a binding binds are empty, and its per-tile light lists are packed by
//! [`pack_light_lists`] for no lights in every tile.
//!
//! The pixel program's inputs are what the material's own vertex program writes for a vertex at that point
//! (`layer_bake::VertexStage`, run in the same frame: its normal up, its tangent along +x, its texture coordinates the
//! run's), and the pixel's position its own: the middle of the one-pixel view at the surface's device depth. The vertex
//! program is bound as it states its inputs itself ([`resolve_vertex`]), not as the scene's meshes give them: the vertex
//! is the probe's. An input the vertex program does not write refuses the program.
use std::collections::BTreeMap;

use iridia_exec::probe::{Answer, AnsweredReads};
use iridia_exec::{ImageLoad, ImageQuery, ImageSample, PixelShader, ProgramShader, Sample, Sampler};
use iridia_interface::{BufferKind, Interface, ResourceKind};
use primase_core::shader::{
    BufferSource, FrameTexture, LightListPart, Shader, StageBinding, TextureSource, pack_light_lists,
};

use crate::layer_bake::resolve::{register, resolve_vertex};
use iridia_exec::{Input, Lane};

use crate::layer_bake::{self, FrameCamera, FrameDepth, FrameSun, FrozenFrame};

/// The camera's clip planes in every run: near enough and far enough for any depth a measurement states.
pub const DEPTH: FrameDepth = FrameDepth {
    near: 0.05,
    far: 100_000.,
};

/// Whether `stage` binds a texture to the frame's scene colour and one to its depth: what a program that draws the
/// scene behind a surface through a depth of water reads.
pub fn reads_behind(stage: &StageBinding) -> bool {
    let reads = |frame: FrameTexture| {
        stage
            .textures
            .iter()
            .any(|t| matches!(&t.source, TextureSource::Frame(f) if *f == frame))
    };
    reads(FrameTexture::SceneColour) && reads(FrameTexture::SceneDepth)
}

/// One run's scene ([`WaterProbe::pixel`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeScene {
    /// The camera's world position; the surface point is at the origin, so its depth along the view axis is this
    /// position's length.
    pub camera: [f32; 3],
    /// The depth along the view axis of what is behind the surface (at or beyond the surface's own: what is in front
    /// of it is not behind it).
    pub behind_depth: f32,
    /// The scene colour behind the surface (linear, as the frame's colour target holds it).
    pub behind: [f32; 3],
    /// Seconds of global time.
    pub time: f32,
    /// The hour of the day.
    pub hour: f32,
    /// The dominant light, `None` for none.
    pub sun: Option<FrameSun>,
    /// The texture coordinate every coordinate set takes (`xy`; `zw` zero).
    pub uv: [f32; 2],
}

impl ProbeScene {
    /// The surface seen from `distance` metres at `elevation` radians above its plane (from +x towards +y), with what
    /// is behind it `water` metres further along the view axis, white, at noon, no sun, time 0, coordinate 0.
    pub fn at(distance: f32, elevation: f32, water: f32) -> Self {
        ProbeScene {
            camera: [distance * elevation.cos(), distance * elevation.sin(), 0.],
            behind_depth: distance + water,
            behind: [1.; 3],
            time: 0.,
            hour: 12.,
            sun: None,
            uv: [0.; 2],
        }
    }

    /// The surface point's depth along the view axis.
    pub fn surface_depth(&self) -> f32 {
        self.camera.iter().map(|c| c * c).sum::<f32>().sqrt()
    }
}

/// What a run gives: the first target's four lanes, every texture read the program made, in order, and the frame's
/// values the probe stated rather than the scene gave (a constant of no known role, a source's default), by reason.
#[derive(Debug)]
pub struct ProbeRun {
    pub colour: [f32; 4],
    pub samples: Vec<ImageSample<u32>>,
    pub stated: BTreeMap<String, u64>,
}

/// What a texture register reads in a run.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    SceneColour,
    SceneDepth,
    /// A shadow map: every comparison passes.
    Lit,
    Value([f32; 4]),
    /// A material texture: the caller answers.
    Material,
}

/// A structured buffer a run reads: elements of `stride` bytes.
#[derive(Clone, Debug, PartialEq)]
struct Structured {
    stride: u32,
    bytes: Vec<u8>,
}

/// A forward pixel program bound for probing ([`WaterProbe::new`]).
#[derive(Debug)]
pub struct WaterProbe {
    shader: ProgramShader,
    stage: StageBinding,
    /// The material's vertex program and its binding, which give the pixel program its inputs.
    vertex: (Shader, StageBinding),
    /// The pixel inputs the vertex program gave in the last frame a run was in, and what that run's vertex stage counted:
    /// runs that differ only in their texture coordinate share them.
    given: std::cell::RefCell<Option<(FrozenFrame, VertexGiven)>>,
    /// The frame's values the runs so far stated rather than their scenes gave ([`ProbeRun::stated`]), by reason: the
    /// most any one run stated.
    stated: std::cell::RefCell<BTreeMap<String, u64>>,
    slots: BTreeMap<u32, Slot>,
    /// The structured buffers the binding binds, by register: its light tables empty, its light lists of no lights.
    structured: BTreeMap<u32, Structured>,
    /// The texture registers the caller answers ([`WaterProbe::pixel`]): the binding's material textures.
    pub material: Vec<u32>,
    /// What the binding and the frame counted, by reason.
    pub policies: BTreeMap<String, u64>,
}

/// What a vertex program gives the pixel program in one frame: each input by its label, and the vertex stage's policies.
type VertexGiven = (BTreeMap<(String, u32), Input>, BTreeMap<String, u64>);

impl WaterProbe {
    /// The forward pixel program `shader` as `stage` (its `core.shader_binding`'s pixel stage) binds it, its inputs
    /// written by the vertex program `vertex` (the entity `vertex_entity`).
    pub fn new(
        shader: &Shader,
        stage: &StageBinding,
        vertex: &Shader,
        vertex_entity: primase_core::Entity,
    ) -> Result<Self, String> {
        let executor = layer_bake::program_executor(shader.format, &shader.bytecode)
            .map_err(|e| format!("its program does not lift: {e}"))?;
        let interface: &Interface = iridia_exec::PixelShader::interface(&executor);
        let mut slots = BTreeMap::new();
        let mut policies = BTreeMap::new();
        for binding in &stage.textures {
            let resource = interface
                .resource(binding.resource)
                .ok_or("a texture binding of a resource the program lacks")?;
            if !matches!(resource.kind, ResourceKind::Texture { .. }) {
                continue;
            }
            let slot = register(resource).map_err(|e| e.to_string())?;
            let read = match &binding.source {
                TextureSource::Frame(FrameTexture::SceneColour) => Slot::SceneColour,
                TextureSource::Frame(FrameTexture::SceneDepth) => Slot::SceneDepth,
                TextureSource::Frame(FrameTexture::ShadowMap) => {
                    *policies.entry(SHADOW_LIT.to_owned()).or_default() += 1;
                    Slot::Lit
                }
                TextureSource::Frame(other) => {
                    *policies
                        .entry(format!(
                            "the frame's {other:?} read as zero: the probe's scene has none"
                        ))
                        .or_default() += 1;
                    Slot::Value([0.; 4])
                }
                TextureSource::Default { value, why } => {
                    *policies.entry(why.clone()).or_default() += 1;
                    Slot::Value(*value)
                }
                TextureSource::Texture { .. } => Slot::Material,
                TextureSource::Missing { why } => return Err(format!("texture t{slot} is missing: {why}")),
                TextureSource::Target { view } => {
                    return Err(format!("texture t{slot} is render graph target {}", view.target));
                }
            };
            slots.insert(slot, read);
        }
        let material = slots
            .iter()
            .filter(|(_, s)| **s == Slot::Material)
            .map(|(r, _)| *r)
            .collect();
        let structured = structured_buffers(interface, stage, &mut policies)?;
        let bound = resolve_vertex(vertex, vertex_entity)
            .map_err(|e| format!("its vertex program does not bind: {e}"))?;
        if let Some(why) = bound.undecided {
            *policies.entry(why).or_default() += 1;
        }
        let vertex_stage = bound.stage;
        Ok(WaterProbe {
            shader: executor,
            stage: stage.clone(),
            vertex: (vertex.clone(), vertex_stage),
            given: std::cell::RefCell::new(None),
            stated: std::cell::RefCell::new(BTreeMap::new()),
            slots,
            structured,
            material,
            policies,
        })
    }

    /// What the vertex program gives the pixel program in `frame` ([`VertexGiven`]), run once for each frame in turn.
    fn given(&self, frame: &FrozenFrame) -> Result<VertexGiven, String> {
        if let Some((at, given)) = self.given.borrow().as_ref()
            && at == frame
        {
            return Ok(given.clone());
        }
        let (shader, stage) = &self.vertex;
        let vertex = layer_bake::VertexStage::new(
            shader,
            stage,
            layer_bake::lifted_program(&self.shader),
            false,
            frame,
        )
        .map_err(|e| format!("its vertex program: {e}"))?;
        let inputs = vertex
            .pixel_inputs(
                PixelShader::interface(&self.shader),
                &layer_bake::Inputs::default(),
            )
            .map_err(|e| format!("its vertex program's outputs: {e}"))?;
        let given = (inputs, vertex.policies);
        *self.given.borrow_mut() = Some((*frame, given.clone()));
        Ok(given)
    }

    /// The frame's values its runs so far stated rather than their scenes gave them (no sun, a source's default, a
    /// constant of no known role, what the vertex stage stated), by reason: the most any one run stated.
    pub fn stated(&self) -> BTreeMap<String, u64> {
        self.stated.borrow().clone()
    }

    /// Whether its binding reads the scene behind its surface ([`reads_behind`]).
    pub fn reads_behind(&self) -> bool {
        reads_behind(&self.stage)
    }

    /// One pixel of `scene`, each material texture's read answered by `material` (its register and the request).
    pub fn pixel(
        &self,
        scene: &ProbeScene,
        material: &mut dyn FnMut(u32, &ImageSample<u32>) -> [f32; 4],
    ) -> Result<ProbeRun, String> {
        let interface = iridia_exec::PixelShader::interface(&self.shader);
        let frame = FrozenFrame {
            time: scene.time,
            hour: Some(scene.hour),
            camera: Some(FrameCamera {
                eye: scene.camera,
                target: [0.; 3],
                depth: DEPTH,
            }),
            sun: scene.sun,
            ..FrozenFrame::static_bake(scene.time)
        };
        let mut policies = BTreeMap::new();
        let constants =
            layer_bake::program_constants(&self.shader, interface, &self.stage, &frame, &mut policies)
                .map_err(|e| format!("its constants: {e}"))?;
        let (given, vertex_policies) = self.given(&frame)?;
        policies.extend(vertex_policies);
        let depth = scene.surface_depth();
        let uv = scene.uv.map(f32::to_bits);
        let mut registers = vec![[0u32; 4]; self.shader.input_count()];
        for v in interface.inputs.iter().filter(|v| v.used) {
            let d3d = v
                .provenance
                .d3d
                .ok_or_else(|| format!("input {}{} has no register", v.label.name, v.label.index))?;
            let lanes: [u32; 4] = match (
                given.get(&(v.label.name.clone(), v.label.index)),
                v.label.name.to_ascii_uppercase().as_str(),
            ) {
                (Some(Input::Lanes(lanes)), _) => lanes.map(|l| match l {
                    Lane::Bits(b) => b,
                    Lane::U => uv[0],
                    Lane::V => uv[1],
                }),
                (Some(Input::Constant(c)), _) => c.map(f32::to_bits),
                (Some(Input::GridUv), _) => [uv[0], uv[1], 0, 0],
                // the middle of the one-pixel view, at the surface's device depth and its depth along the view axis
                (None, "SV_POSITION") => [0.5, 0.5, DEPTH.device_depth(depth), depth].map(f32::to_bits),
                (None, "SV_ISFRONTFACE") => [u32::MAX, 0, 0, 0],
                (None, _) => {
                    return Err(format!(
                        "input {}{}: the vertex program writes no value for it",
                        v.label.name, v.label.index
                    ));
                }
            };
            let row = registers
                .get_mut(d3d.register as usize)
                .ok_or("an input past the program's registers")?;
            for (k, lane) in (0..4).filter(|c| d3d.mask & (1 << c) != 0).enumerate() {
                row[lane] = lanes[k];
            }
        }
        let behind = DEPTH.device_depth(scene.behind_depth);
        let colour = scene.behind;
        let slots = &self.slots;
        let mut reads = Probed {
            structured: &self.structured,
            reads: AnsweredReads::new(Reads {
                answer: |request: &ImageSample<u32>| -> Result<[f32; 4], String> {
                    Ok(match slots.get(&request.texture) {
                        Some(Slot::SceneColour) => [colour[0], colour[1], colour[2], 1.],
                        Some(Slot::SceneDepth) => [behind; 4],
                        Some(Slot::Lit) => [1.; 4],
                        Some(Slot::Value(v)) => *v,
                        Some(Slot::Material) => material(request.texture, request),
                        None => return Err(format!("t{} is read and not bound", request.texture)),
                    })
                },
            }),
        };
        let buffers: Vec<(u32, &[[u32; 4]])> = constants
            .buffers()
            .iter()
            .map(|(slot, rows)| (*slot, rows.as_slice()))
            .collect();
        let run = self
            .shader
            .execute(&registers, &buffers, &mut reads)
            .map_err(|e| format!("its run fails at {}: {}", e.at, e.detail))?;
        let first = run.outputs.first().ok_or("the program writes no target")?;
        Ok(ProbeRun {
            colour: first.map(f32::from_bits),
            samples: reads.reads.samples,
            stated: {
                let mut kept = self.stated.borrow_mut();
                for (reason, n) in &policies {
                    let most = kept.entry(reason.clone()).or_default();
                    *most = (*most).max(*n);
                }
                policies
            },
        })
    }
}

/// What a probe counts where the binding binds the scene's lights: the probe's scene has none but the sun.
pub const NO_LOCAL_LIGHTS: &str =
    "the scene's local lights: the probe's scene has none (empty light tables, light lists of no lights)";

/// The structured buffers of `stage`, by register: each light table empty, the light lists packed for no lights in
/// every tile the lookup holds. A buffer of another source refuses the program: the probe's scene states no draw.
fn structured_buffers(
    interface: &Interface,
    stage: &StageBinding,
    policies: &mut BTreeMap<String, u64>,
) -> Result<BTreeMap<u32, Structured>, String> {
    let mut out = BTreeMap::new();
    let lookup = stage.buffers.iter().find_map(|b| match &b.source {
        BufferSource::LightLists(LightListPart::Lookup(l)) => Some(l),
        _ => None,
    });
    let indices = stage.buffers.iter().find_map(|b| match &b.source {
        BufferSource::LightLists(LightListPart::Indices(i)) => Some(i),
        _ => None,
    });
    let lists = match (lookup, indices) {
        (Some(l), Some(i)) => Some(pack_light_lists(l, i, &[], &[], l.capacity)?),
        _ => None,
    };
    for b in &stage.buffers {
        let resource = interface
            .resource(b.resource)
            .ok_or("a buffer binding of a resource the program lacks")?;
        let slot = register(resource).map_err(|e| e.to_string())?;
        let ResourceKind::Buffer(BufferKind::Structured { stride }) = resource.kind else {
            // a typed light table is read by texel loads, answered as an empty table's ([`Reads::load`])
            continue;
        };
        let bytes = match (&b.source, &lists) {
            (BufferSource::Lights(_), _) => Vec::new(),
            (BufferSource::LightLists(LightListPart::Lookup(_)), Some(p)) => p.lookup.clone(),
            (BufferSource::LightLists(LightListPart::Indices(_)), Some(p)) => p.indices.clone(),
            (other, _) => {
                return Err(format!(
                    "buffer t{slot} ({}) is {other:?}: the probe's scene gives no such buffer",
                    resource.provenance.name
                ));
            }
        };
        out.insert(slot, Structured { stride, bytes });
    }
    if !out.is_empty() {
        *policies.entry(NO_LOCAL_LIGHTS.to_owned()).or_default() += 1;
    }
    Ok(out)
}

/// A run's reads: textures by [`Reads`], structured buffers by the probe's ([`structured_buffers`]); an element past a
/// buffer's end reads 0, as D3D reads it.
struct Probed<'a, A> {
    structured: &'a BTreeMap<u32, Structured>,
    reads: AnsweredReads<u32, A>,
}

impl<A: Answer<u32>> Sampler for Probed<'_, A> {
    fn sample(&mut self, request: Sample) -> Result<[f32; 4], String> {
        Sampler::sample(&mut self.reads, request)
    }
    fn sample_image(&mut self, request: ImageSample<u32>) -> Result<[f32; 4], String> {
        Sampler::sample_image(&mut self.reads, request)
    }
    fn load_image(&mut self, request: ImageLoad<u32>) -> Result<[u32; 4], String> {
        Sampler::load_image(&mut self.reads, request)
    }
    fn query_image(&mut self, request: ImageQuery<u32>) -> Result<[u32; 4], String> {
        Sampler::query_image(&mut self.reads, request)
    }
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        let b = self
            .structured
            .get(&slot)
            .ok_or_else(|| format!("t{slot}: a structured buffer the binding does not bind"))?;
        let at = u64::from(element) * u64::from(b.stride) + u64::from(offset);
        let word = usize::try_from(at)
            .ok()
            .and_then(|at| b.bytes.get(at..at + 4))
            .map_or(0, |w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        Ok(word)
    }
    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        let b = self.structured.get(&slot)?;
        u32::try_from(b.bytes.len() / b.stride.max(1) as usize).ok()
    }
}

/// What a probe counts where the binding binds the frame's shadow map: the probe's surface is lit, every comparison
/// passing.
pub const SHADOW_LIT: &str = "shadow map read as lit everywhere: the probe's scene casts no shadow";

/// The probe's answers: samples by the closure, and a texel load of a light table answered as an empty one (the frame's
/// light counts are zero, so no record is read for its value).
struct Reads<F> {
    answer: F,
}

impl<F: FnMut(&ImageSample<u32>) -> Result<[f32; 4], String>> Answer<u32> for Reads<F> {
    fn sample(&mut self, request: &ImageSample<u32>) -> Result<[f32; 4], String> {
        (self.answer)(request)
    }
    fn load(&mut self, _request: &iridia_exec::ImageLoad<u32>) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
}
