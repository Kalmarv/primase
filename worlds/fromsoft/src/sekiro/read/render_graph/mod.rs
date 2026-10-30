//! The frame the game's renderer draws, as `core.render_graph` ([`primase_core::render_graph`]), read with an area
//! ([`super::area::Options::render_graph`]).
//!
//! The renderer is the game's code, not data, so the frame's steps are a table per game (`sekiro.json`, `elden.json`,
//! [`table`]), taken from recorded frames of the running game (Sekiro: nine frames of four sessions; Elden Ring: seven of
//! two): each step in the order the frames draw it, with the frames that had it (a step only some frames have stands in
//! its place all the same, with what triggers it where the frames show it, or the frames that had it), how many times
//! it ran, its programs (by their names in the game's engine binders), what each of its texture and buffer registers
//! read (one of the frame's targets or buffers, or the renderer's own texture), the sampler state it bound at each
//! sampler register, its outputs and its pipeline state. The programs are the install's: read here from
//! `/shader/<folder>.shaderbnd.dcx` by name, each a `core.shader`, and which sampler each texture is read through is
//! read from the program's own instructions ([`crate::passes::sampler_pairs`]).
//!
//! What each program reads gets a role only where it is known: a target or buffer the frame makes; the renderer's own
//! named constants that `core` has words for (the camera's matrices and position, the target size, the time, the
//! pre-exposure); the full-screen triangle's corners where a recorded frame holds the vertex buffer's bytes (Sekiro's).
//! Everything else is `Unknown` with what was examined, or `Missing` (the renderer's own textures: noise, look-up tables,
//! the textures a frame keeps from an earlier one that no recorded frame writes). The scene's draws are steps of a
//! bucket, drawn by the materials' own bindings; the effects are the effects slot, in the places the frames draw them.
//! Sekiro's light records are `core` light tables the consumer packs from the scene's lights
//! ([`crate::light_records`]), and their counts and tile constants frame parameters; the per-tile light lists a step
//! reads are the frame's buffers the light-organize step writes.
mod table;

use crate::light_records as lights;

use crate::game::Shading;
use crate::invalid;
use crate::passes::sampler_pairs::{self, SamplerPairs};
use crate::sekiro::source::draw_of;
use fromsoft_formats::names::fold;
use fromsoft_formats::{Archive, Binder, Game, Result, dcx::Decoder};
use iridia::ProgramGraph;
use iridia::matrix::lines_at;
use iridia_interface as iri;
use iridia_interface::{D3dClass, Interface, ResourceKind, Stage, SystemValue};
use primase_core::render_graph::{
    BufferSize, DepthView, DrawBucket, DrawSorting, FeatureSlot, Groups, Items, Outputs, PassWork, Presence,
    Region, RenderBuffer, RenderGraph, RenderPass, RenderTarget, Rounding, Runs, SceneMembers,
    ScenePlacements, SceneView, TargetFormat, TargetShape, TargetSize, TargetView, Trigger,
};
use primase_core::shader::{
    BufferBinding, BufferSource, ConstantBinding, ConstantRole, InputBinding, InputRole, LightValue,
    OutputBinding, OutputRole, SamplerBinding, ScreenTriangleValue, Shader, StageBinding, TextureBinding,
    TextureSource, VariantKey, ViewParameter,
};
use primase_core::{Name, World};
use std::collections::{BTreeMap, BTreeSet};

const FORMAT: &str = "render graph";

/// Why a scene step's order of draws is not stated: the recorded frames hold the draws in the order the game drew them,
/// and the rule that ordered them is not read.
const SORTING_NOT_READ: &str = "the recorded frames hold the draws in the order the game drew them; the rule that orders them is not read";

/// What the draw of a scene step of `bucket` draws: the G-buffer's, the forward draws' and the sky's materials are the
/// ones each material's `core.step_bindings` binds to the draw ([`crate::sekiro::passes::resolve_shaders`] by the
/// material's pass, [`crate::sekiro::passes::sky`] for the sky's), at every placement; what the shadow casters, the
/// velocity draws and a decal draw draw is not read.
fn members_of(bucket: DrawBucket) -> SceneMembers {
    let not_read = |why: &str| SceneMembers::NotRead { why: why.to_owned() };
    match bucket {
        DrawBucket::Deferred | DrawBucket::Forward | DrawBucket::Sky => SceneMembers::Stated {
            placements: ScenePlacements::All,
        },
        DrawBucket::ShadowCaster => not_read(
            "which surfaces the game's shadow casters draw, and through which of their materials' programs, is not read",
        ),
        DrawBucket::Velocity => {
            not_read("which surfaces the game's velocity draws draw, and through which programs, is not read")
        }
        DrawBucket::Decal => {
            not_read("which surfaces the game's decal draws draw, and through which programs, is not read")
        }
    }
}

/// What the read of the frame's render graph made and left ([`super::area::Options::render_graph`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Whether the graph was asked for.
    pub asked: bool,
    /// The recorded frames the game's table comes from.
    pub frames: usize,
    /// The graph's steps (a group's own steps not counted), targets and buffers.
    pub steps: usize,
    pub targets: usize,
    pub buffers: usize,
    /// The programs read from the engine binders (`core.shader` entities).
    pub programs: usize,
    /// Texture slots bound `Missing`: the renderer's own textures, and those a frame keeps from an earlier frame that no
    /// recorded frame writes, by what the frames bind there (format and size).
    pub engine_textures: BTreeMap<String, usize>,
    /// Declared texture or buffer registers the recorded frames bind nothing at.
    pub unbound_slots: usize,
    /// Sampler states stated (a declared sampler register and the state every run of its step binds there), and the
    /// declared sampler registers with none: the frames bind nothing there, or different states in different runs.
    pub samplers: usize,
    pub unbound_samplers: usize,
    /// Texture slots the program reads through more than one sampler: their binding names none (each sampler's state is
    /// stated).
    pub textures_through_several_samplers: usize,
    /// Texture reads of the programs whose texture or sampler is not one register (an array indexed at run time).
    pub untraced_reads: usize,
    /// Buffer slots bound `Unknown`: the renderer's own buffers (Elden Ring's light parameters) and those the frames bind
    /// nothing at.
    pub unknown_buffers: usize,
    /// Buffer slots bound as the scene's lights (Sekiro's light records), and the fields of their tables `core.light`
    /// does not give (one per field per slot).
    pub light_tables: usize,
    pub unknown_light_fields: usize,
    /// Buffer slots bound as per-tile light lists the consumer packs (none where the frames show the organize step
    /// writing them).
    pub light_lists: usize,
    /// Constants with no core role (the renderer's own parameters), and those with one.
    pub unknown_constants: usize,
    pub constants: usize,
    /// Vertex inputs whose vertex buffer no recorded frame holds the bytes of.
    pub unknown_inputs: usize,
    /// Unordered-access registers a dispatch writes that its program does not declare.
    pub undeclared_writes: usize,
    /// What the recorded frames drew that the graph leaves out (the interface after the picture, occlusion proxies, an
    /// effect's own buffers), and how many.
    pub left_out: BTreeMap<String, u64>,
    /// The order the frames drew each sorted run of sky, forward and effect draws in (the graph states each as a step of
    /// its own), and how many runs had it.
    pub forward_phase: BTreeMap<String, u64>,
    /// The view size the frames were recorded at.
    pub view: [u32; 2],
}

impl Report {
    /// What the graph left unknown, as notices.
    pub fn notices(&self) -> Vec<primase_core::convert::Notice> {
        use primase_core::convert::{Notice, NoticeKind};
        let stage = "fromsoft reader";
        if !self.asked {
            return Vec::new();
        }
        let mut out = vec![Notice::new(
            stage,
            NoticeKind::Selection,
            format!(
                "the render graph: {} steps, {} targets, {} buffers and {} programs, from {} frames recorded at {} x {}",
                self.steps,
                self.targets,
                self.buffers,
                self.programs,
                self.frames,
                self.view[0],
                self.view[1]
            ),
            1,
        )];
        for (what, n) in &self.engine_textures {
            out.push(Notice::new(
                stage,
                NoticeKind::Unsupported,
                format!("render graph texture slots bound missing: the renderer's own {what}"),
                *n as u64,
            ));
        }
        let unknown = [
            (
                self.unbound_slots,
                "render graph registers the recorded frames bind nothing at",
            ),
            (
                self.unknown_buffers,
                "render graph buffer slots unknown (the renderer's own buffers)",
            ),
            (self.unknown_constants, "render graph constants with no core role"),
            (
                self.unknown_light_fields,
                "render graph light-record fields core.light does not give (written 0)",
            ),
            (
                self.unknown_inputs,
                "render graph vertex inputs whose vertex buffer no recorded frame holds",
            ),
            (
                self.undeclared_writes,
                "render graph unordered-access writes the program does not declare",
            ),
            (
                self.unbound_samplers,
                "render graph sampler registers with no recorded state",
            ),
            (
                self.untraced_reads,
                "render graph texture reads whose texture or sampler is not one register",
            ),
        ];
        for (n, what) in unknown {
            if n > 0 {
                out.push(Notice::new(stage, NoticeKind::Unsupported, what, n as u64));
            }
        }
        for (what, n) in &self.left_out {
            out.push(Notice::new(
                stage,
                NoticeKind::Selection,
                format!("render graph leaves out {what}"),
                *n,
            ));
        }
        for (order, n) in &self.forward_phase {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                format!(
                    "a sorted run the frames drew as `{order}`, stated as the sky, forward and effects steps"
                ),
                *n,
            ));
        }
        out
    }
}

/// A program of the graph: its `core.shader` entity, interface, value graph (or why there is none) and which sampler
/// each of its textures is read through.
struct Program {
    entity: primase_core::Entity,
    interface: Interface,
    graph: std::result::Result<ProgramGraph, String>,
    pairs: SamplerPairs,
}

/// Read the game's render graph: its programs from the install's engine binders, its steps from the game's table, as the
/// world's `core.render_graph`. Refused: a table program the install does not hold or that does not lift, and a table
/// whose graph does not check.
pub(crate) fn read(
    world: &mut World,
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    report: &mut Report,
) -> Result<()> {
    report.asked = true;
    let t = table::table(game).map_err(|e| invalid(FORMAT, 0, e))?;
    let expected = match game {
        Game::Sekiro => "sekiro",
        Game::EldenRing => "elden",
    };
    if t.game != expected {
        return Err(invalid(FORMAT, 0, format!("the {game:?} table is {}'s", t.game)));
    }
    report.frames = t.frames.len();
    report.view = t.view;
    report.left_out = t.left_out.clone();
    report.forward_phase = t.forward_phase.clone();
    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in &t.steps {
        programs_of(s, &mut wanted).map_err(|e| invalid(FORMAT, 0, e))?;
    }
    let mut programs: BTreeMap<String, Program> = BTreeMap::new();
    for (folder, files) in &wanted {
        let path = format!("/shader/{folder}.shaderbnd.dcx");
        if !archive.contains(&path) {
            return Err(invalid(FORMAT, 0, format!("the install holds no {path}")));
        }
        let keys: BTreeSet<String> = files.iter().map(|f| fold(f)).collect();
        let binder = Binder::read_where(&archive.read(&path, decoder)?, decoder, &|name| {
            keys.contains(&fold(leaf(name)))
        })?;
        for file in binder.files {
            let stored = leaf(file.name.as_deref().unwrap_or("")).to_owned();
            let Some(f) = files.iter().find(|f| fold(f) == fold(&stored)) else {
                continue;
            };
            let full = format!("{folder}/{f}");
            let lifted = crate::passes::program_reads::lift(game.shader_format(), &file.bytes)
                .map_err(|e| invalid(FORMAT, 0, format!("{full}: {e}")))?;
            let graph = lifted.graph();
            let interface = lifted.interface.clone();
            let pairs = sampler_pairs::pairs(game.shader_format(), &file.bytes).map_err(|e| {
                invalid(
                    FORMAT,
                    0,
                    format!("{full}: which sampler each texture is read through: {e}"),
                )
            })?;
            report.untraced_reads = report.untraced_reads.saturating_add(pairs.untraced);
            let entity = world.spawn();
            world.insert(entity, Name(full.clone()));
            world.insert(
                entity,
                Shader {
                    format: game.shader_format(),
                    bytecode: file.bytes,
                    interface: interface.clone(),
                },
            );
            programs.insert(
                full,
                Program {
                    entity,
                    interface,
                    graph,
                    pairs,
                },
            );
        }
        if let Some(missing) = files
            .iter()
            .find(|f| !programs.contains_key(&format!("{folder}/{f}")))
        {
            return Err(invalid(FORMAT, 0, format!("{path} holds no {missing}")));
        }
    }
    report.programs = programs.len();
    let targets = t
        .targets
        .iter()
        .map(target)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| invalid(FORMAT, 0, e))?;
    let buffers: Vec<RenderBuffer> = t.buffers.iter().map(buffer).collect();
    let lower = Lower {
        game,
        table: &t,
        programs: &programs,
    };
    let mut cascades: BTreeMap<u32, u8> = BTreeMap::new();
    let mut passes = Vec::with_capacity(t.steps.len());
    for s in &t.steps {
        passes.push(
            lower
                .step(s, &mut cascades, report)
                .map_err(|e| invalid(FORMAT, 0, format!("step {}: {e}", s.name)))?,
        );
    }
    let graph = RenderGraph {
        targets,
        buffers,
        passes,
        picture: t.picture,
        left_out: Vec::new(),
    };
    graph.check().map_err(|e| invalid(FORMAT, 0, e))?;
    report.steps = graph.passes.len();
    report.targets = graph.targets.len();
    report.buffers = graph.buffers.len();
    world.insert_resource(graph);
    Ok(())
}

/// The last path component, either separator.
fn leaf(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or("")
}

/// The programs a step names, by folder.
fn programs_of(
    s: &table::Step,
    out: &mut BTreeMap<String, BTreeSet<String>>,
) -> std::result::Result<(), String> {
    let mut add = |p: &str| -> std::result::Result<(), String> {
        let (folder, file) = p
            .split_once('/')
            .ok_or_else(|| format!("step {}: program {p} names no folder", s.name))?;
        out.entry(folder.to_owned()).or_default().insert(file.to_owned());
        Ok(())
    };
    match &s.work {
        table::Work::Screen(sc) => {
            for v in &sc.vertex {
                add(&v.program)?;
            }
            for p in &sc.pixel {
                add(p)?;
            }
        }
        table::Work::Compute(c) => add(&c.program)?,
        table::Work::Group { steps } => {
            for s in steps {
                programs_of(s, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn target(t: &table::Target) -> std::result::Result<RenderTarget, String> {
    let format = match t.format.as_deref() {
        Some("rgba32_float") => TargetFormat::Rgba32Float,
        Some("rgba16_float") => TargetFormat::Rgba16Float,
        Some("rgba16_unorm") => TargetFormat::Rgba16Unorm,
        Some("rg32_float") => TargetFormat::Rg32Float,
        Some("rg16_float") => TargetFormat::Rg16Float,
        Some("r32_float") => TargetFormat::R32Float,
        Some("r32_uint") => TargetFormat::R32Uint,
        Some("r16_float") => TargetFormat::R16Float,
        Some("r16_unorm") => TargetFormat::R16Unorm,
        Some("rgb10_a2_unorm") => TargetFormat::Rgb10A2Unorm,
        Some("rg11_b10_float") => TargetFormat::Rg11B10Float,
        Some("rgba8_unorm") => TargetFormat::Rgba8Unorm,
        Some("rgba8_unorm_srgb") => TargetFormat::Rgba8UnormSrgb,
        Some("rg8_unorm") => TargetFormat::Rg8Unorm,
        Some("r8_unorm") => TargetFormat::R8Unorm,
        Some("r8_uint") => TargetFormat::R8Uint,
        Some("depth32_stencil8") => TargetFormat::Depth32Stencil8,
        Some("depth32") => TargetFormat::Depth32,
        Some("depth24_stencil8") => TargetFormat::Depth24Stencil8,
        Some("depth16") => TargetFormat::Depth16,
        other => return Err(format!("target {}: format {other:?}", t.name)),
    };
    let size = match t.size {
        table::Size::View {
            numerator,
            denominator,
            depth,
            ref rounding,
        } => TargetSize::View {
            numerator,
            denominator,
            depth,
            rounding: match rounding {
                table::Rounding::Down => Rounding::Down,
                table::Rounding::Up => Rounding::Up,
            },
        },
        table::Size::Fixed { width, height, depth } => TargetSize::Fixed { width, height, depth },
    };
    let shape = match t.shape {
        table::Shape::Plane => TargetShape::Plane,
        table::Shape::Volume => TargetShape::Volume,
        table::Shape::Cube => TargetShape::Cube,
        table::Shape::Array { layers } => TargetShape::Array { layers },
    };
    Ok(RenderTarget {
        name: t.name.clone(),
        format,
        size,
        shape,
        mips: t.mips,
        history: t.history,
        shared: t.shared,
    })
}

fn buffer(b: &table::Buffer) -> RenderBuffer {
    RenderBuffer {
        name: b.name.clone(),
        size: match b.size {
            table::BufferSize::Fixed { bytes } => BufferSize::Fixed { bytes },
            table::BufferSize::Recorded { bytes, view } => BufferSize::Recorded { bytes, view },
        },
        stride: b.stride,
        elements: b.elements,
        history: b.history,
        shared: b.shared,
    }
}

fn view(v: &table::View) -> TargetView {
    TargetView {
        target: v.target,
        mip: v.mip,
        first_layer: v.first_layer,
        layers: v.layers,
    }
}

fn region(r: [u32; 4]) -> Region {
    Region {
        x: r[0],
        y: r[1],
        width: r[2],
        height: r[3],
    }
}

fn outputs(o: &table::Outputs) -> Outputs {
    Outputs {
        colour: o.colour.iter().map(|(slot, v)| (*slot, view(v))).collect(),
        depth: o.depth.as_ref().map(|d| DepthView {
            view: view(&d.view),
            read_only_depth: d.read_only_depth,
            read_only_stencil: d.read_only_stencil,
        }),
    }
}

fn stage_of(name: &str) -> std::result::Result<Stage, String> {
    match name {
        "vertex" => Ok(Stage::Vertex),
        "pixel" => Ok(Stage::Pixel),
        "compute" => Ok(Stage::Compute),
        other => Err(format!("a slot of stage {other}")),
    }
}

/// The D3D comparison (`D3D11_COMPARISON_FUNC` and `D3D12_COMPARISON_FUNC` share the numbers).
fn compare(n: u32) -> std::result::Result<iri::Compare, String> {
    Ok(match n {
        1 => iri::Compare::Never,
        2 => iri::Compare::Less,
        3 => iri::Compare::Equal,
        4 => iri::Compare::LessEqual,
        5 => iri::Compare::Greater,
        6 => iri::Compare::NotEqual,
        7 => iri::Compare::GreaterEqual,
        8 => iri::Compare::Always,
        other => return Err(format!("comparison {other}")),
    })
}

/// A sampler the table records (D3D's numbers) as the interface's state. The filter (`D3D12_FILTER`): bit 0 a linear
/// mip filter, bit 2 linear magnification, bit 4 linear minification, bit 6 anisotropic, bits 7 and 8 the reduction (0
/// a plain filter, 1 a comparison against `compare`; a minimum or maximum, which the state does not state, is refused).
/// The border colour and the level range are stated as recorded; the anisotropy where the filter is anisotropic, the
/// bias where it is not zero.
fn sampler_state(s: &table::Sampler) -> std::result::Result<iri::SamplerState, String> {
    let f = s.filter;
    if f & !0x1d5 != 0 {
        return Err(format!(
            "sampler filter {f:#x}, which has bits D3D does not define"
        ));
    }
    let level = |bit: u32| {
        if f >> bit & 1 == 1 {
            iri::Filter::Linear
        } else {
            iri::Filter::Nearest
        }
    };
    let address = |a: u32| -> std::result::Result<iri::AddressMode, String> {
        Ok(match a {
            1 => iri::AddressMode::Repeat,
            2 => iri::AddressMode::Mirror,
            3 => iri::AddressMode::Clamp,
            4 => iri::AddressMode::Border,
            5 => iri::AddressMode::MirrorOnce,
            other => return Err(format!("sampler address mode {other}")),
        })
    };
    let compare = match f >> 7 & 3 {
        0 => None,
        1 => Some(compare(s.compare)?),
        _ => return Err(format!("sampler filter {f:#x}: a minimum or maximum filter")),
    };
    let max_anisotropy = if f & 0x40 != 0 {
        Some(u8::try_from(s.anisotropy).map_err(|_| format!("sampler anisotropy {}", s.anisotropy))?)
    } else {
        None
    };
    let [u, v, w] = s.address;
    let state = iri::SamplerState {
        wrap: [Some(address(u)?), Some(address(v)?), Some(address(w)?)],
        filters: iri::Filters {
            mag: Some(level(2)),
            min: Some(level(4)),
            mip: Some(if f & 1 == 1 {
                iri::MipFilter::Linear
            } else {
                iri::MipFilter::Nearest
            }),
        },
        max_anisotropy,
        compare,
        mip_lod_bias: (s.lod_bias != 0.0).then_some(s.lod_bias),
        border: Some(s.border),
        lod: Some(s.lod),
    };
    state.verify()?;
    Ok(state)
}

fn stencil_op(n: u32) -> std::result::Result<iri::StencilOp, String> {
    Ok(match n {
        1 => iri::StencilOp::Keep,
        2 => iri::StencilOp::Zero,
        3 => iri::StencilOp::Replace,
        4 => iri::StencilOp::IncrementClamp,
        5 => iri::StencilOp::DecrementClamp,
        6 => iri::StencilOp::Invert,
        7 => iri::StencilOp::IncrementWrap,
        8 => iri::StencilOp::DecrementWrap,
        other => return Err(format!("stencil operation {other}")),
    })
}

fn blend_factor(n: u32) -> std::result::Result<iri::BlendFactor, String> {
    Ok(match n {
        1 => iri::BlendFactor::Zero,
        2 => iri::BlendFactor::One,
        3 => iri::BlendFactor::SrcColour,
        4 => iri::BlendFactor::OneMinusSrcColour,
        5 => iri::BlendFactor::SrcAlpha,
        6 => iri::BlendFactor::OneMinusSrcAlpha,
        7 => iri::BlendFactor::DstAlpha,
        8 => iri::BlendFactor::OneMinusDstAlpha,
        9 => iri::BlendFactor::DstColour,
        10 => iri::BlendFactor::OneMinusDstColour,
        other => return Err(format!("blend factor {other}, which the interface does not name")),
    })
}

fn blend_op(n: u32) -> std::result::Result<iri::BlendOp, String> {
    Ok(match n {
        1 => iri::BlendOp::Add,
        2 => iri::BlendOp::Subtract,
        3 => iri::BlendOp::ReverseSubtract,
        4 => iri::BlendOp::Min,
        5 => iri::BlendOp::Max,
        other => return Err(format!("blend operation {other}")),
    })
}

/// The state the table records (D3D's numbers) as the interface's, for a draw into `targets` render targets: a
/// disabled depth test is D3D's (no test and no write); the stencil reference is set per draw (none stated); a field the
/// runs of a repeated step differ in is not stated.
fn render_state(s: &table::State, targets: usize) -> std::result::Result<iri::RenderState, String> {
    let mut out = iri::RenderState::default();
    if let Some(blend) = &s.blend {
        for (i, b) in blend.iter().take(targets.min(8)).enumerate() {
            let [enable, src, dst, op, src_a, dst_a, op_a, mask] = *b;
            let blend = if enable == 0 {
                None
            } else {
                Some(iri::Blend {
                    colour: iri::BlendEquation {
                        source: blend_factor(src)?,
                        destination: blend_factor(dst)?,
                        op: blend_op(op)?,
                    },
                    alpha: iri::BlendEquation {
                        source: blend_factor(src_a)?,
                        destination: blend_factor(dst_a)?,
                        op: blend_op(op_a)?,
                    },
                })
            };
            let write_mask = u8::try_from(mask).map_err(|_| format!("write mask {mask}"))?;
            if let Some(slot) = out.targets.get_mut(i) {
                *slot = Some(iri::TargetState { blend, write_mask });
            }
        }
        out.blend = out.targets[0].and_then(|t| t.blend);
    }
    out.depth = match &s.depth {
        Some(d) if d.enable => Some(iri::DepthState {
            test: compare(d.func)?,
            write: d.write,
        }),
        Some(_) => Some(iri::DepthState {
            test: iri::Compare::Always,
            write: false,
        }),
        None => None,
    };
    if let Some(st) = &s.stencil {
        let face = |f: [u32; 4]| -> std::result::Result<iri::StencilFace, String> {
            Ok(iri::StencilFace {
                fail: stencil_op(f[0])?,
                depth_fail: stencil_op(f[1])?,
                pass: stencil_op(f[2])?,
                compare: compare(f[3])?,
            })
        };
        out.stencil = Some(iri::Stencil {
            read_mask: u8::try_from(st.read_mask)
                .map_err(|_| format!("stencil read mask {}", st.read_mask))?,
            write_mask: u8::try_from(st.write_mask)
                .map_err(|_| format!("stencil write mask {}", st.write_mask))?,
            reference: st.reference.and_then(|r| u8::try_from(r).ok()),
            front: face(st.front)?,
            back: face(st.back)?,
        });
    }
    out.cull = match s.cull {
        Some(1) => Some(iri::Cull::None),
        Some(2) => Some(iri::Cull::Front),
        Some(3) => Some(iri::Cull::Back),
        Some(other) => return Err(format!("cull mode {other}")),
        None => None,
    };
    out.fill = match s.fill {
        Some(2) => Some(iri::Fill::Wireframe),
        Some(3) => Some(iri::Fill::Solid),
        Some(other) => return Err(format!("fill mode {other}")),
        None => None,
    };
    out.front_counter_clockwise = s.front_ccw;
    out.verify()?;
    Ok(out)
}

/// The engine's named constants `core` has a role for, as the materials' bindings name them
/// ([`crate::layer_bake::resolve::engine_role`]: the camera's matrices and position, the time, the time of day,
/// the pre-exposure and its inverse), and `cbLightAccParam`'s own name for the view matrix. `FC_ScreenSize` is its own
/// (two roles). Every other constant is unknown, with its buffer's name.
fn constant_role(name: &str) -> Option<ConstantRole> {
    match name {
        // cbLightAccParam's world-to-view matrix (three lines): the light-organize program turns each light's
        // position by it before testing it against the tiles
        "g_ViewMatrix" => Some(ConstantRole::View(ViewParameter::View)),
        name => crate::layer_bake::resolve::engine_role(name),
    }
}

struct Lower<'a> {
    game: Game,
    table: &'a table::Table,
    programs: &'a BTreeMap<String, Program>,
}

/// What a step binds at its programs' registers: its texture and buffer slots and its sampler slots.
#[derive(Clone, Copy)]
struct Slots<'a> {
    textures: &'a [table::Slot],
    samplers: &'a [table::SamplerSlot],
}

impl Lower<'_> {
    fn program(&self, name: &str) -> std::result::Result<&Program, String> {
        self.programs
            .get(name)
            .ok_or_else(|| format!("no program {name}"))
    }

    fn step(
        &self,
        s: &table::Step,
        cascades: &mut BTreeMap<u32, u8>,
        report: &mut Report,
    ) -> std::result::Result<RenderPass, String> {
        let presence = match &s.presence {
            table::Presence::Always => Presence::Always,
            table::Presence::When(table::Trigger::LocalShadowCasters) => {
                Presence::When(Trigger::LocalShadowCasters)
            }
            table::Presence::When(table::Trigger::Water) => Presence::When(Trigger::Water),
            table::Presence::Unknown(frames) => Presence::Unknown {
                frames: frames.clone(),
            },
        };
        let runs = match &s.runs {
            table::Runs::Once => Runs::Once,
            table::Runs::Each { why, seen } => Runs::Each {
                items: Items::Unknown { why: why.clone() },
                seen: seen.clone(),
            },
        };
        let work = match &s.work {
            table::Work::Clear { target, value } => PassWork::Clear {
                target: view(target),
                value: *value,
            },
            table::Work::Copy { from, to } => PassWork::Copy {
                from: view(from),
                to: view(to),
            },
            table::Work::ClearBuffer { buffer, value } => PassWork::ClearBuffer {
                buffer: *buffer,
                value: *value,
            },
            table::Work::CopyBuffer { from, to } => PassWork::CopyBuffer { from: *from, to: *to },
            table::Work::Screen(sc) => self.screen(s, sc, report)?,
            table::Work::Compute(c) => self.compute(c, report)?,
            table::Work::Scene {
                bucket,
                outputs: o,
                region: r,
            } => {
                let out = outputs(o);
                let view = match (bucket, r) {
                    (table::Bucket::ShadowCaster, Some(r)) => {
                        // a cascade is a region of its atlas: numbered in the order the frame draws the atlas's regions
                        let atlas = out
                            .depth
                            .as_ref()
                            .map(|d| d.view.target)
                            .ok_or("a shadow cascade with no depth target")?;
                        let next = cascades.entry(atlas).or_insert(0);
                        let cascade = *next;
                        *next = next.saturating_add(1);
                        SceneView::ShadowCascade {
                            cascade,
                            region: region(*r),
                        }
                    }
                    (table::Bucket::ShadowCaster, None) => SceneView::LocalShadow,
                    _ => SceneView::Camera,
                };
                let bucket = match bucket {
                    table::Bucket::Deferred => DrawBucket::Deferred,
                    table::Bucket::Decal => DrawBucket::Decal,
                    table::Bucket::Forward => DrawBucket::Forward,
                    table::Bucket::Sky => DrawBucket::Sky,
                    table::Bucket::ShadowCaster => DrawBucket::ShadowCaster,
                    table::Bucket::Velocity => DrawBucket::Velocity,
                };
                PassWork::Scene {
                    bucket,
                    view,
                    outputs: out,
                    draw: draw_of(bucket).to_owned(),
                    members: members_of(bucket),
                    frame: Vec::new(),
                    sorting: DrawSorting::NotStated {
                        why: SORTING_NOT_READ.to_owned(),
                    },
                }
            }
            table::Work::Slot { slot, outputs: o } => {
                if slot != "effects" {
                    return Err(format!("a slot of {slot}"));
                }
                PassWork::Slot {
                    slot: FeatureSlot::Effects,
                    outputs: outputs(o),
                }
            }
            table::Work::Group { steps } => PassWork::Group {
                steps: steps
                    .iter()
                    .map(|s| self.step(s, cascades, report))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            },
        };
        Ok(RenderPass {
            name: s.name.clone(),
            shared: false,
            presence,
            runs,
            work,
        })
    }

    fn screen(
        &self,
        s: &table::Step,
        sc: &table::Screen,
        report: &mut Report,
    ) -> std::result::Result<PassWork, String> {
        let out = outputs(&sc.outputs);
        let target_names: BTreeMap<u32, String> = out
            .colour
            .iter()
            .map(|(slot, v)| {
                let name = self
                    .table
                    .targets
                    .get(v.target as usize)
                    .map_or("?".to_owned(), |t| t.name.clone());
                (*slot, name)
            })
            .collect();
        let mut stages = Vec::new();
        let key = |i: usize, n: usize| -> Option<VariantKey> {
            (n > 1).then(|| VariantKey {
                field: format!("{} program", s.name),
                value: u32::try_from(i).unwrap_or(u32::MAX),
                parts: Vec::new(),
            })
        };
        let slots = Slots {
            textures: &sc.textures,
            samplers: &sc.samplers,
        };
        for (i, v) in sc.vertex.iter().enumerate() {
            let p = self.program(&v.program)?;
            let mut b = self.stage(
                p,
                &v.program,
                Stage::Vertex,
                slots,
                Some(&v.data),
                &target_names,
                report,
            )?;
            b.variant = key(i, sc.vertex.len());
            stages.push(b);
        }
        for (i, name) in sc.pixel.iter().enumerate() {
            let p = self.program(name)?;
            let mut b = self.stage(p, name, Stage::Pixel, slots, None, &target_names, report)?;
            b.variant = key(i, sc.pixel.len());
            stages.push(b);
        }
        Ok(PassWork::Screen {
            vertices: sc.vertices.ok_or("no vertex count")?,
            stages,
            state: render_state(&sc.state, out.colour.len())?,
            outputs: out,
            region: sc.region.map(region),
        })
    }

    fn compute(&self, c: &table::Compute, report: &mut Report) -> std::result::Result<PassWork, String> {
        let p = self.program(&c.program)?;
        let stage = self.stage(
            p,
            &c.program,
            Stage::Compute,
            Slots {
                textures: &c.textures,
                samplers: &c.samplers,
            },
            None,
            &BTreeMap::new(),
            report,
        )?;
        let mut writes = Vec::new();
        let mut buffer_writes = Vec::new();
        for w in &c.writes {
            let Some(r) = p.interface.resources.iter().find(|r| {
                r.provenance.d3d.is_some_and(|d| {
                    d.class == D3dClass::UnorderedAccess && d.register == w.register && d.space == 0
                })
            }) else {
                report.undeclared_writes = report.undeclared_writes.saturating_add(1);
                continue;
            };
            match (&w.view, w.graph_buffer) {
                (Some(v), None) => writes.push((r.id, view(v))),
                (None, Some(b)) => buffer_writes.push((r.id, b)),
                _ => {
                    return Err(format!(
                        "unordered-access register {} names no target or buffer",
                        w.register
                    ));
                }
            }
        }
        Ok(PassWork::Compute {
            stage,
            groups: Groups::Fixed(c.groups.ok_or("no thread groups")?),
            writes,
            buffer_writes,
        })
    }

    /// One program's binding in a step: what the frames bind at each texture and buffer register it declares (the
    /// step's slots for its stage, and for this program where the slots name one), the sampler each texture is read
    /// through where it is one, the state the frames bind at each sampler register it declares, its named constants,
    /// its vertex inputs as the step's vertex data shows them, and its outputs, the step's render targets.
    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        p: &Program,
        name: &str,
        stage: Stage,
        slots: Slots<'_>,
        vertex_data: Option<&str>,
        target_names: &BTreeMap<u32, String>,
        report: &mut Report,
    ) -> std::result::Result<StageBinding, String> {
        let interface = &p.interface;
        let slot_at = |space: u32, register: u32| {
            slots.textures.iter().find(|s| {
                stage_of(&s.stage).is_ok_and(|st| st == stage)
                    && s.program.as_deref().is_none_or(|p| p == name)
                    && s.space == space
                    && s.register == register
            })
        };
        // the program's sampler resource at a register
        let sampler_at = |space: u32, register: u32| {
            interface
                .resources
                .iter()
                .find(|r| {
                    matches!(r.kind, ResourceKind::Sampler { .. })
                        && r.provenance.d3d.is_some_and(|d| {
                            d.class == D3dClass::Sampler && d.space == space && d.register == register
                        })
                })
                .map(|r| r.id)
        };
        let mut samplers = Vec::new();
        for r in &interface.resources {
            let ResourceKind::Sampler { comparison } = r.kind else {
                continue;
            };
            let Some(d) = r.provenance.d3d.filter(|d| d.class == D3dClass::Sampler) else {
                continue;
            };
            let recorded = slots.samplers.iter().find(|s| {
                stage_of(&s.stage).is_ok_and(|st| st == stage)
                    && s.program.as_deref().is_none_or(|p| p == name)
                    && s.space == d.space
                    && s.register == d.register
            });
            let Some(recorded) = recorded else {
                report.unbound_samplers = report.unbound_samplers.saturating_add(1);
                continue;
            };
            let state = sampler_state(&recorded.state).map_err(|e| format!("s{}: {e}", d.register))?;
            if comparison != state.compare.is_some() {
                return Err(format!(
                    "s{}: the program declares a {} sampler and the frames bind a {} one",
                    d.register,
                    if comparison { "comparison" } else { "plain" },
                    if state.compare.is_some() {
                        "comparison"
                    } else {
                        "plain"
                    }
                ));
            }
            report.samplers = report.samplers.saturating_add(1);
            samplers.push(SamplerBinding {
                resource: r.id,
                state,
            });
        }
        let mut textures = Vec::new();
        let mut buffers = Vec::new();
        for r in &interface.resources {
            let Some(d) = r.provenance.d3d.filter(|d| d.class == D3dClass::ShaderResource) else {
                continue;
            };
            let slot = slot_at(d.space, d.register);
            match &r.kind {
                ResourceKind::Texture { .. } => {
                    let source = match slot {
                        Some(table::Slot { target: Some(v), .. }) => TextureSource::Target { view: view(v) },
                        Some(slot) => {
                            let what = slot
                                .engine
                                .clone()
                                .unwrap_or_else(|| "a buffer bound at a texture register".to_owned());
                            let n = report.engine_textures.entry(what).or_default();
                            *n = n.saturating_add(1);
                            TextureSource::Missing {
                                why: format!(
                                    "engine texture: {}",
                                    slot.engine
                                        .as_deref()
                                        .unwrap_or("a buffer bound at a texture register")
                                ),
                            }
                        }
                        None => {
                            report.unbound_slots = report.unbound_slots.saturating_add(1);
                            TextureSource::Missing {
                                why: "the render graph slot names no target or engine image".into(),
                            }
                        }
                    };
                    let reads = p.pairs.textures.get(&(d.space, d.register));
                    if reads.is_some_and(|t| t.samplers.len() > 1) {
                        report.textures_through_several_samplers =
                            report.textures_through_several_samplers.saturating_add(1);
                    }
                    textures.push(TextureBinding {
                        resource: r.id,
                        sampler: p
                            .pairs
                            .sampler_of((d.space, d.register))
                            .and_then(|(space, register)| sampler_at(space, register)),
                        source,
                    });
                }
                ResourceKind::Buffer(kind) => {
                    // the scene's lights where the frames bind no buffer of their own (the light lists are the
                    // organize step's)
                    let light = match (kind, slot) {
                        (
                            _,
                            Some(table::Slot {
                                graph_buffer: Some(_),
                                ..
                            }),
                        ) => None,
                        (iri::BufferKind::Structured { stride }, _) => {
                            lights::source(self.game, &r.provenance.name, *stride)
                        }
                        _ => None,
                    };
                    let source = match (light, slot) {
                        (Some(BufferSource::LightLists(part)), _) => {
                            report.light_lists = report.light_lists.saturating_add(1);
                            BufferSource::LightLists(part)
                        }
                        (Some(BufferSource::Lights(table)), _) => {
                            report.light_tables = report.light_tables.saturating_add(1);
                            let unknown = table
                                .fields
                                .iter()
                                .filter(|f| matches!(f.value, LightValue::Unknown { .. }))
                                .count();
                            report.unknown_light_fields = report.unknown_light_fields.saturating_add(unknown);
                            BufferSource::Lights(table)
                        }
                        (Some(other), _) => other,
                        (None, slot) => match slot {
                            Some(table::Slot {
                                graph_buffer: Some(b),
                                ..
                            }) => BufferSource::Graph(*b),
                            Some(table::Slot {
                                engine_buffer: Some(what),
                                ..
                            }) => {
                                report.unknown_buffers = report.unknown_buffers.saturating_add(1);
                                BufferSource::Unknown {
                                    why: format!(
                                        "the renderer's own buffer the recorded frames bind here ({what})"
                                    ),
                                }
                            }
                            Some(_) => {
                                report.unknown_buffers = report.unknown_buffers.saturating_add(1);
                                BufferSource::Unknown {
                                    why: "the recorded frames bind a texture at this buffer register".into(),
                                }
                            }
                            None => {
                                report.unbound_slots = report.unbound_slots.saturating_add(1);
                                report.unknown_buffers = report.unknown_buffers.saturating_add(1);
                                BufferSource::Unknown {
                                    why: format!("the recorded frames bind nothing at t{}", d.register),
                                }
                            }
                        },
                    };
                    buffers.push(BufferBinding {
                        resource: r.id,
                        source,
                    });
                }
                _ => {}
            }
        }
        let constants = self.constants(p, report);
        let inputs = interface
            .inputs
            .iter()
            .map(|v| {
                let role = if v.system.is_some() {
                    InputRole::System
                } else if stage == Stage::Pixel {
                    InputRole::Previous
                } else {
                    match (vertex_data, v.label.name.as_str(), v.label.index) {
                        (Some("screen_triangle"), "POSITION", 0) => {
                            InputRole::ScreenTriangle(ScreenTriangleValue::Position)
                        }
                        (Some("screen_triangle"), "TEXCOORD", 0) => {
                            InputRole::ScreenTriangle(ScreenTriangleValue::TextureCoordinate)
                        }
                        (data, ..) => {
                            report.unknown_inputs = report.unknown_inputs.saturating_add(1);
                            InputRole::Unknown {
                                why: match data {
                                    Some("unrecorded") => {
                                        "the step's vertex buffer: no recorded frame holds its bytes"
                                            .to_owned()
                                    }
                                    Some("screen_triangle") => {
                                        "an input the full-screen triangle's buffer does not hold".to_owned()
                                    }
                                    Some(other) => format!(
                                        "the step's vertex buffer ({other}): its contents are not read"
                                    ),
                                    None => "the step's vertex data is not stated".to_owned(),
                                },
                            }
                        }
                    }
                };
                InputBinding {
                    label: v.label.clone(),
                    role,
                }
            })
            .collect();
        let mut outputs = Vec::new();
        if stage == Stage::Pixel {
            for v in &interface.outputs {
                let slot = match v.system {
                    Some(SystemValue::Target(n)) => n,
                    Some(_) => continue,
                    None => match v.provenance.d3d {
                        Some(d) => d.register,
                        None => continue,
                    },
                };
                let width = v.value.width.min(4);
                let why = match target_names.get(&slot) {
                    Some(name) => format!("the step's render target {slot} ({name}), which later steps read"),
                    None => format!("render target {slot}, which the step binds nothing at"),
                };
                outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes: (1u8 << width) - 1,
                    role: OutputRole::unknown(why),
                });
            }
        }
        Ok(StageBinding {
            inputs,
            constants,
            textures,
            samplers,
            buffers,
            outputs,
            ..StageBinding::new(stage, p.entity)
        })
    }

    /// The program's named constants: a role where `core` has one ([`constant_role`]), else unknown with the buffer's
    /// name; a matrix's lines as the program's value graph shows them.
    fn constants(&self, p: &Program, report: &mut Report) -> Vec<ConstantBinding> {
        let mut out = Vec::new();
        for r in &p.interface.resources {
            let ResourceKind::ConstantBuffer { layout } = &r.kind else {
                continue;
            };
            for c in &layout.constants {
                let Some(size) = c.ty.size().filter(|s| *s > 0) else {
                    continue;
                };
                if c.name == "FC_ScreenSize" && size == 16 {
                    for (at, role) in [
                        (0, ConstantRole::View(ViewParameter::ViewportSize)),
                        (8, ConstantRole::View(ViewParameter::InverseViewportSize)),
                    ] {
                        out.push(ConstantBinding {
                            resource: r.id,
                            offset: c.offset + at,
                            size: 8,
                            name: Some(c.name.clone()),
                            role,
                            lines: None,
                        });
                    }
                    report.constants = report.constants.saturating_add(1);
                    continue;
                }
                let counted = lights::frame_constant(self.game, &c.name)
                    .filter(|_| size == 4)
                    .map(ConstantRole::Frame);
                let role = counted.or_else(|| constant_role(&c.name)).unwrap_or_else(|| {
                    report.unknown_constants = report.unknown_constants.saturating_add(1);
                    ConstantRole::Unknown {
                        why: format!(
                            "{}.{}: the {:?} renderer's own value, which no core role states",
                            r.provenance.name, c.name, self.game
                        ),
                    }
                });
                report.constants = report.constants.saturating_add(1);
                out.push(ConstantBinding {
                    resource: r.id,
                    offset: c.offset,
                    size,
                    name: Some(c.name.clone()),
                    role,
                    lines: None,
                });
            }
        }
        let graph = p.graph.as_ref().map_err(String::as_str);
        primase_core::shader::state_lines(&mut out, |c| {
            lines_at(graph, &p.interface, c.resource, c.offset, c.size)
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both games' tables parse, every step's targets and buffers are the table's, and the picture is a target.
    #[test]
    fn the_tables_parse_and_name_their_own_targets() {
        for game in [Game::Sekiro, Game::EldenRing] {
            let t = table::table(game).unwrap();
            assert!(!t.steps.is_empty() && !t.frames.is_empty(), "{game:?}");
            let picture = t.picture.unwrap();
            assert!((picture as usize) < t.targets.len(), "{game:?}");
            for target in &t.targets {
                super::target(target).unwrap();
            }
        }
    }

    /// The position of a step in a game's table, by a predicate on its name and work.
    fn position(t: &table::Table, f: impl Fn(&table::Step) -> bool) -> Option<usize> {
        t.steps.iter().position(f)
    }

    fn screen_program(s: &table::Step) -> Option<&str> {
        match &s.work {
            table::Work::Screen(sc) => sc.pixel.first().map(String::as_str),
            _ => None,
        }
    }

    fn is_sky(s: &table::Step) -> bool {
        matches!(
            s.work,
            table::Work::Scene {
                bucket: table::Bucket::Sky,
                ..
            }
        )
    }

    /// Where the frame's volumetric fog is composited against its sky, as each game's recorded frames draw it: Sekiro
    /// composites the fog before its sky (sek3's frame 15704: GXVolumetricFog_SceneComposite, then the sky dome, the
    /// moon and the far mountains), Elden Ring after it (er3's frame 6993: the cloud layer M[Clouds9] at draw 2026, the
    /// fog composite at 2065). `core` states the order as the graph's: a consumer follows it.
    #[test]
    fn each_game_composites_its_fog_where_its_frames_do() {
        let fog = |s: &table::Step| {
            screen_program(s) == Some("gxrenderershader/GXVolumetricFog_SceneComposite.ppo")
        };
        let sekiro = table::table(Game::Sekiro).unwrap();
        let (f, sky) = (
            position(&sekiro, fog).unwrap(),
            position(&sekiro, is_sky).unwrap(),
        );
        assert!(
            f < sky,
            "Sekiro: the fog composite (step {f}) before the sky (step {sky})"
        );
        let elden = table::table(Game::EldenRing).unwrap();
        let (f, sky) = (position(&elden, fog).unwrap(), position(&elden, is_sky).unwrap());
        assert!(
            sky < f,
            "Elden Ring: the sky (step {sky}) before the fog composite (step {f})"
        );
    }
}
