//! A pixel program's albedo read as layers laid over one another: the one reading of a program's blend as
//! `core.layered` states it ([`LayeredAlbedo`]), for any source whose material binds a program and states its roles.
//!
//! The albedo ([`super::Surface`]'s [`Base::Albedo`]) is read from the program's value graph: under each of its three
//! lanes, a chain of interpolations ([`Over`]), each towards a layer's colour by a weight. Each layer's colour is one
//! texture sample's lanes and constants, as the program computes them ([`Scalar`]: its own float operations, in its
//! order); each weight is the same value in the three lanes, made of vertex stream lanes, texture sample lanes and
//! constants. A value the chain is laid over by a constant weight towards a constant colour, directly above the bottom
//! layer, is the bottom layer's colour (`lerp(texel, white, 0.3)`, a colour moved part of the way to a constant).
//! Outside the chain the lane may be saturated and multiplied by factors; each factor must be exactly 1 where the host
//! freezes what it reads ([`Freeze`]): a factor that is not is refused, never dropped. A lane with no interpolation
//! under it is one layer laid over nothing: its colour is the lane under a saturate and factors of exactly 1, and a
//! product by any other constant is the colour's own (a single texture times its factor).
//!
//! What the program reads besides the texture samples is answered by the binding's own words
//! ([`StageBinding::known_words`]) and the host's freeze: constant words of runtime roles the host holds still, and what
//! each pixel input lane is (a lane of a stream of the mesh, or a word every pixel of the draw is given). A value made
//! of those words alone is evaluated with the program's own operations (`iridia_exec::value`), bit for bit; one the
//! freeze does not answer is refused, naming its node.
//!
//! The reading states what core's numeric rules make of it exactly ([`crate::layered`]'s expression rules: each
//! operation rounded on its own, `saturate` and `min`/`max` as Direct3D's). It does not state the texture's
//! filtering: a sample is the texel the consumer's sampler gives.
use std::collections::{BTreeMap, BTreeSet};

use iridia::Program;
use iridia::blend::{Over, Scalar, Term as Op};
use iridia::flow::{At, Node, NodeId, Read, Values};
use iridia_exec::value::{Evaluated, Evaluator};
use iridia_interface::{AddressMode, ResourceId, ResourceKind};
use serde::Serialize;

use super::surface::Base;
use super::{StageBinding, Surface, TextureSource};
use crate::Semantic;
use crate::layered::{Channel, ColorLane, Expression, ExpressionBuilder, Source, TermId};
use crate::pbr::{TextureRef, UvTransform, Wrap};

/// What a pixel input lane is, as the host links it to what it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputLink {
    /// Lane `channel` of the mesh's stream `semantic`, interpolated: the lane's value at each pixel, bit for bit where
    /// the stream's value at the vertices is.
    Stream { semantic: Semantic, channel: u8 },
    /// The same 32-bit word at every pixel of the draw.
    Word(u32),
}

/// What the host holds still for the read: constant words beyond the material's own, and what the pixel inputs are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Freeze {
    /// Words of constants the binding states a runtime role for (the instance's colour, the frame's slot), by the
    /// constant buffer resource and byte offset, as [`StageBinding::known_words`] keys the material's.
    pub words: BTreeMap<(ResourceId, u32), u32>,
    /// Each pixel input's lanes by its label (the name as the program spells it, any case, and index); a lane `None`
    /// or an input not here is not known.
    pub inputs: BTreeMap<(String, u32), [Option<InputLink>; 4]>,
}

impl Freeze {
    /// The link of lane `lane` of the pixel input named `name` and `index`.
    fn input(&self, name: &str, index: u32, lane: u8) -> Option<InputLink> {
        self.inputs
            .iter()
            .find(|((n, i), _)| *i == index && n.eq_ignore_ascii_case(name))
            .and_then(|(_, lanes)| lanes.get(usize::from(lane)).copied().flatten())
    }
}

/// One layer of a [`LayeredAlbedo`]: a texture sampled at one place, and the colour the program makes of its lanes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AlbedoLayer {
    /// The texture, the UV set and transform it is sampled at, its addressing and filters.
    pub texture: TextureRef,
    /// Each colour lane is the sample's own lane times this factor (1 for the lane itself), where [`AlbedoLayer::color`]
    /// is `None`: a base colour texture times its factor.
    pub factor: [f32; 3],
    /// Otherwise the colour's red, green and blue, each an expression of the sample's lanes; [`AlbedoLayer::factor`] is
    /// then 1.
    pub color: Option<[Expression<ColorLane>; 3]>,
}

/// A pixel program's albedo as layers laid over one another ([`crate::layered::Composition::Over`]): the first layer,
/// then each later one laid over what is below it by its weight, `lerp(below, layer, weight)` in each lane.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayeredAlbedo {
    /// The layers, bottom first.
    pub layers: Vec<AlbedoLayer>,
    /// One weight channel per layer; the first layer's is the constant 1 (the bottom layer is not weighted).
    pub weights: Vec<Channel>,
    /// The blended colour is saturated, each lane (NaN gives 0).
    pub saturate: bool,
    /// Sampled textures whose addressing the binding does not state, read as repeating.
    pub unstated_wrap: usize,
}

/// Why a program's albedo is not read as layers: what kind of node or form stopped it (a short, stable name a host
/// counts by) and the details.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    /// The kind of what was refused: `no albedo`, a node's kind (`input`, `texel load`, `op`, ...) or a form
    /// (`factor`, `coordinate`, `per-lane weight`, `layer sample`).
    pub kind: String,
    /// What was refused, in words.
    pub why: String,
}

fn refuse(kind: impl Into<String>, why: impl Into<String>) -> Refusal {
    Refusal {
        kind: kind.into(),
        why: why.into(),
    }
}

/// The kind of a node, as a [`Refusal`] names it.
fn node_kind(node: &Node) -> String {
    match node {
        Node::Input { .. } => "input".into(),
        Node::Supplied { .. } => "supplied value".into(),
        Node::Literal(_) => "literal".into(),
        Node::Global { .. } => "global".into(),
        Node::Texel { read, .. } => match read {
            Read::Sample => "texel sample".into(),
            Read::Gather(_) => "texel gather".into(),
            Read::Load => "texel load".into(),
        },
        Node::Query { .. } => "query".into(),
        Node::Op { op, .. } => format!("op {}", op_kind(op)),
        Node::Merge { .. } => "merge".into(),
        Node::Loop { .. } => "loop".into(),
    }
}

/// An operation's kind without its operands' details.
fn op_kind(op: &iridia::flow::Op) -> String {
    let full = format!("{op:?}");
    full.split('(').next().unwrap_or(&full).to_owned()
}

/// One texture sample: the texture resource, the coordinate's nodes and the values that decide it besides.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Sample {
    resource: ResourceId,
    coordinate: Vec<NodeId>,
}

/// A leaf of a value as the reading takes it.
#[derive(Clone, Debug, PartialEq)]
enum Leaf {
    /// A word the binding and the freeze give.
    Constant(f32),
    /// A lane of a texture sample.
    Texel(Sample, u8),
    /// A lane of a mesh stream.
    Stream(Semantic, u8),
}

/// The reading of one program: its graph, what its leaves are, and what it has evaluated.
struct Reader<'a, L> {
    program: &'a Program,
    stage: &'a StageBinding,
    freeze: &'a Freeze,
    values: &'a Values,
    evaluator: Evaluator<'a, L>,
}

impl<L: FnMut(NodeId, &Node, &[u32]) -> Option<u32>> Reader<'_, L> {
    /// What leaf `id` of a [`Scalar`] is: a word the freeze gives, a texture sample's lane or a mesh stream's.
    fn leaf(&mut self, id: NodeId) -> Result<Leaf, Refusal> {
        let node = self.values.node(id);
        match self.evaluator.evaluate(id) {
            Ok(Evaluated::Word(w)) => {
                let x = f32::from_bits(w);
                return if x.is_finite() {
                    Ok(Leaf::Constant(x))
                } else {
                    Err(refuse(
                        "constant",
                        format!("{} is {x} where the freeze holds it", node_kind(node)),
                    ))
                };
            }
            Ok(Evaluated::Unknown(_)) => {}
            Err(why) => return Err(refuse(node_kind(node), why)),
        }
        match node {
            Node::Texel {
                resource: Some(resource),
                lane,
                read: Read::Sample,
                coordinate,
                controls,
            } => {
                if !controls.is_empty() {
                    return Err(refuse(
                        "texel sample",
                        "a sample with a level, bias, gradient or offset",
                    ));
                }
                Ok(Leaf::Texel(
                    Sample {
                        resource: *resource,
                        coordinate: coordinate.clone(),
                    },
                    *lane,
                ))
            }
            Node::Input { input, lane } => {
                let label = &self.program.interface.inputs[*input].label;
                match self.freeze.input(&label.name, label.index, *lane) {
                    Some(InputLink::Stream { semantic, channel }) => Ok(Leaf::Stream(semantic, channel)),
                    // a word would have been evaluated above
                    _ => Err(refuse(
                        "input",
                        format!(
                            "{}{} lane {lane} is not linked to the mesh",
                            label.name, label.index
                        ),
                    )),
                }
            }
            other => Err(refuse(
                node_kind(other),
                format!(
                    "{} {id:?} is not known where the freeze holds it",
                    node_kind(other)
                ),
            )),
        }
    }

    /// The terms of value `node`, each leaf as the reading takes it.
    fn read(
        &mut self,
        node: NodeId,
        stop: &BTreeSet<NodeId>,
    ) -> Result<(Scalar, BTreeMap<NodeId, Leaf>), Refusal> {
        let scalar = Scalar::of(self.values, node, stop);
        let mut leaves = BTreeMap::new();
        for id in scalar.leaves() {
            if !stop.contains(&id) {
                leaves.insert(id, self.leaf(id)?);
            }
        }
        Ok((scalar, leaves))
    }

    /// The texture a sample reads and the UV set and transform it reads it at.
    fn texture(
        &mut self,
        sample: &Sample,
        unstated: &mut BTreeSet<ResourceId>,
    ) -> Result<TextureRef, Refusal> {
        let bound = self
            .stage
            .textures
            .iter()
            .find(|t| t.resource == sample.resource)
            .ok_or_else(|| refuse("texture", format!("texture {:?} has no binding", sample.resource)))?;
        let (texture, sampler) = match &bound.source {
            TextureSource::Texture { texture, sampler } => (*texture, sampler),
            other => {
                return Err(refuse(
                    "texture",
                    format!("texture {:?} binds no texture: {other:?}", sample.resource),
                ));
            }
        };
        let [u, v] = sample.coordinate[..] else {
            return Err(refuse(
                "coordinate",
                format!("a sample of {} coordinates", sample.coordinate.len()),
            ));
        };
        let (su, ou, set_u) = self.coordinate(u, 0)?;
        let (sv, ov, set_v) = self.coordinate(v, 1)?;
        if set_u != set_v {
            return Err(refuse(
                "coordinate",
                format!("u from UV set {set_u} and v from {set_v}"),
            ));
        }
        let transform = UvTransform {
            offset: [ou, ov],
            scale: [su, sv],
            rotation: 0.0,
        };
        let mut wrap = [Wrap::Repeat; 2];
        for (axis, address) in sampler.wrap[..2].iter().enumerate() {
            wrap[axis] = match address {
                None => {
                    unstated.insert(sample.resource);
                    Wrap::Repeat
                }
                Some(AddressMode::Repeat) => Wrap::Repeat,
                Some(AddressMode::Mirror) => Wrap::Mirror,
                Some(AddressMode::Clamp) => Wrap::Clamp,
                Some(other) => {
                    return Err(refuse(
                        "texture",
                        format!(
                            "texture {:?} is addressed {other:?}, which no wrap is",
                            sample.resource
                        ),
                    ));
                }
            };
        }
        Ok(TextureRef {
            texture,
            texcoord: set_u,
            wrap,
            transform: (transform != UvTransform::IDENTITY).then_some(transform),
            filters: sampler.filters,
        })
    }

    /// A sample's coordinate lane `axis` (0 u, 1 v): `scale * uv + offset` of that lane of one UV set, as a transform
    /// computes it (the product, then the sum): (scale, offset, the set). The program may compute it in more steps where
    /// they give the same bits: a chain of products by constants is one scale where each step's factor, or the scale
    /// so far, is a power of two (scaling by one is exact, so the product rounds once, in the normal range; D3D flushes
    /// the subnormals), and a chain of sums is one offset where every offset but one is zero.
    fn coordinate(&mut self, node: NodeId, axis: u8) -> Result<(f32, f32, u8), Refusal> {
        let (scalar, leaves) = self.read(node, &BTreeSet::new())?;
        let terms = &scalar.terms;
        let constant = |i: usize| match terms.get(i) {
            Some(Op::Constant(c)) => Some(*c),
            Some(Op::Leaf(n)) => match leaves.get(n) {
                Some(Leaf::Constant(c)) => Some(*c),
                _ => None,
            },
            _ => None,
        };
        let set = |i: usize| match terms.get(i) {
            Some(Op::Leaf(n)) => match leaves.get(n) {
                Some(Leaf::Stream(Semantic::TexCoord(set), lane)) if *lane == axis => Some(*set),
                _ => None,
            },
            _ => None,
        };
        let power_of_two = |x: f32| x.is_normal() && x.to_bits() & 0x007f_ffff == 0;
        // term `i` as (scale, offset, set), computed with the same bits as `scale * uv + offset`
        fn affine(
            i: usize,
            terms: &[Op],
            constant: &dyn Fn(usize) -> Option<f32>,
            set: &dyn Fn(usize) -> Option<u8>,
            power_of_two: &dyn Fn(f32) -> bool,
        ) -> Option<(f32, f32, u8)> {
            if let Some(s) = set(i) {
                return Some((1.0, 0.0, s));
            }
            let operands = |a: usize, b: usize| match (constant(a), constant(b)) {
                (Some(c), None) => Some((c, b)),
                (None, Some(c)) => Some((c, a)),
                _ => None,
            };
            match terms.get(i)? {
                Op::Multiply([a, b]) => {
                    let (c, other) = operands(*a, *b)?;
                    let (k, o, s) = affine(other, terms, constant, set, power_of_two)?;
                    // a product of an offset coordinate is not one scale and offset of the same bits
                    let exact = o == 0.0 && (k == 1.0 || power_of_two(k) || power_of_two(c));
                    exact.then_some((k * c, 0.0, s))
                }
                Op::Add([a, b]) => {
                    let (c, other) = operands(*a, *b)?;
                    let (k, o, s) = affine(other, terms, constant, set, power_of_two)?;
                    (o == 0.0 || c == 0.0).then_some((k, o + c, s))
                }
                // less a constant is plus its negation, bit for bit
                Op::Subtract([a, b]) => {
                    let c = -constant(*b)?;
                    let (k, o, s) = affine(*a, terms, constant, set, power_of_two)?;
                    (o == 0.0 || c == 0.0).then_some((k, o + c, s))
                }
                _ => None,
            }
        }
        affine(
            terms.len().saturating_sub(1),
            terms,
            &constant,
            &set,
            &power_of_two,
        )
        .ok_or_else(|| {
            refuse(
                "coordinate",
                format!("coordinate lane {axis} is no scale and offset of a UV set: {terms:?}"),
            )
        })
    }
}

/// An expression of `scalar`'s terms, its leaves made sources by `source` (or constants); `None` where a term's
/// operand is not an earlier term.
fn expression<S: Clone + PartialEq + Serialize>(
    scalar: &Scalar,
    leaves: &BTreeMap<NodeId, Leaf>,
    mut source: impl FnMut(&Leaf) -> Result<Option<S>, Refusal>,
) -> Result<Expression<S>, Refusal> {
    let mut b = ExpressionBuilder::<S>::new();
    let mut at: Vec<TermId> = Vec::with_capacity(scalar.terms.len());
    for t in &scalar.terms {
        let get = |i: &usize| {
            at.get(*i)
                .copied()
                .ok_or_else(|| refuse("term", "an operand after its term"))
        };
        let id = match t {
            Op::Leaf(n) => {
                let leaf = leaves
                    .get(n)
                    .ok_or_else(|| refuse("term", format!("leaf {n:?} not read")))?;
                match leaf {
                    Leaf::Constant(c) => b.constant(*c),
                    other => match source(other)? {
                        Some(s) => b.source(s),
                        None => return Err(refuse("term", format!("{other:?} is no source here"))),
                    },
                }
            }
            Op::Constant(c) if c.is_finite() => b.constant(*c),
            Op::Constant(c) => return Err(refuse("constant", format!("a literal {c}"))),
            Op::Add([x, y]) => b.add(get(x)?, get(y)?),
            Op::Subtract([x, y]) => b.subtract(get(x)?, get(y)?),
            Op::Multiply([x, y]) => b.multiply(get(x)?, get(y)?),
            Op::Min([x, y]) => b.min(get(x)?, get(y)?),
            Op::Max([x, y]) => b.max(get(x)?, get(y)?),
            Op::Saturate(x) => b.saturate(get(x)?),
        };
        at.push(id);
    }
    let root = *at.last().ok_or_else(|| refuse("term", "a value of no terms"))?;
    b.expression(root).map_err(|why| refuse("expression", why))
}

/// One colour lane of a layer: the sample it reads and the expression of its lanes.
struct LaneColour {
    sample: Sample,
    expression: Expression<ColorLane>,
}

/// The pixel program `program`'s albedo as layers ([`LayeredAlbedo`]), read under `stage`'s roles and words and the
/// host's `freeze`; or why it is not.
pub fn read_layered_albedo(
    program: &Program,
    stage: &StageBinding,
    freeze: &Freeze,
) -> Result<LayeredAlbedo, Refusal> {
    let values = Values::of_program(program).map_err(|why| refuse("graph", why))?;
    let surface = Surface::read(stage.outputs.iter().map(|o| (o.lanes, &o.role)));
    let place = match surface.base {
        Some(Base::Albedo(place)) if place.width() >= 3 => place,
        _ => return Err(refuse("no albedo", "no output holds an albedo of three lanes")),
    };
    let label = &stage.outputs[place.output].label;
    let output = program
        .interface
        .outputs
        .iter()
        .position(|o| o.label.index == label.index && o.label.name.eq_ignore_ascii_case(&label.name))
        .ok_or_else(|| {
            refuse(
                "no albedo",
                format!("the program writes no {}{}", label.name, label.index),
            )
        })?;
    let lanes: Vec<NodeId> = (0..3u8)
        .map(|k| {
            let lane = u8::try_from(place.first()).unwrap_or(0) + k;
            values
                .output(output, lane)
                .ok_or_else(|| refuse("no albedo", format!("albedo lane {k} is not written")))
        })
        .collect::<Result<_, _>>()?;

    let mut known = stage.known_words();
    known.extend(freeze.words.iter().map(|(k, v)| (*k, *v)));
    let interface = &program.interface;
    let leaf = |_: NodeId, node: &Node, at: &[u32]| -> Option<u32> {
        match node {
            Node::Input { input, lane } => {
                let label = &interface.inputs[*input].label;
                match freeze.input(&label.name, label.index, *lane)? {
                    InputLink::Word(w) => Some(w),
                    InputLink::Stream { .. } => None,
                }
            }
            Node::Global {
                resource: Some(resource),
                at: place,
            } => {
                let r = interface.resource(*resource)?;
                if !matches!(r.kind, ResourceKind::ConstantBuffer { .. }) {
                    return None;
                }
                let byte = match place {
                    At::Component(word) => word.checked_mul(4)?,
                    // a constant buffer's rows of four words, indexed from its start
                    At::Indexed { lane, .. } => match at {
                        [row] => row.checked_mul(16)?.checked_add(lane.checked_mul(4)?)?,
                        _ => return None,
                    },
                };
                known.get(&(*resource, byte)).copied()
            }
            _ => None,
        }
    };
    let mut reader = Reader {
        program,
        stage,
        freeze,
        values: &values,
        evaluator: Evaluator::new(&values, program.modes.flush_denormals, leaf),
    };

    // under each lane, the chain; its texture layers and weights, the same in the three lanes
    let mut saturate = None;
    let mut columns: [Vec<LaneColour>; 3] = Default::default();
    let mut weights: Option<Vec<NodeId>> = None;
    for (k, &lane) in lanes.iter().enumerate() {
        let (saturated, lane_weights, lane_colours) = match Over::under(&values, lane) {
            Some(over) => {
                let top = over.layers.last().map_or(over.base, |l| l.node);
                let saturated = outside(&mut reader, lane, top, k)?;
                // the bottom layer's colour: the base, or a constant blend over it directly above it
                let mut bottom = over.base;
                let mut rest = over.layers.as_slice();
                while let [first, more @ ..] = rest
                    && constant(&mut reader, first.weight)?
                    && constant(&mut reader, first.value)?
                {
                    bottom = first.node;
                    rest = more;
                }
                let mut lane_weights = Vec::with_capacity(rest.len());
                let mut lane_colours = vec![colour(&mut reader, bottom, k)?];
                for layer in rest {
                    lane_weights.push(layer.weight);
                    lane_colours.push(colour(&mut reader, layer.value, k)?);
                }
                (saturated, lane_weights, lane_colours)
            }
            // no interpolation: one layer, laid over nothing
            None => {
                let (scalar, leaves) = reader.read(lane, &BTreeSet::new())?;
                let (saturated, top) = unblended(&scalar, &leaves);
                let colour = colour_of(&below(&scalar, top), &leaves, k)?;
                (saturated, Vec::new(), vec![colour])
            }
        };
        match saturate {
            Some(before) if before != saturated => {
                return Err(refuse("outside", "the albedo's lanes saturate differently"));
            }
            _ => saturate = Some(saturated),
        }
        match &weights {
            None => weights = Some(lane_weights),
            Some(w) if *w == lane_weights => {}
            Some(_) => {
                return Err(refuse(
                    "per-lane weight",
                    "the albedo's lanes are blended by different weights or layers",
                ));
            }
        }
        columns[k] = lane_colours;
    }
    let weights = weights.unwrap_or_default();

    let mut unstated = BTreeSet::new();
    let mut layers = Vec::with_capacity(weights.len() + 1);
    let [red, green, blue] = &columns;
    for (i, ((r, g), b)) in red.iter().zip(green).zip(blue).enumerate() {
        if r.sample != g.sample || r.sample != b.sample {
            return Err(refuse(
                "layer sample",
                format!("layer {i}'s lanes read different samples"),
            ));
        }
        let texture = reader.texture(&r.sample, &mut unstated)?;
        let lanes = [r.expression.clone(), g.expression.clone(), b.expression.clone()];
        let (factor, color) = match factor(&lanes) {
            Some(factor) => (factor, None),
            None => ([1.0; 3], Some(lanes)),
        };
        layers.push(AlbedoLayer {
            texture,
            factor,
            color,
        });
    }

    let mut channels = Vec::with_capacity(layers.len());
    let mut base = ExpressionBuilder::new();
    let one = base.constant(1.0);
    channels.push(base.channel(one).map_err(|why| refuse("expression", why))?);
    for &w in &weights {
        let (scalar, leaves) = reader.read(w, &BTreeSet::new())?;
        let mut textures = BTreeMap::new();
        for leaf in leaves.values() {
            if let Leaf::Texel(sample, _) = leaf
                && !textures.contains_key(sample)
            {
                textures.insert(sample.clone(), reader.texture(sample, &mut unstated)?);
            }
        }
        let e = expression(&scalar, &leaves, |leaf| {
            Ok(match leaf {
                Leaf::Stream(semantic, channel) => Some(Source::Vertex {
                    semantic: *semantic,
                    channel: *channel,
                }),
                Leaf::Texel(sample, lane) => {
                    let mut channels = [0.0; 4];
                    channels[usize::from(*lane)] = 1.0;
                    Some(Source::Mask {
                        texture: textures[sample],
                        channels,
                        bias: 0.0,
                    })
                }
                Leaf::Constant(_) => None,
            })
        })?;
        channels.push(Channel::from_expression(&e).map_err(|why| refuse("expression", why))?);
    }
    Ok(LayeredAlbedo {
        layers,
        weights: channels,
        saturate: saturate.unwrap_or(false),
        unstated_wrap: unstated.len(),
    })
}

/// Whether `node` is a constant where the freeze holds it.
fn constant<L: FnMut(NodeId, &Node, &[u32]) -> Option<u32>>(
    r: &mut Reader<'_, L>,
    node: NodeId,
) -> Result<bool, Refusal> {
    match r.evaluator.evaluate(node) {
        Ok(Evaluated::Word(_)) => Ok(true),
        Ok(Evaluated::Unknown(_)) => Ok(false),
        Err(why) => Err(refuse(node_kind(r.values.node(node)), why)),
    }
}

/// A layer's colour in lane `k`: value `node` as an expression of one sample's lanes.
fn colour<L: FnMut(NodeId, &Node, &[u32]) -> Option<u32>>(
    r: &mut Reader<'_, L>,
    node: NodeId,
    k: usize,
) -> Result<LaneColour, Refusal> {
    let (scalar, leaves) = r.read(node, &BTreeSet::new())?;
    colour_of(&scalar, &leaves, k)
}

/// A layer's colour in lane `k`: `scalar` (its leaves as `leaves` takes them) as an expression of one sample's lanes.
fn colour_of(scalar: &Scalar, leaves: &BTreeMap<NodeId, Leaf>, k: usize) -> Result<LaneColour, Refusal> {
    let mut sample = None;
    for id in scalar.leaves() {
        let leaf = leaves
            .get(&id)
            .ok_or_else(|| refuse("term", format!("leaf {id:?} not read")))?;
        match leaf {
            Leaf::Texel(s, _) if sample.is_none() => sample = Some(s.clone()),
            Leaf::Texel(s, _) if sample.as_ref() == Some(s) => {}
            Leaf::Texel(..) => {
                return Err(refuse(
                    "layer sample",
                    format!("a layer's colour in lane {k} reads two samples"),
                ));
            }
            Leaf::Stream(semantic, channel) => {
                return Err(refuse(
                    "layer stream",
                    format!("a layer's colour in lane {k} reads {semantic:?} lane {channel}"),
                ));
            }
            Leaf::Constant(_) => {}
        }
    }
    let sample = sample.ok_or_else(|| {
        refuse(
            "layer sample",
            format!("a layer's colour in lane {k} reads no texture"),
        )
    })?;
    let expression = expression(scalar, leaves, |leaf| {
        Ok(match leaf {
            Leaf::Texel(_, lane) => Some(ColorLane(*lane)),
            Leaf::Stream(..) | Leaf::Constant(_) => None,
        })
    })?;
    Ok(LaneColour { sample, expression })
}

/// An albedo lane with no interpolation under it (`scalar`, its leaves as `leaves` takes them): one layer's colour
/// under what lies outside it, as [`outside`] takes a blend's: whether a saturate is, and the term the colour is.
/// Outside are a saturate, once, and products by a factor of exactly 1 where the freeze holds it; the colour is the
/// first term that is neither, so a product by any other constant is the colour's own (a texture times its factor).
fn unblended(scalar: &Scalar, leaves: &BTreeMap<NodeId, Leaf>) -> (bool, usize) {
    let terms = &scalar.terms;
    let one = |i: usize| {
        let c = match terms.get(i) {
            Some(Op::Constant(c)) => Some(*c),
            Some(Op::Leaf(n)) => match leaves.get(n) {
                Some(Leaf::Constant(c)) => Some(*c),
                _ => None,
            },
            _ => None,
        };
        c.is_some_and(|c| c.to_bits() == 1.0f32.to_bits())
    };
    let mut at = terms.len().saturating_sub(1);
    let mut saturate = false;
    loop {
        match terms.get(at) {
            Some(Op::Saturate(x)) if !saturate => {
                saturate = true;
                at = *x;
            }
            Some(Op::Multiply([a, b])) if one(*a) => at = *b,
            Some(Op::Multiply([a, b])) if one(*b) => at = *a,
            _ => return (saturate, at),
        }
    }
}

/// The value of term `t` of `scalar` as a scalar of its own: the terms it is made of, in their order.
fn below(scalar: &Scalar, t: usize) -> Scalar {
    let terms = &scalar.terms[..=t.min(scalar.terms.len().saturating_sub(1))];
    let operands = |term: &Op| -> Vec<usize> {
        match *term {
            Op::Add([a, b])
            | Op::Subtract([a, b])
            | Op::Multiply([a, b])
            | Op::Min([a, b])
            | Op::Max([a, b]) => {
                vec![a, b]
            }
            Op::Saturate(a) => vec![a],
            Op::Leaf(_) | Op::Constant(_) => Vec::new(),
        }
    };
    // operands come before their terms: one pass down from the top marks every term it is made of
    let mut used = vec![false; terms.len()];
    if let Some(top) = used.last_mut() {
        *top = true;
    }
    for i in (0..terms.len()).rev() {
        if used[i] {
            for o in operands(&terms[i]) {
                if let Some(u) = used.get_mut(o) {
                    *u = true;
                }
            }
        }
    }
    let mut index = vec![0; terms.len()];
    let mut out = Vec::new();
    for (i, term) in terms.iter().enumerate() {
        if !used[i] {
            continue;
        }
        let at = |o: usize| index.get(o).copied().unwrap_or(usize::MAX);
        let renumbered = match *term {
            Op::Add([a, b]) => Op::Add([at(a), at(b)]),
            Op::Subtract([a, b]) => Op::Subtract([at(a), at(b)]),
            Op::Multiply([a, b]) => Op::Multiply([at(a), at(b)]),
            Op::Min([a, b]) => Op::Min([at(a), at(b)]),
            Op::Max([a, b]) => Op::Max([at(a), at(b)]),
            Op::Saturate(a) => Op::Saturate(at(a)),
            leaf @ (Op::Leaf(_) | Op::Constant(_)) => leaf,
        };
        index[i] = out.len();
        out.push(renumbered);
    }
    Scalar { terms: out }
}

/// Each lane its own sample lane times a constant (1 for the lane itself): those constants.
fn factor(lanes: &[Expression<ColorLane>; 3]) -> Option<[f32; 3]> {
    use crate::layered::Term;
    let mut out = [1.0; 3];
    for (k, e) in lanes.iter().enumerate() {
        let own = ColorLane(u8::try_from(k).ok()?);
        if e.sources.as_slice() != [own] {
            return None;
        }
        out[k] = match e.terms.as_slice() {
            [Term::Source(0)] => 1.0,
            [Term::Constant(c), Term::Source(0), Term::Multiply(_)] => *c,
            _ => return None,
        };
    }
    Some(out)
}

/// What albedo lane `k` (`lane`) makes of the blend `top` under it: whether it saturates it. Every factor outside the
/// blend must be exactly 1.
fn outside<L: FnMut(NodeId, &Node, &[u32]) -> Option<u32>>(
    r: &mut Reader<'_, L>,
    lane: NodeId,
    top: NodeId,
    k: usize,
) -> Result<bool, Refusal> {
    let stop = BTreeSet::from([top]);
    let scalar = Scalar::of(r.values, lane, &stop);
    let terms = &scalar.terms;
    // a leaf outside the blend is a factor's: its word where the freeze holds it, or why it has none
    let mut words = BTreeMap::new();
    for id in scalar.leaves().into_iter().filter(|id| *id != top) {
        let word = match r.evaluator.evaluate(id) {
            Ok(Evaluated::Word(w)) => Ok(f32::from_bits(w)),
            Ok(Evaluated::Unknown(leaf)) => Err(format!(
                "{} {id:?} needs {} {leaf:?}, which the freeze does not state",
                node_kind(r.values.node(id)),
                node_kind(r.values.node(leaf))
            )),
            Err(why) => Err(format!("{} {id:?}: {why}", node_kind(r.values.node(id)))),
        };
        words.insert(id, word);
    }
    let one = |i: usize| -> Option<Result<f32, String>> {
        match terms.get(i)? {
            Op::Constant(c) => Some(Ok(*c)),
            Op::Leaf(n) => words.get(n).cloned(),
            _ => None,
        }
    };
    let mut at = terms.len().saturating_sub(1);
    let mut saturate = false;
    loop {
        match terms.get(at) {
            Some(Op::Leaf(n)) if *n == top => break,
            Some(Op::Saturate(x)) if !saturate => {
                saturate = true;
                at = *x;
            }
            Some(Op::Multiply([a, b])) => {
                let (factor, other) = match (one(*a), one(*b)) {
                    (Some(f), _) => (f, *b),
                    (_, Some(f)) => (f, *a),
                    _ => {
                        return Err(refuse(
                            "factor",
                            format!("albedo lane {k} multiplies its blend by a value made of others"),
                        ));
                    }
                };
                let factor = factor.map_err(|why| {
                    refuse(
                        "factor",
                        format!(
                            "albedo lane {k} multiplies its blend by a factor the freeze does not give: {why}"
                        ),
                    )
                })?;
                if factor.to_bits() != 1.0f32.to_bits() {
                    return Err(refuse(
                        "factor",
                        format!("albedo lane {k} multiplies its blend by {factor} where the freeze holds it"),
                    ));
                }
                at = other;
            }
            other => {
                return Err(refuse(
                    "outside",
                    format!("albedo lane {k} makes {other:?} of its blend, not a saturate or a factor"),
                ));
            }
        }
    }
    Ok(saturate)
}
