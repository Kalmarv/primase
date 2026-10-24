//! A draw on the CPU, as Direct3D draws an indexed triangle list: the vertex program for each vertex, each triangle
//! clipped, mapped through the viewport and rasterized, the pixel program for each pixel it covers, the depth test,
//! and the render targets written. It is the draw a capture of a running game records, run again on the programs the
//! capture names, so its pixels can be set against the ones the game wrote.
//!
//! The rules are the Direct3D 11 functional specification's (which Direct3D 12 keeps), held to Microsoft's WARP
//! rasterizer by the `exec_warp_draw` tests:
//!
//! - **Linking.** A pixel program input reads the vertex program output of the same semantic and index (names compared
//!   without case), component by component; `SV_Position` is the pixel's position, `SV_IsFrontFace` the triangle's
//!   facing. An input no output carries is an error.
//! - **Clipping.** A triangle is clipped to `0 <= z <= w` (depth clipping) and to `-w <= x <= w`, `-w <= y <= w`: the
//!   viewport, section 15.4 of the D3D11.3 specification's clip volume. 15.4 allows a guard band ("or arbitrarily
//!   wider range if implementation uses a guard band"); WARP uses none (a triangle cut 16.25 pixels past the viewport
//!   instead of at it puts the pixels it draws up to 1e-4 off WARP's), so a vertex clipping makes on a side of the
//!   viewport is snapped like any other. A vertex near the camera plane, whose screen position is far outside, is
//!   snapped only after clipping brings it in. Those planes leave every vertex `w >= |x|, |y|, z >= 0`, so a division
//!   by `w` is finite but at the clip-space origin, which a clipped polygon reaches only when its triangle's vertices
//!   are linearly dependent: its plane holds the camera and it has no area on screen, so it draws nothing. A vertex
//!   clipping makes holds its weights on the triangle's three vertices in clip space, and its
//!   outputs are the triangle's by the interpolation of the input they feed: perspective-correct ones by those weights,
//!   the screen-linear ones (`noperspective`) by the weights of its screen position (a clip-space weight times the
//!   vertex's w, normalised), and flat ones the provoking vertex's. A triangle with a NaN position after clipping draws
//!   nothing (15.4); one with an infinity becomes NaN where clipping cuts it, which 15.4 allows ("may or may not result
//!   in a discarded primitive").
//! - **Viewport.** `x = X + (x/w + 1) / 2 * W`, `y = Y + (1 - y/w) / 2 * H` (y down), `z = MinDepth + z/w * (MaxDepth -
//!   MinDepth)`.
//! - **Rasterization.** Positions are snapped to 1/256 of a pixel (16.8 fixed point), as D3D's 8 bits of sub-pixel
//!   precision (15.16), to the nearest with a tie to even and saturating at the format's range (3.2.4.1). A pixel is
//!   drawn when its centre is inside the triangle, or on an edge that is a top or a left edge. The facing is the sign
//!   of the snapped area: with y down a clockwise triangle is front facing unless the state says counter-clockwise;
//!   culling drops the facing it names; a zero-area triangle draws nothing.
//! - **Interpolation.** At the pixel centre, from the snapped positions: perspective-correct (the default), linear in
//!   screen space (`noperspective`) or the provoking vertex's value (`nointerpolation`, integers; the triangle's first
//!   vertex). Centroid and sample interpolation are the centre's (one sample a pixel). Depth is linear in screen space
//!   and clamped to the viewport's depth range.
//! - **Pixels.** In 2 x 2 quads aligned to even pixels, uncovered lanes run as helpers (their inputs extrapolated) so
//!   derivatives and implicit levels of detail are the quad's; a helper writes nothing. A discarded pixel writes
//!   nothing. The depth test runs after the pixel program (its result is the same: a pixel that fails writes
//!   nothing); a pixel that passes writes every target lane the program writes and, when the state writes depth, its
//!   depth. Triangles are drawn in index order, so a later one overwrites an earlier one where both pass.
use crate::{Error, Execution, PixelShader, ProgramShader, Sampler};
use primase_shader::{InterpMode, SystemValue, Varying};

fn error(detail: impl Into<String>) -> Error {
    Error {
        bytecode: "draw",
        at: 0,
        detail: detail.into(),
    }
}

/// The viewport: where clip space lands in the targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// Which facing culling drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cull {
    None,
    Front,
    Back,
}

/// A depth comparison: the pixel's depth (left) against the depth buffer's (right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

impl Compare {
    fn passes(self, pixel: f32, stored: f32) -> bool {
        match self {
            Compare::Never => false,
            Compare::Less => pixel < stored,
            Compare::Equal => pixel == stored,
            Compare::LessEqual => pixel <= stored,
            Compare::Greater => pixel > stored,
            Compare::NotEqual => pixel != stored,
            Compare::GreaterEqual => pixel >= stored,
            Compare::Always => true,
        }
    }
}

/// The depth test and whether a pixel that passes writes its depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthState {
    pub compare: Compare,
    pub write: bool,
}

/// The fixed-function state of a draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rasterizer {
    pub viewport: Viewport,
    pub cull: Cull,
    /// Whether a counter-clockwise triangle (y down) is front facing.
    pub front_counter_clockwise: bool,
    /// The depth test; `None`: no depth buffer.
    pub depth: Option<DepthState>,
}

/// What a draw writes to: render targets by output register (`width * height` texels each, row by row, as the pixel
/// program's output words) and the depth buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    /// By output register; `None` for a register the draw does not write.
    pub targets: Vec<Option<Vec<[u32; 4]>>>,
    /// `width * height` depths, when the state tests depth.
    pub depth: Option<Vec<f32>>,
}

/// What a draw did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawStats {
    pub triangles: usize,
    pub culled: usize,
    /// Triangles clipped away entirely.
    pub clipped_away: usize,
    /// Triangles with a NaN position after clipping, which draw nothing.
    pub not_a_number: usize,
    /// Pixels the program ran for (helpers not counted).
    pub shaded: usize,
    pub discarded: usize,
    pub depth_failed: usize,
    pub written: usize,
}

/// Where a pixel program input comes from.
#[derive(Clone, Debug)]
enum Source {
    /// A vertex program output: its register and lanes, the pixel input's register and lanes, the interpolation.
    Varying {
        from_register: usize,
        from_lanes: Vec<u8>,
        to_register: usize,
        to_lanes: Vec<u8>,
        mode: InterpMode,
    },
    Position {
        register: usize,
        lanes: Vec<u8>,
    },
    FrontFace {
        register: usize,
        lane: u8,
    },
}

fn lanes_of(mask: u8) -> Vec<u8> {
    (0..4).filter(|l| mask >> l & 1 != 0).collect()
}

fn same_label(a: &Varying, b: &Varying) -> bool {
    a.label.index == b.label.index && a.label.name.eq_ignore_ascii_case(&b.label.name)
}

/// A vertex and a pixel program linked for drawing.
#[derive(Debug)]
pub struct DrawShader<'a> {
    vertex: &'a ProgramShader,
    pixel: &'a ProgramShader,
    sources: Vec<Source>,
}

/// A vertex after the vertex program: its clip-space position and its output registers.
#[derive(Clone, Debug)]
struct Shaded {
    position: [f32; 4],
    outputs: Vec<[u32; 4]>,
}

/// A vertex in the rasterizer: its clip-space position and its weights on the drawn triangle's three vertices in clip
/// space (a vertex of the triangle weighs itself one; one clipping makes, the blend of its edge's ends).
#[derive(Clone, Debug)]
struct ClipVertex {
    clip: [f32; 4],
    weights: [f64; 3],
}

/// The weights of the screen position of a point whose clip-space weights on a triangle are `clip` and whose
/// vertices' w are `w`: each clip-space weight times its vertex's w, normalised (a screen position is the clip-space
/// one over its w, so it is the blend of the vertices' screen positions by these).
fn screen_weights(clip: [f64; 3], w: [f64; 3]) -> [f64; 3] {
    let scaled = [clip[0] * w[0], clip[1] * w[1], clip[2] * w[2]];
    let sum = scaled[0] + scaled[1] + scaled[2];
    scaled.map(|x| x / sum)
}

impl<'a> DrawShader<'a> {
    /// Link `vertex`'s outputs to `pixel`'s inputs by semantic.
    pub fn new(vertex: &'a ProgramShader, pixel: &'a ProgramShader) -> Result<Self, Error> {
        let mut sources = Vec::new();
        for v in pixel.interface().inputs.iter().filter(|v| v.used) {
            let Some(sig) = v.provenance.d3d else {
                return Err(error(format!(
                    "pixel input {}{} has no register",
                    v.label.name, v.label.index
                )));
            };
            let to_register = sig.register as usize;
            let to_lanes = lanes_of(sig.mask);
            match v.system {
                Some(SystemValue::FragCoord) => {
                    sources.push(Source::Position {
                        register: to_register,
                        lanes: to_lanes,
                    });
                    continue;
                }
                Some(SystemValue::FrontFacing) => {
                    // the signature names the lane the facing is read in: a mask of none is a malformed signature
                    let Some(&lane) = to_lanes.first() else {
                        return Err(error(format!(
                            "pixel input {}{} is the facing, and its signature mask names no lane",
                            v.label.name, v.label.index
                        )));
                    };
                    sources.push(Source::FrontFace {
                        register: to_register,
                        lane,
                    });
                    continue;
                }
                Some(other) => {
                    return Err(error(format!(
                        "pixel input {}{} is the system value {other:?}, which a draw does not supply",
                        v.label.name, v.label.index
                    )));
                }
                None => {}
            }
            let out = vertex
                .interface()
                .outputs
                .iter()
                .find(|o| o.system.is_none() && same_label(o, v))
                .ok_or_else(|| {
                    error(format!(
                        "pixel input {}{} has no vertex output of its semantic",
                        v.label.name, v.label.index
                    ))
                })?;
            let from = out.provenance.d3d.ok_or_else(|| {
                error(format!(
                    "vertex output {}{} has no register",
                    out.label.name, out.label.index
                ))
            })?;
            let mode = match v.interp {
                Some(i) => i.mode,
                None if v.value.scalar == primase_shader::ScalarType::Float => InterpMode::Perspective,
                None => InterpMode::Flat,
            };
            sources.push(Source::Varying {
                from_register: from.register as usize,
                from_lanes: lanes_of(from.mask),
                to_register,
                to_lanes,
                mode,
            });
        }
        Ok(Self {
            vertex,
            pixel,
            sources,
        })
    }

    /// Draw `indices` (a triangle list) of `vertices` (each the vertex program's input registers) into `surface`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_indexed(
        &self,
        vertices: &[Vec<[u32; 4]>],
        indices: &[u32],
        vertex_buffers: &[(u32, &[[u32; 4]])],
        pixel_buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
        state: &Rasterizer,
        surface: &mut Surface,
    ) -> Result<DrawStats, Error> {
        if !indices.len().is_multiple_of(3) {
            return Err(error(format!(
                "{} indices are not whole triangles",
                indices.len()
            )));
        }
        let n = surface.width as usize * surface.height as usize;
        if surface.targets.iter().flatten().any(|t| t.len() != n)
            || surface.depth.as_ref().is_some_and(|d| d.len() != n)
        {
            return Err(error("a target or the depth buffer is not width x height"));
        }
        if state.depth.is_some() && surface.depth.is_none() {
            return Err(error("the state tests depth and the surface has no depth buffer"));
        }
        // the vertex program once per vertex the indices name
        let mut shaded: Vec<Option<Shaded>> = vec![None; vertices.len()];
        let mut stats = DrawStats::default();
        for triangle in indices.as_chunks::<3>().0 {
            stats.triangles += 1;
            let mut drawn = [0usize; 3];
            for (k, &i) in triangle.iter().enumerate() {
                let i = i as usize;
                let input = vertices.get(i).ok_or_else(|| {
                    error(format!(
                        "index {i} names a vertex past the {} given",
                        vertices.len()
                    ))
                })?;
                if shaded[i].is_none() {
                    let (e, position) = self.vertex.execute_vertex(input, vertex_buffers, sampler)?;
                    shaded[i] = Some(Shaded {
                        position: position.map(f32::from_bits),
                        outputs: e.outputs,
                    });
                }
                drawn[k] = i;
            }
            let original = drawn.map(|i| shaded[i].as_ref().expect("shaded above"));
            let corners = (0..3)
                .map(|k| {
                    let mut weights = [0.; 3];
                    weights[k] = 1.;
                    ClipVertex {
                        clip: original[k].position,
                        weights,
                    }
                })
                .collect();
            let polygon = clip(corners);
            if polygon.len() < 3 {
                stats.clipped_away += 1;
                continue;
            }
            if polygon.iter().any(|v| v.clip.iter().any(|c| c.is_nan())) {
                stats.not_a_number += 1;
                continue;
            }
            for k in 1..polygon.len() - 1 {
                let tri = [&polygon[0], &polygon[k], &polygon[k + 1]];
                self.raster(tri, original, pixel_buffers, sampler, state, surface, &mut stats)?;
            }
        }
        Ok(stats)
    }

    #[allow(clippy::too_many_arguments)]
    fn raster(
        &self,
        tri: [&ClipVertex; 3],
        original: [&Shaded; 3],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
        state: &Rasterizer,
        surface: &mut Surface,
        stats: &mut DrawStats,
    ) -> Result<(), Error> {
        let vp = state.viewport;
        let Some(setup) = Setup::new(tri, &vp) else {
            return Ok(());
        };
        let front = setup.clockwise != state.front_counter_clockwise;
        if (state.cull == Cull::Front && front) || (state.cull == Cull::Back && !front) {
            stats.culled += 1;
            return Ok(());
        }
        let (w, h) = (i64::from(surface.width), i64::from(surface.height));
        let Some(([min_x, max_x], [min_y, max_y])) = setup.bounds(w, h) else {
            return Ok(());
        };
        // quads aligned to even pixels
        let qx0 = min_x & !1;
        let qy0 = min_y & !1;
        let mut lane_inputs: [Vec<[u32; 4]>; 4] = Default::default();
        for qy in (qy0..max_y).step_by(2) {
            for qx in (qx0..max_x).step_by(2) {
                let lanes = [(qx, qy), (qx + 1, qy), (qx, qy + 1), (qx + 1, qy + 1)];
                let mut covered = [false; 4];
                let mut depths = [0f32; 4];
                for (l, &(px, py)) in lanes.iter().enumerate() {
                    let (b, inside) = setup.weights(px, py);
                    covered[l] = inside && px < w && py < h;
                    let z = b[0] * setup.screen[0][2] + b[1] * setup.screen[1][2] + b[2] * setup.screen[2][2];
                    let z = (z as f32).clamp(vp.min_depth.min(vp.max_depth), vp.max_depth.max(vp.min_depth));
                    depths[l] = z;
                    self.inputs_at(
                        tri,
                        original,
                        b,
                        &setup.inv_w,
                        [px as f32 + 0.5, py as f32 + 0.5, z],
                        front,
                        &mut lane_inputs[l],
                    );
                }
                if !covered.iter().any(|c| *c) {
                    continue;
                }
                let inputs = [
                    lane_inputs[0].as_slice(),
                    lane_inputs[1].as_slice(),
                    lane_inputs[2].as_slice(),
                    lane_inputs[3].as_slice(),
                ];
                let quad: [Execution; 4] = self.pixel.execute_quad(inputs, buffers, sampler)?;
                for (l, e) in quad.iter().enumerate() {
                    if !covered[l] {
                        continue;
                    }
                    stats.shaded += 1;
                    if e.discarded {
                        stats.discarded += 1;
                        continue;
                    }
                    let (px, py) = lanes[l];
                    let at = (py * w + px) as usize;
                    if let (Some(test), Some(buffer)) = (state.depth, surface.depth.as_mut()) {
                        if !test.compare.passes(depths[l], buffer[at]) {
                            stats.depth_failed += 1;
                            continue;
                        }
                        if test.write {
                            buffer[at] = depths[l];
                        }
                    }
                    for (register, target) in surface.targets.iter_mut().enumerate() {
                        let Some(target) = target else { continue };
                        let mask = e.output_masks.get(register).copied().unwrap_or(0);
                        let value = e.outputs.get(register).copied().unwrap_or([0; 4]);
                        for lane in 0..4 {
                            if mask >> lane & 1 != 0 {
                                target[at][lane] = value[lane];
                            }
                        }
                    }
                    stats.written += 1;
                }
            }
        }
        Ok(())
    }

    /// The pixel program's input registers at a pixel of barycentrics `b` (weights of `tri`'s vertices), `tri` a
    /// triangle of the clipped polygon of the drawn triangle `original` (whose first vertex is the provoking one).
    #[allow(clippy::too_many_arguments)]
    fn inputs_at(
        &self,
        tri: [&ClipVertex; 3],
        original: [&Shaded; 3],
        b: [f64; 3],
        inv_w: &[f64; 3],
        position: [f32; 3],
        front: bool,
        into: &mut Vec<[u32; 4]>,
    ) {
        into.clear();
        into.resize(self.pixel.input_count(), [0; 4]);
        // perspective-correct weights: b / w, normalised; and the interpolated w (1 / sum of b / w)
        let pw: [f64; 3] = [b[0] * inv_w[0], b[1] * inv_w[1], b[2] * inv_w[2]];
        let sum = pw[0] + pw[1] + pw[2];
        let persp = pw.map(|x| x / sum);
        // the same weights on the drawn triangle's vertices: perspective-correct through each vertex's clip-space
        // weights, screen-linear through the weights of its screen position
        let corner_w = original.map(|v| f64::from(v.position[3]));
        let (mut on_persp, mut on_linear) = ([0f64; 3], [0f64; 3]);
        for (k, v) in tri.iter().enumerate() {
            let screen = screen_weights(v.weights, corner_w);
            for i in 0..3 {
                on_persp[i] += persp[k] * v.weights[i];
                on_linear[i] += b[k] * screen[i];
            }
        }
        for s in &self.sources {
            match s {
                Source::Position { register, lanes } => {
                    let value = [position[0], position[1], position[2], (1. / sum) as f32];
                    for (k, lane) in lanes.iter().enumerate() {
                        into[*register][*lane as usize] = value[k].to_bits();
                    }
                }
                Source::FrontFace { register, lane } => {
                    into[*register][*lane as usize] = if front { u32::MAX } else { 0 };
                }
                Source::Varying {
                    from_register,
                    from_lanes,
                    to_register,
                    to_lanes,
                    mode,
                } => {
                    for (k, to) in to_lanes.iter().enumerate() {
                        let Some(from) = from_lanes.get(k) else { continue };
                        let word =
                            |v: &Shaded| v.outputs.get(*from_register).map_or(0, |r| r[*from as usize]);
                        let blend = |weights: [f64; 3]| {
                            let v: f64 = (0..3)
                                .map(|i| weights[i] * f64::from(f32::from_bits(word(original[i]))))
                                .sum();
                            (v as f32).to_bits()
                        };
                        let word = match mode {
                            InterpMode::Flat => word(original[0]),
                            InterpMode::Perspective => blend(on_persp),
                            InterpMode::Linear => blend(on_linear),
                        };
                        into[*to_register][*to as usize] = word;
                    }
                }
            }
        }
    }
}

/// The 16.8 fixed point's range in 1/256 pixels: 16 signed integer bits, 8 fractional (3.2.4.1).
const FIXED: [i64; 2] = [-(1 << 23), (1 << 23) - 1];

/// A screen coordinate in 16.8 fixed point: to the nearest, a tie to even, saturating at the format's range (3.2.4.1's
/// reference conversion; WARP takes the tie to even too, on exec_warp_draw's tie cases).
fn fixed(x: f64) -> i64 {
    ((x * 256.).round_ties_even() as i64).clamp(FIXED[0], FIXED[1])
}

/// Twice the signed area of `a`, `b`, `p` in fixed point: positive when `p` is right of `a -> b` (y down). With every
/// position in the 16.8 range a difference is under 2^24 and a product under 2^48.
fn edge(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// A clipped triangle set up for rasterization: its positions through the viewport, snapped, and its edges.
#[derive(Debug)]
struct Setup {
    /// Screen positions (unsnapped) and depth.
    screen: [[f64; 3]; 3],
    /// The reciprocal of each vertex's w.
    inv_w: [f64; 3],
    /// The snapped positions.
    fixed: [[i64; 2]; 3],
    /// Whether the snapped triangle is clockwise on screen (y down).
    clockwise: bool,
    /// Twice the snapped triangle's area, in fixed point (positive).
    area: i64,
    /// The edges in clockwise order, as indices into the triangle.
    edges: [(usize, usize); 3],
    /// Whether each edge is a top or a left edge.
    top_left: [bool; 3],
}

impl Setup {
    /// `None` for a triangle of no snapped area, which draws nothing.
    fn new(tri: [&ClipVertex; 3], vp: &Viewport) -> Option<Self> {
        let mut screen = [[0f64; 3]; 3];
        let mut inv_w = [0f64; 3];
        let mut fixed_at = [[0i64; 2]; 3];
        for (k, v) in tri.iter().enumerate() {
            let w = f64::from(v.clip[3]);
            let (nx, ny, nz) = (
                f64::from(v.clip[0]) / w,
                f64::from(v.clip[1]) / w,
                f64::from(v.clip[2]) / w,
            );
            let x = f64::from(vp.x) + (nx + 1.) * 0.5 * f64::from(vp.width);
            let y = f64::from(vp.y) + (1. - ny) * 0.5 * f64::from(vp.height);
            let z = f64::from(vp.min_depth) + nz * f64::from(vp.max_depth - vp.min_depth);
            screen[k] = [x, y, z];
            inv_w[k] = 1. / w;
            fixed_at[k] = [fixed(x), fixed(y)];
        }
        let area = edge(fixed_at[0], fixed_at[1], fixed_at[2]);
        if area == 0 {
            return None;
        }
        // y down: a positive area is clockwise on screen
        let clockwise = area > 0;
        let order: [usize; 3] = if clockwise { [0, 1, 2] } else { [0, 2, 1] };
        let edges: [(usize, usize); 3] = [(order[0], order[1]), (order[1], order[2]), (order[2], order[0])];
        // top-left rule for a clockwise triangle with y down: a top edge is horizontal and runs right (dx > 0); a
        // left edge runs up (dy < 0)
        let top_left: [bool; 3] = edges.map(|(a, b)| {
            let dx = fixed_at[b][0] - fixed_at[a][0];
            let dy = fixed_at[b][1] - fixed_at[a][1];
            (dy == 0 && dx > 0) || dy < 0
        });
        Some(Self {
            screen,
            inv_w,
            fixed: fixed_at,
            clockwise,
            area: area.abs(),
            edges,
            top_left,
        })
    }

    /// The pixels of a `w` x `h` target the triangle's bounding box holds, as x and y ranges; `None` when it holds
    /// none.
    fn bounds(&self, w: i64, h: i64) -> Option<([i64; 2], [i64; 2])> {
        let along = |axis: usize, size: i64| {
            let low = self.fixed.iter().map(|p| p[axis]).min().unwrap_or(0) >> 8;
            let high = (self.fixed.iter().map(|p| p[axis]).max().unwrap_or(0) >> 8) + 1;
            [low.clamp(0, size), high.clamp(0, size)]
        };
        let (x, y) = (along(0, w), along(1, h));
        (x[0] < x[1] && y[0] < y[1]).then_some((x, y))
    }

    /// The barycentrics of a pixel centre from the snapped positions (the weight of each vertex of the triangle), and
    /// whether the triangle covers it.
    fn weights(&self, px: i64, py: i64) -> ([f64; 3], bool) {
        let p = [px * 256 + 128, py * 256 + 128];
        let mut inside = true;
        let mut b = [0f64; 3];
        for (e, &(a, c)) in self.edges.iter().enumerate() {
            let value = edge(self.fixed[a], self.fixed[c], p);
            if value < 0 || (value == 0 && !self.top_left[e]) {
                inside = false;
            }
            // the edge a -> c weighs the vertex opposite it
            let opposite = 3 - a - c;
            b[opposite] = value as f64 / self.area as f64;
        }
        (b, inside)
    }
}

/// Clip a triangle to `0 <= z <= w` and the viewport (`-w <= x, y <= w`): the polygon left, as clip-space vertices with
/// their weights on the triangle's (an empty list when nothing is left). A polygon with a vertex at the clip-space
/// origin (w = 0, so x, y and z are 0 too: the planes keep `w >= |x|, |y|` and `w >= z >= 0`) is empty: a point of a
/// triangle is the zero vector only when its vertices are linearly dependent, so the triangle's plane holds the camera
/// and it has no area on screen; it has no screen position to divide out there.
fn clip(triangle: Vec<ClipVertex>) -> Vec<ClipVertex> {
    let w = |c: &[f32; 4]| f64::from(c[3]);
    // each plane as the signed distance of a vertex: kept where >= 0
    type Plane<'a> = &'a dyn Fn(&[f32; 4]) -> f64;
    let planes: [Plane; 6] = [
        &|c| f64::from(c[2]),
        &|c| w(c) - f64::from(c[2]),
        &|c| w(c) + f64::from(c[0]),
        &|c| w(c) - f64::from(c[0]),
        &|c| w(c) + f64::from(c[1]),
        &|c| w(c) - f64::from(c[1]),
    ];
    let mut polygon = triangle;
    for plane in planes {
        if polygon.iter().all(|v| plane(&v.clip) >= 0.) {
            continue;
        }
        let mut out = Vec::new();
        for i in 0..polygon.len() {
            let a = &polygon[i];
            let b = &polygon[(i + 1) % polygon.len()];
            let (da, db) = (plane(&a.clip), plane(&b.clip));
            if da >= 0. {
                out.push(a.clone());
            }
            if (da >= 0.) != (db >= 0.) {
                let t = da / (da - db);
                out.push(blend(a, b, t));
            }
        }
        polygon = out;
        if polygon.len() < 3 {
            return polygon;
        }
    }
    if polygon.iter().any(|v| v.clip[3] == 0.) {
        return Vec::new();
    }
    polygon
}

/// The point at `t` from `a` to `b` in clip space, its weights blended alike.
fn blend(a: &ClipVertex, b: &ClipVertex, t: f64) -> ClipVertex {
    let clip = [0, 1, 2, 3]
        .map(|k| (f64::from(a.clip[k]) + (f64::from(b.clip[k]) - f64::from(a.clip[k])) * t) as f32);
    let weights = [0, 1, 2].map(|i| a.weights[i] + (b.weights[i] - a.weights[i]) * t);
    ClipVertex { clip, weights }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: i64 = 16;
    const VIEWPORT: Viewport = Viewport {
        x: 0.,
        y: 0.,
        width: SIZE as f32,
        height: SIZE as f32,
        min_depth: 0.,
        max_depth: 1.,
    };

    /// A clip-space position at pixel coordinates (x right, y down), depth z, w.
    fn at(x: f32, y: f32, z: f32, w: f32) -> [f32; 4] {
        let s = SIZE as f32;
        [(x / s * 2. - 1.) * w, (1. - y / s * 2.) * w, z * w, w]
    }

    fn clipped(corners: [[f32; 4]; 3]) -> Vec<ClipVertex> {
        let mut k = 0;
        let triangle = corners.map(|clip| {
            let mut weights = [0.; 3];
            weights[k] = 1.;
            k += 1;
            ClipVertex { clip, weights }
        });
        clip(triangle.to_vec())
    }

    /// A clip-space position on screen, in pixels of the target.
    fn on_screen(c: [f64; 4]) -> [f64; 2] {
        [
            (c[0] / c[3] + 1.) * 0.5 * SIZE as f64,
            (1. - c[1] / c[3]) * 0.5 * SIZE as f64,
        ]
    }

    /// How many times each pixel of the target is covered by the triangles of the clipped polygon.
    fn coverage(polygon: &[ClipVertex]) -> Vec<u32> {
        let mut count = vec![0; (SIZE * SIZE) as usize];
        for k in 1..polygon.len().saturating_sub(1) {
            let Some(setup) = Setup::new([&polygon[0], &polygon[k], &polygon[k + 1]], &VIEWPORT) else {
                continue;
            };
            let Some(([x0, x1], [y0, y1])) = setup.bounds(SIZE, SIZE) else {
                continue;
            };
            for py in y0..y1 {
                for px in x0..x1 {
                    if setup.weights(px, py).1 {
                        count[(py * SIZE + px) as usize] += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn a_vertex_near_the_camera_plane_is_clipped_to_the_viewport() {
        // c, at w = 1e-7, is about 8e9 pixels right of the target (and a fifth of that up), past what 16.8 fixed point
        // holds and what the edge function's products hold in i64; the viewport's sides cut its edges inside the
        // target. The triangle seen from the target is a, b and the point at infinity along (5, -1): the centres with
        // 42.25 < x + 5 y < 78.25 right of a -> b (x - y > -5.75). No pixel centre is within 0.04 pixels of its
        // edges, and snapping the cut vertices moves an edge by under 1/256 of a pixel.
        let a = at(2.25, 8., 0.5, 1.);
        let b = at(8.25, 14., 0.5, 1.);
        let c = [1e3, 2e2, 0., 1e-7];
        let polygon = clipped([a, b, c]);
        let expected: Vec<u32> = (0..SIZE * SIZE)
            .map(|i| {
                let (x, y) = ((i % SIZE) as f64 + 0.5, (i / SIZE) as f64 + 0.5);
                u32::from(x + 5. * y > 42.25 && x + 5. * y < 78.25 && x - y > -5.75)
            })
            .collect();
        assert_eq!(coverage(&polygon), expected);
        for v in &polygon {
            for screen in on_screen(v.clip.map(f64::from)) {
                assert!(
                    (-1e-3..=SIZE as f64 + 1e-3).contains(&screen),
                    "{screen} past the viewport"
                );
            }
        }
    }

    /// A vertex clipping makes is the blend of the triangle's vertices by its weights in clip space, and of their
    /// screen positions by its screen weights ([`screen_weights`]): what a perspective-correct and a screen-linear
    /// output of it are blended by. Where the ends of a cut edge have different w the two differ: the viewport's right
    /// side cuts both edges to the far vertex (w = 1e-7) about 1e-3 of the way along in clip space, and its weight on
    /// screen is 1e-7 times that (the ratio of the ends' w).
    #[test]
    fn a_cut_vertex_holds_its_weights_in_clip_space_and_on_screen() {
        let corners = [
            at(2.25, 8., 0.5, 1.),
            at(8.25, 14., 0.5, 1.),
            [1e3, 2e2, 0., 1e-7],
        ];
        let polygon = clipped(corners);
        let corner_w = corners.map(|c| f64::from(c[3]));
        let mut cut = 0;
        for v in &polygon {
            let in_clip: [f64; 4] =
                std::array::from_fn(|k| (0..3).map(|i| v.weights[i] * f64::from(corners[i][k])).sum());
            for (k, (blended, kept)) in in_clip.iter().zip(v.clip).enumerate() {
                let kept = f64::from(kept);
                assert!(
                    (blended - kept).abs() <= 1e-6 * kept.abs().max(1.),
                    "clip lane {k}: {blended} from the weights, {kept} kept"
                );
            }
            let screen = screen_weights(v.weights, corner_w);
            let far = on_screen(corners[2].map(f64::from));
            let near = [0, 1].map(|i| on_screen(corners[i].map(f64::from)));
            let at = on_screen(v.clip.map(f64::from));
            for axis in 0..2 {
                let blended = screen[0] * near[0][axis] + screen[1] * near[1][axis] + screen[2] * far[axis];
                assert!(
                    (blended - at[axis]).abs() <= 1e-3,
                    "screen axis {axis}: {blended} from the weights, {} kept",
                    at[axis]
                );
            }
            if v.weights[2] > 0. && v.weights[2] < 1. {
                cut += 1;
                assert!(
                    v.weights[2] < 1e-2 && screen[2] < 1e-6 * v.weights[2],
                    "{:?} {screen:?}",
                    v.weights
                );
            }
        }
        assert_eq!(cut, 2, "{polygon:?}");
    }

    /// A vertex inside the viewport nearer than any fixed w is kept, as Direct3D's clip volume keeps it: its screen
    /// position is a division by its w, which the viewport's planes keep within the target.
    #[test]
    fn a_vertex_at_a_tiny_w_inside_the_viewport_is_kept() {
        let near = [1e-8, -1e-8, 0., 1e-7];
        let polygon = clipped([at(2., 2., 0.5, 1.), at(14., 3., 0.5, 1.), near]);
        let kept: Vec<[u32; 4]> = polygon.iter().map(|v| v.clip.map(f32::to_bits)).collect();
        assert!(kept.contains(&near.map(f32::to_bits)), "{polygon:?}");
        assert!(coverage(&polygon).iter().any(|&n| n > 0));
    }

    /// A triangle through the clip-space origin (its vertices linearly dependent: `c = -(a + b) / 2` here) is a line on
    /// screen; clipping leaves the origin a vertex of it, and it draws nothing.
    #[test]
    fn a_triangle_through_the_clip_space_origin_draws_nothing() {
        let a = at(4., 4., 0.5, 1.);
        let b = at(12., 6., 0.5, 1.);
        let c = [0, 1, 2, 3].map(|k| -(a[k] + b[k]) / 2.);
        assert!(clipped([a, b, c]).is_empty());
    }

    #[test]
    fn a_triangle_inside_the_viewport_is_not_clipped() {
        let corners = [
            at(1., 14., 0.5, 1.),
            at(8., 1.5, 0.5, 3.),
            at(15., 13., 0.25, 0.5),
        ];
        let polygon = clipped(corners);
        let kept: Vec<[u32; 4]> = polygon.iter().map(|v| v.clip.map(f32::to_bits)).collect();
        assert_eq!(kept, corners.map(|c| c.map(f32::to_bits)));
    }

    #[test]
    fn an_infinite_position_is_clipped_away_or_not_a_number() {
        let polygon = clipped([
            at(2., 2., 0.5, 1.),
            [f32::INFINITY, 0., 0.5, 1.],
            at(2., 12., 0.5, 1.),
        ]);
        assert!(polygon.len() < 3 || polygon.iter().any(|v| v.clip.iter().any(|c| c.is_nan())));
    }

    #[test]
    fn snapping_rounds_to_the_nearest_a_tie_to_even_and_saturates() {
        assert_eq!(fixed(100.25), 25664);
        assert_eq!(fixed(1. / 1024.), 0);
        assert_eq!(fixed(3. / 1024.), 1);
        // a tie goes to the even step, either way from zero
        assert_eq!(fixed(1152.5 / 256.), 1152);
        assert_eq!(fixed(1153.5 / 256.), 1154);
        assert_eq!(fixed(-1152.5 / 256.), -1152);
        assert_eq!(fixed(1. / 512.), 0);
        assert_eq!(fixed(1e30), FIXED[1]);
        assert_eq!(fixed(-1e30), FIXED[0]);
        assert_eq!(fixed(f64::INFINITY), FIXED[1]);
        assert_eq!(fixed(f64::NAN), 0);
    }
}
