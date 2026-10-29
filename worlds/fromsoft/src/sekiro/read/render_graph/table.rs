//! The step tables of the games' renderers ([`super`]), as stored (`sekiro.json`, `elden.json`): the frame's targets
//! and buffers, then its steps in order, each with the frames that had it, how many times it ran, and what it did as
//! the recorded frames show it. Programs are named as the game's engine binders hold them (`<folder>/<file>`: the file
//! in `/shader/<folder>.shaderbnd.dcx`); states are the D3D numbers the frames recorded (D3D11's and D3D12's enums share
//! them); a texture or buffer slot is named by its stage, space and register.
use serde::Deserialize;
use std::collections::BTreeMap;

/// One game's table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Table {
    /// `sekiro` or `elden`.
    pub game: String,
    /// The view size the frames were recorded at.
    pub view: [u32; 2],
    /// The recorded frames the table is from, as `<run> frame_<n>`.
    pub frames: Vec<String>,
    pub targets: Vec<Target>,
    pub buffers: Vec<Buffer>,
    pub steps: Vec<Step>,
    /// The target the last tone-map step writes: the picture before the interface.
    pub picture: Option<u32>,
    /// What the frames drew that the table leaves out, and how many: the interface after the picture, occlusion
    /// proxies, an effect's own buffers.
    pub left_out: BTreeMap<String, u64>,
    /// The order in which the frames drew their sky, forward and effect draws in each sorted run, and how many runs had
    /// it (the table states the sky, the forward draws and the effects as steps of their own).
    pub forward_phase: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub name: String,
    /// `core.render_graph`'s format name; `None` where the frames' format has none (refused).
    pub format: Option<String>,
    pub size: Size,
    pub shape: Shape,
    pub mips: u32,
    pub history: bool,
    pub shared: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Size {
    View {
        numerator: u32,
        denominator: u32,
        depth: u32,
        rounding: Rounding,
    },
    Fixed {
        width: u32,
        height: u32,
        depth: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Rounding {
    Down,
    Up,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Shape {
    Plane,
    Volume,
    Cube,
    Array { layers: u32 },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Buffer {
    pub name: String,
    pub size: BufferSize,
    pub stride: Option<u32>,
    pub elements: Option<u64>,
    pub history: bool,
    pub shared: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BufferSize {
    Recorded { bytes: u64, view: [u32; 2] },
    Fixed { bytes: u64 },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub name: String,
    pub presence: Presence,
    pub runs: Runs,
    pub work: Work,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presence {
    Always,
    When(Trigger),
    Unknown(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Trigger {
    LocalShadowCasters,
    Water,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Runs {
    Once,
    Each { why: String, seen: Vec<u32> },
}

/// A view of a target: `target` indexes [`Table::targets`].
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct View {
    pub target: u32,
    pub mip: u32,
    pub first_layer: u32,
    pub layers: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Work {
    Clear {
        target: View,
        value: [f32; 4],
    },
    Copy {
        from: View,
        to: View,
    },
    ClearBuffer {
        buffer: u32,
        value: [u32; 4],
    },
    CopyBuffer {
        from: u32,
        to: u32,
    },
    Screen(Screen),
    Compute(Compute),
    Scene {
        bucket: Bucket,
        outputs: Outputs,
        /// A shadow cascade's region of its atlas; `None` for the camera's view, and for a step that runs once per region
        /// of a local lights' atlas (a run's own viewport).
        #[serde(default)]
        region: Option<[u32; 4]>,
    },
    Slot {
        slot: String,
        outputs: Outputs,
    },
    Group {
        steps: Vec<Step>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Bucket {
    Deferred,
    Decal,
    Forward,
    Sky,
    ShadowCaster,
    Velocity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Screen {
    /// The vertices one draw draws (`None` where the frames did not state a count).
    pub vertices: Option<u32>,
    /// The vertex programs: one, or the variants the runs of a repeated step drew with, each with what its vertex
    /// buffer held.
    pub vertex: Vec<Vertex>,
    /// The pixel programs: one, or the variants the runs drew with.
    pub pixel: Vec<String>,
    pub textures: Vec<Slot>,
    pub outputs: Outputs,
    pub state: State,
    /// The draw's viewport where it is not the whole of its outputs: x, y, width, height.
    pub region: Option<[u32; 4]>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Vertex {
    pub program: String,
    /// `screen_triangle` (every draw that recorded its vertex buffer's content bound the full-screen triangle), `none`
    /// (no vertex buffer: the program makes its vertices from their index), `unrecorded` (a buffer the frames did not
    /// record), or `other <hash>`.
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Compute {
    pub program: String,
    /// The thread groups the frames dispatched.
    pub groups: Option<[u32; 3]>,
    pub textures: Vec<Slot>,
    pub writes: Vec<Write>,
}

/// What the frame binds at one shader-resource register the step's programs declare: one of the graph's targets or
/// buffers, or a texture or buffer the renderer itself provides (described, not a target).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Slot {
    pub stage: String,
    /// The program the slot is for, where the step draws with more than one for its stage (the variants bind different
    /// things at one register); `None`: every program of the stage.
    #[serde(default)]
    pub program: Option<String>,
    pub space: u32,
    pub register: u32,
    pub target: Option<View>,
    pub graph_buffer: Option<u32>,
    pub engine: Option<String>,
    pub engine_buffer: Option<String>,
}

/// An unordered-access register a dispatch writes: a target's view or one of the graph's buffers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Write {
    pub register: u32,
    pub view: Option<View>,
    pub graph_buffer: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outputs {
    /// Render targets by slot.
    pub colour: Vec<(u32, View)>,
    pub depth: Option<DepthOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DepthOutput {
    pub view: View,
    pub read_only_depth: bool,
    pub read_only_stencil: bool,
}

/// The pipeline state as D3D's numbers; a field the runs of a repeated step differ in is `None`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    /// Per render target: enable, source, destination, operation, alpha source, alpha destination, alpha operation,
    /// write mask.
    pub blend: Option<Vec<[u32; 8]>>,
    pub depth: Option<Depth>,
    pub stencil: Option<StencilState>,
    pub cull: Option<u32>,
    pub fill: Option<u32>,
    pub front_ccw: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Depth {
    pub enable: bool,
    pub write: bool,
    pub func: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StencilState {
    pub read_mask: u32,
    pub write_mask: u32,
    /// Set per draw by the source: `None`.
    pub reference: Option<u32>,
    /// Fail, depth fail, pass, comparison.
    pub front: [u32; 4],
    pub back: [u32; 4],
}

/// The table of a game, parsed.
pub(crate) fn table(game: fromsoft_formats::Game) -> Result<Table, String> {
    let text = match game {
        fromsoft_formats::Game::Sekiro => include_str!("sekiro.json"),
        fromsoft_formats::Game::EldenRing => include_str!("elden.json"),
    };
    serde_json::from_str(text).map_err(|e| format!("the {game:?} render graph table: {e}"))
}
