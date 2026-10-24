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
//! - **Clipping.** A triangle is clipped to `0 <= z <= w` (depth clipping) and to `w > 0`; the outputs of a vertex made
//!   by clipping are the clip-space blend of the edge's two ends. Clipping in x and y is the guard band's: none here, a
//!   pixel outside the targets is simply not drawn.
//! - **Viewport.** `x = X + (x/w + 1) / 2 * W`, `y = Y + (1 - y/w) / 2 * H` (y down), `z = MinDepth + z/w * (MaxDepth -
//!   MinDepth)`.
//! - **Rasterization.** Positions are snapped to 1/256 of a pixel (16.8 fixed point), as D3D's 8 bits of sub-pixel
//!   precision. A pixel is drawn when its centre is inside the triangle, or on an edge that is a top or a left edge.
//!   The facing is the sign of the snapped area: with y down a clockwise triangle is front facing unless the state
//!   says counter-clockwise; culling drops the facing it names; a zero-area triangle draws nothing.
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

/// A vertex in the rasterizer: clip-space position, the varyings as floats (bits kept for flat ones), screen position.
#[derive(Clone, Debug)]
struct ClipVertex {
    clip: [f32; 4],
    outputs: Vec<[u32; 4]>,
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
                    sources.push(Source::FrontFace {
                        register: to_register,
                        lane: to_lanes.first().copied().unwrap_or(0),
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
            let mut corners = Vec::with_capacity(3);
            for &i in triangle {
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
                let s = shaded[i].as_ref().expect("shaded above");
                corners.push(ClipVertex {
                    clip: s.position,
                    outputs: s.outputs.clone(),
                });
            }
            // the provoking vertex's outputs, for flat inputs: the triangle's first, whatever clipping makes
            let provoking = corners[0].outputs.clone();
            let polygon = clip(corners);
            if polygon.len() < 3 {
                stats.clipped_away += 1;
                continue;
            }
            for k in 1..polygon.len() - 1 {
                let tri = [&polygon[0], &polygon[k], &polygon[k + 1]];
                self.raster(
                    tri,
                    &provoking,
                    pixel_buffers,
                    sampler,
                    state,
                    surface,
                    &mut stats,
                )?;
            }
        }
        Ok(stats)
    }

    #[allow(clippy::too_many_arguments)]
    fn raster(
        &self,
        tri: [&ClipVertex; 3],
        provoking: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
        state: &Rasterizer,
        surface: &mut Surface,
        stats: &mut DrawStats,
    ) -> Result<(), Error> {
        let vp = state.viewport;
        // screen positions (unsnapped), the reciprocal w, and the snapped 16.8 fixed-point positions
        let mut screen = [[0f64; 3]; 3];
        let mut inv_w = [0f64; 3];
        let mut fixed = [[0i64; 2]; 3];
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
            fixed[k] = [(x * 256.).round() as i64, (y * 256.).round() as i64];
        }
        let edge = |a: [i64; 2], b: [i64; 2], p: [i64; 2]| {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        };
        let area = edge(fixed[0], fixed[1], fixed[2]);
        if area == 0 {
            return Ok(());
        }
        // y down: a positive area is clockwise on screen
        let clockwise = area > 0;
        let front = clockwise != state.front_counter_clockwise;
        if (state.cull == Cull::Front && front) || (state.cull == Cull::Back && !front) {
            stats.culled += 1;
            return Ok(());
        }
        // the edges of the triangle in clockwise order (positive area), as indices into tri
        let order: [usize; 3] = if clockwise { [0, 1, 2] } else { [0, 2, 1] };
        let area = area.abs();
        let edges: [(usize, usize); 3] = [(order[0], order[1]), (order[1], order[2]), (order[2], order[0])];
        // top-left rule for a clockwise triangle with y down: a top edge is horizontal and runs right (dx > 0); a
        // left edge runs up (dy < 0)
        let top_left: [bool; 3] = edges.map(|(a, b)| {
            let dx = fixed[b][0] - fixed[a][0];
            let dy = fixed[b][1] - fixed[a][1];
            (dy == 0 && dx > 0) || dy < 0
        });
        let (w, h) = (surface.width as i64, surface.height as i64);
        let min_x = (fixed.iter().map(|p| p[0]).min().unwrap_or(0) >> 8).clamp(0, w);
        let max_x = ((fixed.iter().map(|p| p[0]).max().unwrap_or(0) >> 8) + 1).clamp(0, w);
        let min_y = (fixed.iter().map(|p| p[1]).min().unwrap_or(0) >> 8).clamp(0, h);
        let max_y = ((fixed.iter().map(|p| p[1]).max().unwrap_or(0) >> 8) + 1).clamp(0, h);
        if min_x >= max_x || min_y >= max_y {
            return Ok(());
        }
        // barycentrics of a pixel centre from the snapped positions: the weight of each vertex of tri
        let weights = |px: i64, py: i64| -> ([f64; 3], bool) {
            let p = [px * 256 + 128, py * 256 + 128];
            let mut inside = true;
            let mut b = [0f64; 3];
            for (e, &(a, c)) in edges.iter().enumerate() {
                let value = edge(fixed[a], fixed[c], p);
                if value < 0 || (value == 0 && !top_left[e]) {
                    inside = false;
                }
                // the edge a -> c weighs the vertex opposite it
                let opposite = 3 - a - c;
                b[opposite] = value as f64 / area as f64;
            }
            (b, inside)
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
                    let (b, inside) = weights(px, py);
                    covered[l] = inside && px < w && py < h;
                    let z = b[0] * screen[0][2] + b[1] * screen[1][2] + b[2] * screen[2][2];
                    let z = (z as f32).clamp(vp.min_depth.min(vp.max_depth), vp.max_depth.max(vp.min_depth));
                    depths[l] = z;
                    self.inputs_at(
                        tri,
                        provoking,
                        b,
                        &inv_w,
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

    /// The pixel program's input registers at a pixel of barycentrics `b` (weights of tri's vertices).
    #[allow(clippy::too_many_arguments)]
    fn inputs_at(
        &self,
        tri: [&ClipVertex; 3],
        provoking: &[[u32; 4]],
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
                        let value = |v: &ClipVertex| {
                            f64::from(f32::from_bits(
                                v.outputs.get(*from_register).map_or(0, |r| r[*from as usize]),
                            ))
                        };
                        let word = match mode {
                            InterpMode::Flat => {
                                provoking.get(*from_register).map_or(0, |r| r[*from as usize])
                            }
                            InterpMode::Perspective => {
                                let v = persp[0] * value(tri[0])
                                    + persp[1] * value(tri[1])
                                    + persp[2] * value(tri[2]);
                                (v as f32).to_bits()
                            }
                            InterpMode::Linear => {
                                let v = b[0] * value(tri[0]) + b[1] * value(tri[1]) + b[2] * value(tri[2]);
                                (v as f32).to_bits()
                            }
                        };
                        into[*to_register][*to as usize] = word;
                    }
                }
            }
        }
    }
}

/// Clip a triangle to `0 <= z <= w` and `w > 0`: the polygon left, as clip-space vertices whose outputs are the blend
/// of the edge's ends (an empty list when nothing is left).
fn clip(triangle: Vec<ClipVertex>) -> Vec<ClipVertex> {
    // each plane as the signed distance of a vertex: kept where >= 0
    let planes: [fn(&[f32; 4]) -> f64; 3] = [
        |c| f64::from(c[2]),
        |c| f64::from(c[3]) - f64::from(c[2]),
        |c| f64::from(c[3]) - 1e-6,
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
    polygon
}

/// The point at `t` from `a` to `b` in clip space, its outputs blended alike (as floats).
fn blend(a: &ClipVertex, b: &ClipVertex, t: f64) -> ClipVertex {
    let mix = |x: f32, y: f32| (f64::from(x) + (f64::from(y) - f64::from(x)) * t) as f32;
    let clip = [0, 1, 2, 3].map(|k| mix(a.clip[k], b.clip[k]));
    let outputs = a
        .outputs
        .iter()
        .zip(&b.outputs)
        .map(|(x, y)| [0, 1, 2, 3].map(|k| mix(f32::from_bits(x[k]), f32::from_bits(y[k])).to_bits()))
        .collect();
    ClipVertex { clip, outputs }
}
