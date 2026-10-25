//! A layered material's vertex program, run for the bake: what the pixel program is given is what the game's own vertex
//! program writes, not a table. The bake has no mesh to rasterise (a group's baked texture serves many meshes, and none of
//! the meshes that read a second UV set has a unique unwrap: [`super::SECOND_UV_APPROXIMATION`]), so the program is run at
//! the corners of a texture-coordinate cell and what it writes is read as the interpolation a rasteriser would make across
//! one quad covering the cell: a value the program writes that does not depend on the coordinate is a constant, and one
//! that is the coordinate itself (identity scale and scroll) is the grid's U or V. A value that is neither is refused, by
//! name: the grid cannot carry it.
//!
//! The coordinate set is the packed `TEXCOORD0` of 16-bit shorts (`uv * 2048`), so the program is run on the exact
//! corners 0 and 1 (raw 0 and 2048) and never on a texel's own coordinate, which it would quantise.
use super::binding::{FrozenFrame, Inputs, TEXTURE_SCROLL_POLICY, frozen_words};
use super::resolve::{layer_channels, register};
use crate::{Result, invalid};
use primase_core::Semantic;
use primase_core::shader::{ConstantRole, InputRole, LaneStream, Shader as CoreShader, StageBinding};
use primase_shader::{Interface, Varying};
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{Input, Lane, PixelShader, Sampler};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The policy for the second vertex colour of a mesh: the layered vertex program hands `COLOR1.x` on as the tint's alpha,
/// and the bake does not group by it.
pub const COLOUR1_POLICY: &str = "COLOR1.x frozen to 1: observed in Sekiro m10_00/m11_01 and ercap3 RB5-12 c3000 weapon VBs (gap/colour-ercap3-colour1.json SHA256 cb54bc8e8a8b06e82d01320d1c5b110e54f4f9726f7c3fd1ccfcb29561f17190); other meshes and absent streams remain an approximation";

/// The policy for the first vertex colour of a mesh: a program that reads `COLOR0` (the most frequent refusal in the
/// recorded bake reports: "input COLOR0 has no value to run on") is run with it white, the bake's declared static frame. The
/// lowered geometry keeps the mesh's own as `Color(0)`, which a baked texture shared by many meshes cannot carry; a
/// program that multiplies its albedo or an occlusion by it is drawn as if the colour were 1.
pub const COLOUR0_POLICY: &str = "COLOR0 frozen to white (1, 1, 1, 1): the bake has no mesh, so a vertex colour the program reads (an albedo multiplier, an occlusion or a blend weight) is taken as 1";

/// What feeds one vertex attribute of the program, by the role the binding gives its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Feed {
    /// The origin.
    Position,
    /// The stored normal bytes of a surface facing up.
    Normal,
    /// The stored tangent bytes of the x axis, positive handedness.
    Tangent,
    /// The tangent's w byte alone (its sign), positive handedness: the input of its own the cloth permutations read.
    TangentSign,
    /// The packed texture coordinates: both sets the grid's.
    TexCoords,
    /// The one texture coordinate set of a program that takes one (the non-layered SAT shaders'): two shorts, `uv * 2048`,
    /// the grid's.
    TexCoord0,
    /// The Byte4C layer member of that index, its lanes the level's bake inputs.
    Member(u8),
    /// The first vertex colour, white in all four lanes.
    Colour0,
    /// The second vertex colour.
    Colour1,
    /// The instance index, zero.
    Instance,
    /// The joint indices of a skinned vertex: joint zero in all four slots (the bind pose: the palette is the identity).
    Joints,
    /// The joint weights: all of it on the first.
    Weights,
}

/// One vertex attribute of the lifted program: where its lanes go in the input registers, and what feeds them.
#[derive(Debug)]
struct Attribute {
    register: usize,
    /// The register lanes the element takes, in order.
    lanes: Vec<usize>,
    feed: Feed,
}

/// The lifted vertex program with its frozen constants and the plan to feed it.
#[derive(Debug)]
pub(crate) struct VertexStage {
    /// The program's container as stored, which `shader` is lifted from.
    bytecode: Arc<[u8]>,
    shader: ProgramShader,
    constants: Vec<(u32, Vec<[u32; 4]>)>,
    attributes: Vec<Attribute>,
    /// Pixel input lanes the selected program actually reads, by semantic label.
    pixel_lanes: BTreeMap<(String, u32), u8>,
    ground: bool,
    pub policies: BTreeMap<String, u64>,
}

/// The raw buffer of per-instance indices the program reads: one instance, index zero.
struct Instances;
impl Sampler for Instances {
    fn sample(&mut self, request: primase_shader_exec::Sample) -> std::result::Result<[f32; 4], String> {
        Err(format!(
            "t{}: a vertex program of this bake samples nothing",
            request.texture
        ))
    }
    fn structured(&mut self, _slot: u32, _element: u32, _offset: u32) -> std::result::Result<u32, String> {
        Ok(0)
    }
}

/// The texture coordinates of the four runs, in this order: the cell's corners (0, 0), (1, 0), (0, 1) and (1, 1).
pub(super) const CORNERS: [[u32; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];

fn lanes_of(mask: u8) -> Vec<usize> {
    (0..4).filter(|l| mask >> l & 1 != 0).collect()
}

fn label_of(v: &Varying) -> (String, u32) {
    (v.label.name.to_ascii_uppercase(), v.label.index)
}

impl VertexStage {
    /// Lift the vertex program `vertex` (a `core.shader` the binding `stage` was resolved against), bind its constants as
    /// the roles say and `frame` freezes them, and plan what feeds each of its inputs. `ground`: the material is of the
    /// ground family, whose second layer weight is read from another lane.
    pub fn new(
        vertex: &CoreShader,
        stage: &StageBinding,
        pixel: &primase_shader::Program,
        ground: bool,
        frame: &FrozenFrame,
    ) -> Result<Self> {
        let lifted = super::binding::program_executor_bound(
            vertex.format,
            &vertex.bytecode,
            &primase_shader::BoundSizes::of(stage),
        )?;
        let demand = primase_shader::flow::VertexDemand::of_programs(lifted.program(), pixel)
            .map_err(|e| invalid("vertex demand", 0, e))?;
        let pixel_lanes = demand
            .pixel
            .iter()
            .map(|(&i, &mask)| (label_of(&pixel.interface.inputs[i]), mask))
            .collect();
        let mut policies = BTreeMap::new();
        // the constants: every buffer the program reads, zero where nothing states a value
        let mut constants: Vec<(u32, Vec<[u32; 4]>)> = lifted
            .constant_buffers()
            .iter()
            .map(|&(slot, rows)| (slot, vec![[0u32; 4]; rows as usize]))
            .collect();
        for c in &stage.constants {
            let resource = vertex
                .interface
                .resource(c.resource)
                .ok_or_else(|| invalid("vertex binding", 0, "a constant of a resource the program lacks"))?;
            let slot = register(resource)?;
            let Some((_, rows)) = constants.iter_mut().find(|(s, _)| *s == slot) else {
                continue;
            };
            let Some(words) = frozen_words(&c.role, c.lines.as_ref(), frame, &mut policies)? else {
                if let ConstantRole::Unknown { why } = &c.role {
                    return Err(invalid(
                        "vertex binding",
                        0,
                        format!(
                            "{} has no value (a vertex program is not run on a zero it cannot know): {why}",
                            c.name.as_deref().unwrap_or("a constant")
                        ),
                    ));
                }
                continue;
            };
            let start = c.offset as usize / 4;
            // a constant fills its bytes, and a buffer the source binds larger than the program declares (a joint palette)
            // the whole bound
            let span = stage
                .bound_size(c.resource)
                .map_or(c.size, |bound| bound.saturating_sub(c.offset).max(c.size));
            for (i, w) in words.iter().take(span as usize / 4).enumerate() {
                let at = start + i;
                if let Some(row) = rows.get_mut(at / 4) {
                    row[at % 4] = *w;
                }
            }
        }
        // what feeds each input: the role the binding states for its label
        let mut attributes = Vec::new();
        for (input, v) in lifted
            .interface()
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, v)| v.used)
        {
            let label = label_of(v);
            let required = demand.vertex.get(&input).copied().unwrap_or(0);
            if required == 0 {
                policies.insert(
                    format!("{}{} fed nothing: no pixel output reads them", label.0, label.1),
                    1,
                );
                continue;
            }
            let role = stage
                .inputs
                .iter()
                .find(|i| (i.label.name.to_ascii_uppercase(), i.label.index) == label)
                .map(|i| &i.role)
                .ok_or_else(|| {
                    invalid(
                        "vertex binding",
                        0,
                        format!("input {}{} has no binding", label.0, label.1),
                    )
                })?;
            let feed = match role {
                InputRole::Stream(Semantic::Position) => Feed::Position,
                InputRole::Stream(Semantic::Normal) => Feed::Normal,
                InputRole::Stream(Semantic::Tangent) => Feed::Tangent,
                InputRole::Stream(Semantic::Joints(0)) => Feed::Joints,
                InputRole::Stream(Semantic::Weights(0)) => Feed::Weights,
                InputRole::Stream(Semantic::TexCoord(0)) => Feed::TexCoord0,
                InputRole::Stream(Semantic::Color(0)) => {
                    policies.insert(COLOUR0_POLICY.into(), 1);
                    Feed::Colour0
                }
                InputRole::Stream(Semantic::Color(n)) if (1..=6).contains(n) => Feed::Member(*n),
                InputRole::Stream(s) if *s == super::resolve::SECOND_COLOUR => {
                    // only x is handed on: anything more would be a value the bake does not know
                    if required & 0b1110 != 0 {
                        return Err(invalid(
                            "vertex binding",
                            0,
                            "the program reads COLOR1 lanes beyond x",
                        ));
                    }
                    policies.insert(COLOUR1_POLICY.into(), 1);
                    Feed::Colour1
                }
                InputRole::Lanes(lanes) => match lanes.as_slice() {
                    // the packed pair of coordinate sets
                    [
                        LaneStream {
                            semantic: Semantic::TexCoord(0),
                            first: 0,
                            count: 2,
                            component: 0,
                        },
                        LaneStream {
                            semantic: Semantic::TexCoord(1),
                            first: 2,
                            count: 2,
                            component: 0,
                        },
                    ] => Feed::TexCoords,
                    // the tangent's w as an input of its own
                    [
                        LaneStream {
                            semantic: Semantic::Tangent,
                            first: 0,
                            count: 1,
                            component: 3,
                        },
                    ] => Feed::TangentSign,
                    other => {
                        return Err(invalid(
                            "vertex binding",
                            0,
                            format!(
                                "input {}{} has lanes the bake cannot feed: {other:?}",
                                label.0, label.1
                            ),
                        ));
                    }
                },
                InputRole::System => Feed::Instance,
                InputRole::Unknown { why }
                    if matches!((label.0.as_str(), label.1), ("BINORMAL", 0) | ("TANGENT", 2)) =>
                {
                    // BINORMAL0 supplies the tangent-sign byte separately in the inspected programs.
                    // TANGENT2 is also read by non-cloth permutations. Neither has a source role yet;
                    // do not diagnose missing simulation data from an unsupported vertex input.
                    let missing = if label.0 == "BINORMAL" {
                        "separate tangent-sign byte has no source role"
                    } else {
                        "additional tangent input has no source role"
                    };
                    return Err(invalid(
                        "vertex binding",
                        0,
                        format!("input {}{}: {missing} ({why})", label.0, label.1),
                    ));
                }
                other => {
                    return Err(invalid(
                        "vertex binding",
                        0,
                        format!("input {}{} has no value to run on: {other:?}", label.0, label.1),
                    ));
                }
            };
            let sig = v.provenance.d3d.ok_or_else(|| {
                invalid(
                    "vertex binding",
                    0,
                    format!("input {}{} has no D3D register", label.0, label.1),
                )
            })?;
            attributes.push(Attribute {
                register: sig.register as usize,
                lanes: lanes_of(sig.mask),
                feed,
            });
        }
        Ok(Self {
            bytecode: vertex.bytecode.as_slice().into(),
            shader: lifted,
            constants,
            attributes,
            pixel_lanes,
            ground,
            policies,
        })
    }

    /// The program's container as stored.
    pub(super) fn bytecode(&self) -> &Arc<[u8]> {
        &self.bytecode
    }

    /// Every buffer the program reads, by slot, as its rows of four words.
    pub(super) fn constants(&self) -> &[(u32, Vec<[u32; 4]>)] {
        &self.constants
    }

    /// The input registers of one run at texture coordinate `uv` (0 or 1 on each axis) and the level's `params`.
    fn registers(&self, uv: [u32; 2], params: &Inputs) -> Vec<[u32; 4]> {
        let mut registers = vec![[0u32; 4]; self.shader.input_count()];
        let channels = layer_channels(self.ground);
        let weights = [
            params.weights_snow[0],
            params.weights_snow[1],
            params.weights_snow[2],
            params.weights_snow[3],
            params.tint_color[0],
            params.tint_color[1],
            params.tint_color[2],
            params.detail,
        ];
        for a in &self.attributes {
            let value: [u32; 4] = match a.feed {
                Feed::Position | Feed::Instance => [0; 4],
                Feed::Normal => [127, 254, 127, 0],
                Feed::Tangent => [254, 127, 127, 254],
                Feed::TangentSign => [254, 0, 0, 0],
                Feed::TexCoords => {
                    let raw = |u: u32| (u * 2048) as i32 as u32;
                    [raw(uv[0]), raw(uv[1]), raw(uv[0]), raw(uv[1])]
                }
                Feed::TexCoord0 => {
                    let raw = |u: u32| (u * 2048) as i32 as u32;
                    [raw(uv[0]), raw(uv[1]), 0, 0]
                }
                Feed::Member(n) => {
                    let mut v = [0f32; 4];
                    for (k, &(member, lane)) in channels.iter().enumerate() {
                        if member == n {
                            v[lane] = weights[k];
                        }
                    }
                    v.map(f32::to_bits)
                }
                Feed::Colour0 => [1f32.to_bits(); 4],
                Feed::Colour1 => [1f32.to_bits(), 0, 0, 0],
                Feed::Joints => [0; 4],
                Feed::Weights => [1f32.to_bits(), 0, 0, 0],
            };
            if let Some(row) = registers.get_mut(a.register) {
                for (k, &lane) in a.lanes.iter().enumerate() {
                    row[lane] = value[k];
                }
            }
        }
        registers
    }

    /// The input registers of the four runs, at the cell's corners [`CORNERS`], for the level's `params`.
    pub(super) fn corners(&self, params: &Inputs) -> [Vec<[u32; 4]>; 4] {
        CORNERS.map(|uv| self.registers(uv, params))
    }

    /// The output registers the program writes from each of `corners`' input registers. Every structured buffer it
    /// reads (the per-instance indices) gives zero: one instance, index zero.
    pub(super) fn outputs(&self, corners: &[Vec<[u32; 4]>; 4]) -> Result<[Vec<[u32; 4]>; 4]> {
        let buffers: Vec<(u32, &[[u32; 4]])> =
            self.constants.iter().map(|(s, r)| (*s, r.as_slice())).collect();
        let mut runs = corners.iter().map(|registers| {
            self.shader
                .execute(registers, &buffers, &mut Instances)
                .map(|run| run.outputs)
                .map_err(|e| invalid("vertex program", e.at, e.detail))
        });
        Ok([
            runs.next().expect("four corners")?,
            runs.next().expect("four corners")?,
            runs.next().expect("four corners")?,
            runs.next().expect("four corners")?,
        ])
    }

    /// What the pixel program `pixel` is given by the vertex program, per pixel input the program writes (the label, the
    /// lanes): from four runs at the cell's corners, each lane a constant, the grid's U or V, or an error.
    pub(crate) fn pixel_inputs(
        &self,
        pixel: &Interface,
        params: &Inputs,
    ) -> Result<BTreeMap<(String, u32), Input>> {
        self.pixel_inputs_from(pixel, &self.outputs(&self.corners(params))?)
    }

    /// [`VertexStage::pixel_inputs`] from the program's outputs at the corners ([`VertexStage::outputs`]).
    pub(super) fn pixel_inputs_from(
        &self,
        pixel: &Interface,
        runs: &[Vec<[u32; 4]>; 4],
    ) -> Result<BTreeMap<(String, u32), Input>> {
        let mut found = BTreeMap::new();
        for input in pixel.inputs.iter().filter(|i| i.used) {
            let label = label_of(input);
            let needed = self.pixel_lanes.get(&label).copied().unwrap_or(0);
            if needed == 0 {
                found.insert(
                    (input.label.name.clone(), input.label.index),
                    Input::Lanes([Lane::Bits(0); 4]),
                );
                continue;
            }
            let Some(out) = self
                .shader
                .interface()
                .outputs
                .iter()
                .find(|o| o.system.is_none() && label_of(o) == label)
            else {
                continue;
            };
            let sig = out.provenance.d3d.ok_or_else(|| {
                invalid(
                    "vertex program",
                    0,
                    format!("output {}{} has no D3D register", label.0, label.1),
                )
            })?;
            let lanes = lanes_of(sig.mask);
            let width = input.value.width as usize;
            if lanes.len() < width {
                return Err(invalid(
                    "vertex program",
                    0,
                    format!(
                        "output {}{} has fewer lanes than the pixel program reads",
                        label.0, label.1
                    ),
                ));
            }
            let mut result = [Lane::Bits(0); 4];
            for k in 0..width {
                if needed & (1 << k) == 0 {
                    continue;
                }
                let at = |r: usize| runs[r].get(sig.register as usize).map_or(0, |row| row[lanes[k]]);
                let (c, x, y, xy) = (at(0), at(1), at(2), at(3));
                let f = f32::from_bits;
                result[k] = if x == c && y == c && xy == c {
                    Lane::Bits(c)
                } else if f(c) == 0. && f(x) == 1. && f(y) == 0. && f(xy) == 1. {
                    Lane::U
                } else if f(c) == 0. && f(x) == 0. && f(y) == 1. && f(xy) == 1. {
                    Lane::V
                } else {
                    return Err(invalid(
                        "vertex program",
                        0,
                        format!(
                            "{}{} lane {k} is {:?} at the cell's corners: neither a constant nor the grid's U or V ({TEXTURE_SCROLL_POLICY})",
                            label.0,
                            label.1,
                            [f(c), f(x), f(y), f(xy)]
                        ),
                    ));
                };
            }
            found.insert(
                (input.label.name.clone(), input.label.index),
                Input::Lanes(result),
            );
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer_bake::resolve::resolve_vertex;
    use primase_core::World;
    use primase_core::shader::ShaderFormat;

    const VERTEX: &[u8] = include_bytes!("../../../tests/fixtures/layered_vs.dxbc");
    const PIXEL: &[u8] = include_bytes!("../../../tests/fixtures/layered.dxbc");

    fn demanded(vertex: &[u8], pixel: &[u8], missing_position: bool) -> Result<VertexStage> {
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: vertex.to_vec(),
            interface: primase_dxbc::reflection(vertex).unwrap(),
        };
        let mut binding = resolve_vertex(&shader, World::default().spawn()).unwrap().stage;
        if missing_position {
            binding
                .inputs
                .iter_mut()
                .find(|i| i.label.name == "POSITION")
                .unwrap()
                .role = InputRole::Unknown {
                why: "fixture has no position source".into(),
            };
        }
        VertexStage::new(
            &shader,
            &binding,
            &primase_dxbc::lift(pixel).unwrap(),
            false,
            &FrozenFrame::static_bake(0.),
        )
    }

    const DEMAND_VS: &[u8] = include_bytes!("../../../tests/fixtures/vertex_demand_vs.dxbc");
    const DEMAND_DEAD: &[u8] = include_bytes!("../../../tests/fixtures/vertex_demand_dead_ps.dxbc");

    #[test]
    fn an_input_feeding_only_an_unused_varying_needs_no_bake_source() {
        let vertex = demanded(DEMAND_VS, DEMAND_DEAD, false).unwrap();
        assert!(
            vertex
                .policies
                .keys()
                .any(|p| p == "TEXCOORD1 fed nothing: no pixel output reads them")
        );
        let pixel = primase_dxbc::reflection(DEMAND_DEAD).unwrap();
        let inputs = vertex.pixel_inputs(&pixel, &Inputs::default()).unwrap();
        assert_eq!(lanes(&inputs[&("TEXCOORD".into(), 0)])[..2], [Lane::U, Lane::V]);
        // The same missing source is required as soon as its varying reaches a pixel output.
        let live = include_bytes!("../../../tests/fixtures/vertex_demand_live_ps.dxbc");
        assert!(
            demanded(DEMAND_VS, live, false)
                .unwrap_err()
                .to_string()
                .contains("TEXCOORD1")
        );
    }

    #[test]
    fn rasterization_and_discard_dependencies_still_need_their_sources() {
        let discard = include_bytes!("../../../tests/fixtures/vertex_demand_discard_ps.dxbc");
        let clip = include_bytes!("../../../tests/fixtures/vertex_demand_clip_vs.dxbc");
        assert!(
            demanded(DEMAND_VS, discard, false)
                .unwrap_err()
                .to_string()
                .contains("TEXCOORD1")
        );
        assert!(
            demanded(clip, DEMAND_DEAD, false)
                .unwrap_err()
                .to_string()
                .contains("TEXCOORD1")
        );
        assert!(
            demanded(DEMAND_VS, DEMAND_DEAD, true)
                .unwrap_err()
                .to_string()
                .contains("POSITION0")
        );
    }

    fn stage(frame: &FrozenFrame) -> Result<VertexStage> {
        let interface = primase_dxbc::reflection(VERTEX).unwrap();
        let entity = World::default().spawn();
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: VERTEX.to_vec(),
            interface,
        };
        let binding = resolve_vertex(&shader, entity).unwrap().stage;
        VertexStage::new(
            &shader,
            &binding,
            &primase_dxbc::lift(PIXEL).unwrap(),
            false,
            frame,
        )
    }

    const CLOTH: &[u8] = include_bytes!("../../../tests/fixtures/layered_cloth_vs.dxbc");
    const SIGN_PIXEL: &[u8] = include_bytes!("../../../tests/fixtures/layered_sign_ps.dxbc");

    /// The cloth permutation's one-byte BINORMAL0 is the tangent's w, fed as a sign byte: not a texture coordinate, which
    /// the lanes of the packed pair are. A lane of another component, or of another attribute, is refused by name.
    #[test]
    fn a_tangent_component_lane_is_fed_the_tangent_sign_and_no_coordinate() {
        let interface = primase_dxbc::reflection(CLOTH).unwrap();
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: CLOTH.to_vec(),
            interface,
        };
        let entity = World::default().spawn();
        let mut binding = resolve_vertex(&shader, entity).unwrap().stage;
        let frame = FrozenFrame::static_bake(0.);
        // Unlike PIXEL, this program consumes TANGENT0.w and TEXCOORD7.w: both the sign and COLOR1.x are live.
        let pixel = primase_dxbc::lift(SIGN_PIXEL).unwrap();
        let planned = VertexStage::new(&shader, &binding, &pixel, false, &frame).unwrap();
        let sign: Vec<_> = planned
            .attributes
            .iter()
            .filter(|a| matches!(a.feed, Feed::TangentSign))
            .collect();
        assert_eq!(sign.len(), 1);
        assert!(planned.policies.contains_key(COLOUR1_POLICY));
        let params = Inputs::default();
        for registers in planned.corners(&params) {
            // the byte of a positive sign in the lane the input has, whatever corner of the cell the run is at
            assert_eq!(registers[sign[0].register][0], 254);
        }
        let inputs = planned.pixel_inputs(&pixel.interface, &params).unwrap();
        assert_eq!(
            lanes(&inputs[&("TANGENT".into(), 0)])[3],
            Lane::Bits(1f32.to_bits())
        );
        assert_eq!(
            lanes(&inputs[&("TEXCOORD".into(), 7)])[3],
            Lane::Bits(1f32.to_bits())
        );
        // the same input with a lane of the tangent's z is a value the bake does not know
        let label = binding
            .inputs
            .iter()
            .position(|i| i.label.name.eq_ignore_ascii_case("BINORMAL"))
            .unwrap();
        binding.inputs[label].role = InputRole::Lanes(vec![LaneStream {
            semantic: Semantic::Tangent,
            first: 0,
            count: 1,
            component: 2,
        }]);
        let refused = VertexStage::new(&shader, &binding, &pixel, false, &frame).expect_err("refused");
        assert!(
            refused.to_string().contains("lanes the bake cannot feed"),
            "{refused}"
        );
    }

    /// A program that takes one coordinate set (the non-layered SAT shaders': `MetallicTex`, `MetallicValue`) is run with the
    /// grid's coordinates in its two shorts, `uv * 2048`; it was refused for having "no value to run on" for `TEXCOORD0`.
    #[test]
    fn a_one_set_vertex_program_is_fed_the_grid_in_its_two_shorts() {
        const SINGLE: &[u8] = include_bytes!("../../../tests/fixtures/single_uv_vs.dxbc");
        let interface = primase_dxbc::reflection(SINGLE).unwrap();
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: SINGLE.to_vec(),
            interface,
        };
        let binding = resolve_vertex(&shader, World::default().spawn()).unwrap().stage;
        let planned = VertexStage::new(
            &shader,
            &binding,
            &primase_dxbc::lift(PIXEL).unwrap(),
            false,
            &FrozenFrame::static_bake(0.),
        )
        .unwrap();
        let uv: Vec<_> = planned
            .attributes
            .iter()
            .filter(|a| matches!(a.feed, Feed::TexCoord0))
            .collect();
        assert_eq!(uv.len(), 1);
        let corners = planned.corners(&Inputs::default());
        let at = |corner: usize| -> Vec<u32> { uv[0].lanes.iter().map(|&l| corners[corner][uv[0].register][l]).collect() };
        // the corners (0, 0), (1, 0), (0, 1), (1, 1) at raw 0 and 2048
        assert_eq!(at(0), [0, 0]);
        assert_eq!(at(1), [2048, 0]);
        assert_eq!(at(2), [0, 2048]);
        assert_eq!(at(3), [2048, 2048]);
    }

    /// A vertex input the binding states as the first vertex colour is fed white in every lane and the policy is named
    /// (the cloth fixture's COLOR1, bound as `Color(0)`): it is no longer refused for having no value to run on.
    #[test]
    fn a_first_vertex_colour_is_fed_white_and_the_policy_is_named() {
        let interface = primase_dxbc::reflection(CLOTH).unwrap();
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: CLOTH.to_vec(),
            interface,
        };
        let entity = World::default().spawn();
        let mut binding = resolve_vertex(&shader, entity).unwrap().stage;
        let at = binding
            .inputs
            .iter()
            .position(|i| i.label.name.eq_ignore_ascii_case("COLOR") && i.label.index == 1)
            .unwrap();
        binding.inputs[at].role = InputRole::Stream(Semantic::Color(0));
        let pixel = primase_dxbc::lift(SIGN_PIXEL).unwrap();
        let planned = VertexStage::new(&shader, &binding, &pixel, false, &FrozenFrame::static_bake(0.)).unwrap();
        assert!(planned.policies.contains_key(COLOUR0_POLICY));
        let colour: Vec<_> = planned
            .attributes
            .iter()
            .filter(|a| matches!(a.feed, Feed::Colour0))
            .collect();
        assert_eq!(colour.len(), 1);
        for registers in planned.corners(&Inputs::default()) {
            for &lane in &colour[0].lanes {
                assert_eq!(registers[colour[0].register][lane], 1f32.to_bits());
            }
        }
    }

    fn lanes(input: &Input) -> [Lane; 4] {
        match input {
            Input::Lanes(l) => *l,
            other => panic!("{other:?}"),
        }
    }

    /// The program's own writes are the pixel inputs: the packed coordinates as the grid's U and V in both sets, the layer
    /// weights from the Byte4C members by the channel table, constant across the cell.
    #[test]
    fn the_vertex_program_gives_the_pixel_program_its_inputs() {
        let vertex = stage(&FrozenFrame::static_bake(0.)).unwrap();
        let pixel = primase_dxbc::reflection(PIXEL).unwrap();
        let params = Inputs {
            weights_snow: [0.125, 0.25, 0.5, 0.75],
            ..Inputs::default()
        };
        let inputs = vertex.pixel_inputs(&pixel, &params).unwrap();
        let find = |name: &str, index: u32| {
            inputs
                .iter()
                .find(|((n, i), _)| n.eq_ignore_ascii_case(name) && *i == index)
                .map(|(_, v)| lanes(v))
                .unwrap_or_else(|| panic!("no {name}{index}"))
        };
        assert_eq!(find("TEXCOORD", 0), [Lane::U, Lane::V, Lane::U, Lane::V]);
        let bits = |v: [f32; 4]| v.map(|x| Lane::Bits(x.to_bits()));
        assert_eq!(find("TEXCOORD", 5), bits([0.125, 0.25, 0.5, 0.75]));
        // SV_Position is the pipeline's, not a varying the program writes
        assert!(inputs.keys().all(|(n, _)| !n.eq_ignore_ascii_case("SV_Position")));
        // PIXEL reads only position, UVs and weights; the VS's COLOR1.x -> TEXCOORD7.w is not linked to a read.
        assert!(!vertex.policies.contains_key(COLOUR1_POLICY));
        assert!(
            vertex
                .policies
                .contains_key("COLOR1 fed nothing: no pixel output reads them")
        );
        assert!(vertex.policies.contains_key(TEXTURE_SCROLL_POLICY));
    }

    /// The skinned permutation, at the bind pose (joint zero at full weight, a palette of identities), gives the pixel program
    /// the same inputs as the unskinned one: its coordinates the grid's, its weights the level's.
    #[test]
    fn the_skinned_permutation_at_the_bind_pose_gives_the_same_pixel_inputs() {
        const SKIN: &[u8] = include_bytes!("../../../tests/fixtures/layered_skin_vs.dxbc");
        let interface = primase_dxbc::reflection(SKIN).unwrap();
        let entity = World::default().spawn();
        let shader = CoreShader {
            format: ShaderFormat::Dxbc,
            bytecode: SKIN.to_vec(),
            interface,
        };
        let binding = resolve_vertex(&shader, entity).unwrap().stage;
        let skinned = VertexStage::new(
            &shader,
            &binding,
            &primase_dxbc::lift(PIXEL).unwrap(),
            false,
            &FrozenFrame::static_bake(0.),
        )
        .unwrap();
        assert!(skinned.policies.contains_key(super::super::BIND_POSE_POLICY));
        let pixel = primase_dxbc::reflection(PIXEL).unwrap();
        let params = Inputs {
            weights_snow: [0.125, 0.25, 0.5, 0.75],
            ..Inputs::default()
        };
        let plain = stage(&FrozenFrame::static_bake(0.)).unwrap();
        assert_eq!(
            skinned.pixel_inputs(&pixel, &params).unwrap(),
            plain.pixel_inputs(&pixel, &params).unwrap()
        );
    }

    /// The corners a bake's state carries are the cell's, in [`CORNERS`] order, and the outputs it carries, run from them,
    /// give the pixel inputs the bake reads.
    #[test]
    fn the_corner_runs_are_the_pixel_inputs_source() {
        let vertex = stage(&FrozenFrame::static_bake(0.)).unwrap();
        let pixel = primase_dxbc::reflection(PIXEL).unwrap();
        let params = Inputs {
            weights_snow: [0.125, 0.25, 0.5, 0.75],
            ..Inputs::default()
        };
        let corners = vertex.corners(&params);
        for (registers, uv) in corners.iter().zip(CORNERS) {
            assert_eq!(registers, &vertex.registers(uv, &params));
        }
        let outputs = vertex.outputs(&corners).unwrap();
        assert_eq!(
            vertex.pixel_inputs_from(&pixel, &outputs).unwrap(),
            vertex.pixel_inputs(&pixel, &params).unwrap()
        );
        assert_eq!(&**vertex.bytecode(), VERTEX);
    }

    /// A scroll that is not the identity is not a grid lane: refused, by input and lane, not baked as the identity.
    #[test]
    fn a_texture_scroll_other_than_the_identity_is_refused_by_name() {
        let mut frame = FrozenFrame::static_bake(0.);
        frame.texture_scroll[0] = [2., 1., 0., 0.];
        let vertex = stage(&frame).unwrap();
        let pixel = primase_dxbc::reflection(PIXEL).unwrap();
        let error = vertex
            .pixel_inputs(&pixel, &Inputs::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("TEXCOORD0 lane 0"), "{error}");
    }
}
