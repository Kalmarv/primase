//! What a program's outputs are made of: a dataflow query over the lifted module, with no game in it.
//!
//! [`Values`] is the entry point's value graph, lane by lane: each output lane and each discard's condition as a node
//! made of inputs, texels, constants and the operations between them. The module's local variables are seen through
//! (a lifter keeps registers and phis in locals: a load is the value the last store left there), a branch whose arms
//! leave different values is a [`Node::Merge`] that names its condition, a loop-carried value a [`Node::Loop`] (the
//! graph's only cycles) and a bitcast is no operation (the bits are the value). Equal nodes are one node, so a value
//! used twice is shared.
//!
//! [`Flow`] is what may reach each output lane and the discard, and how ([`Via`]): an input lane or a texel lane
//! through arithmetic (its value), only as a texel's coordinate, or only through a condition. It is a may-reach
//! over-approximation: a constant that is zero at run time still carries what it multiplies, and the answer is never
//! "must reach". [`link`] takes a pixel program's flow back through the vertex program that feeds it.
//!
//! Not followed, and stated: a write to a storage buffer or image into a later read of it (a read is the resource's
//! own value). Refused with an error, never dropped: a call (the lifters emit one function), and the ray-query and
//! cooperative-matrix expressions.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use naga::{ArraySize, Expression, Handle, Statement, TypeInner};
/// The module's operations a [`Op`] names, re-exported so a consumer matches them without naming naga.
pub use naga::{
    BinaryOperator, DerivativeAxis, DerivativeControl, MathFunction, RelationalFunction, ScalarKind,
    UnaryOperator,
};

use crate::{Interface, Label, Program, ResourceId, Stage, entry_io};

/// A node of a [`Values`] graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

/// A literal's value: integers, floats and booleans (0 or 1) alike. Equal when the bits are.
#[derive(Clone, Copy, Debug)]
pub struct Literal(pub f64);
impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for Literal {}
impl std::hash::Hash for Literal {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

/// How a texel is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Read {
    /// Filtered at a coordinate.
    Sample,
    /// The four texels around a coordinate, of this lane.
    Gather(u8),
    /// One texel by its integer address.
    Load,
}

/// Where in a global a component is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum At {
    /// The component's place in the global's lanes, in order (a constant buffer's row `r`, lane `l` is `4r + l`).
    Component(u32),
    /// A lane of an element the program indexes at run time, by these index values.
    Indexed { lane: u32, index: Vec<NodeId> },
}

/// An operation of the module ([`Node::Op`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Unary(UnaryOperator),
    Binary(BinaryOperator),
    Math(MathFunction),
    Relational(RelationalFunction),
    /// `select(reject, accept, condition)`: the operands are the condition, the accepted value and the rejected one.
    Select,
    Derivative(DerivativeAxis, DerivativeControl),
    /// A conversion to another scalar kind (not a bitcast, which leaves a value as it is).
    Convert(ScalarKind),
    /// One of the operands chosen by an index computed at run time: the candidates, then the index's lanes.
    Index,
    /// A subgroup operation over the operand's values in other invocations.
    Subgroup,
}

/// What a value lane is made of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    /// A lane of an input varying: its index in `Interface::inputs`.
    Input {
        input: usize,
        lane: u8,
    },
    /// A component of a module global that is not a texture: a constant buffer's word, a buffer's, constant data the
    /// module carries. `resource` is the interface's resource it is bound as, `None` for a global no binding names.
    Global {
        resource: Option<ResourceId>,
        at: At,
    },
    /// A lane of a texel a texture read returns. `coordinate` is the coordinate's (or address's) lanes in order;
    /// `controls` the rest that decides which texel: array layer, level, bias, gradients, offset, compare value.
    Texel {
        resource: Option<ResourceId>,
        lane: u8,
        read: Read,
        coordinate: Vec<NodeId>,
        controls: Vec<NodeId>,
    },
    /// What the program asks of a resource that is not its contents: an image's size, levels or samples, a buffer's
    /// length.
    Query {
        resource: Option<ResourceId>,
    },
    Literal(Literal),
    Op {
        op: Op,
        operands: Vec<NodeId>,
    },
    /// After a branch whose arms leave different values here: one of `arms`, as `condition` (the branch's condition
    /// or switch selector, and those of the branches it is in) decides.
    Merge {
        condition: Vec<NodeId>,
        arms: Vec<NodeId>,
    },
    /// A value a loop carries: one of `arms` (its value entering the loop and after each way an iteration ends),
    /// for as many iterations as `exit` (the conditions the loop is left by) decides.
    Loop {
        arms: Vec<NodeId>,
        exit: Vec<NodeId>,
    },
}

/// The entry point of a program as a value graph ([`Node`]): each output lane's node and each discard's conditions.
#[derive(Clone, Debug)]
pub struct Values {
    stage: Stage,
    nodes: Vec<Node>,
    /// (index in `Interface::outputs`, lane) -> its value.
    outputs: BTreeMap<(usize, u8), NodeId>,
    /// The conditions guarding each discard, outermost first; empty for one taken unconditionally.
    discards: Vec<Vec<NodeId>>,
}

impl Values {
    /// The value graph of `program`'s entry point. An error names what it cannot follow.
    pub fn of_program(program: &Program) -> Result<Values, String> {
        Walker::new(program)?.run()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    /// The node of output `output` (its index in `Interface::outputs`) lane `lane`, where the program writes it.
    pub fn output(&self, output: usize, lane: u8) -> Option<NodeId> {
        self.outputs.get(&(output, lane)).copied()
    }

    /// Every output lane written, by (index in `Interface::outputs`, lane).
    pub fn outputs(&self) -> &BTreeMap<(usize, u8), NodeId> {
        &self.outputs
    }

    /// The node of the output labelled `label`, lane `lane`.
    pub fn output_labelled(&self, interface: &Interface, label: &Label, lane: u8) -> Option<NodeId> {
        self.output(labelled(&interface.outputs, label)?, lane)
    }

    /// The conditions guarding each discard, outermost first (empty: taken whenever it is reached).
    pub fn discards(&self) -> &[Vec<NodeId>] {
        &self.discards
    }

    /// The nodes `id` is made of, as [`Node`] lists them: operands, a texel's coordinate and controls, an index, a
    /// merge's conditions and arms, a loop's arms and exits.
    pub fn operands(&self, id: NodeId) -> Vec<NodeId> {
        match self.node(id) {
            Node::Input { .. } | Node::Query { .. } | Node::Literal(_) => Vec::new(),
            Node::Global { at, .. } => match at {
                At::Component(_) => Vec::new(),
                At::Indexed { index, .. } => index.clone(),
            },
            Node::Texel {
                coordinate, controls, ..
            } => coordinate.iter().chain(controls).copied().collect(),
            Node::Op { operands, .. } => operands.clone(),
            Node::Merge { condition, arms } => condition.iter().chain(arms).copied().collect(),
            Node::Loop { arms, exit } => arms.iter().chain(exit).copied().collect(),
        }
    }
}

/// How a source reaches what it reaches, weakest first: only through a condition, only as a texel's coordinate (or
/// its level, offset or compare value), or through its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// Only through a condition: a branch or loop exit deciding the value, or a discard's.
    Control,
    /// Only as what decides which texel is read.
    Coordinate,
    /// Through arithmetic, selects, swizzles, conversions and local variables: the value is computed from it.
    Value,
}

/// One input lane: its index in `Interface::inputs` and the lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lane {
    pub input: usize,
    pub lane: u8,
}

/// A lane of a texel read from a resource of the program of `stage` (after [`link`], the pixel or the vertex
/// program's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TexelLane {
    pub stage: Stage,
    pub resource: ResourceId,
    pub lane: u8,
}

/// What may reach one output lane or the discard, each source with the strongest [`Via`] of its paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    pub inputs: BTreeMap<Lane, Via>,
    pub texels: BTreeMap<TexelLane, Via>,
    /// After [`link`]: lanes of the pixel program's inputs no vertex output feeds (a front-face flag, a sample
    /// index), by the pixel interface.
    pub unlinked: BTreeMap<Lane, Via>,
}

/// What may reach each output lane and each discard of an entry point ([`Reach`]). A may-reach over-approximation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flow {
    outputs: BTreeMap<(usize, u8), Reach>,
    discard: Reach,
}

impl Flow {
    /// The flow of `program`'s entry point. An error names what the walk cannot follow, never a silent "no
    /// dependency".
    pub fn of_program(program: &Program) -> Result<Flow, String> {
        Ok(Flow::of_values(&Values::of_program(program)?))
    }

    /// The flow of a value graph.
    pub fn of_values(values: &Values) -> Flow {
        let mut flow = Flow::default();
        for (&key, &node) in &values.outputs {
            flow.outputs.insert(key, reach(values, &[(node, Via::Value)]));
        }
        let conditions: Vec<(NodeId, Via)> = values
            .discards
            .iter()
            .flatten()
            .map(|&c| (c, Via::Control))
            .collect();
        flow.discard = reach(values, &conditions);
        flow
    }

    /// The input lanes that may reach output `label` lane `lane` (`SV_Target0`.w is `SV_Target`, 0, lane 3).
    pub fn reaching(&self, interface: &Interface, label: &Label, lane: u8) -> Vec<(Lane, Via)> {
        self.reach_of(interface, label, lane)
            .map(|r| r.inputs.iter().map(|(l, v)| (*l, *v)).collect())
            .unwrap_or_default()
    }

    /// The texel lanes that may reach output `label` lane `lane`.
    pub fn texels_reaching(&self, interface: &Interface, label: &Label, lane: u8) -> Vec<(TexelLane, Via)> {
        self.reach_of(interface, label, lane)
            .map(|r| r.texels.iter().map(|(t, v)| (*t, *v)).collect())
            .unwrap_or_default()
    }

    /// Everything that may reach output `label` lane `lane`, `None` where the program does not write it.
    pub fn reach_of(&self, interface: &Interface, label: &Label, lane: u8) -> Option<&Reach> {
        self.outputs.get(&(labelled(&interface.outputs, label)?, lane))
    }

    /// Every output lane's reach, by (index in `Interface::outputs`, lane).
    pub fn outputs(&self) -> &BTreeMap<(usize, u8), Reach> {
        &self.outputs
    }

    /// The input lanes that may reach a discard's condition: whether the pixel is drawn at all.
    pub fn discard(&self) -> &BTreeMap<Lane, Via> {
        &self.discard.inputs
    }

    /// Everything that may reach a discard's condition.
    pub fn discard_reach(&self) -> &Reach {
        &self.discard
    }
}

/// A pixel program's flow in terms of the vertex program's inputs: each pixel input lane taken back through the
/// vertex output of the same label (D3D links the stages by semantic name and index; a lane is the same lane of the
/// element on both sides), with the weaker of the two paths' [`Via`]. A pixel input no vertex output carries stays
/// as an `unlinked` pixel lane; texels keep the stage they are read in.
pub fn link(vertex: (&Interface, &Flow), pixel: (&Interface, &Flow)) -> Result<Flow, String> {
    let (vertex_interface, vertex_flow) = vertex;
    let (pixel_interface, pixel_flow) = pixel;
    if vertex_interface.stage != Stage::Vertex || pixel_interface.stage != Stage::Pixel {
        return Err(format!(
            "link takes a vertex and a pixel program, not {:?} and {:?}",
            vertex_interface.stage, pixel_interface.stage
        ));
    }
    let through = |reach: &Reach| -> Result<Reach, String> {
        let mut out = Reach {
            texels: reach.texels.clone(),
            ..Reach::default()
        };
        for (lane, via) in &reach.inputs {
            let varying = pixel_interface
                .inputs
                .get(lane.input)
                .ok_or_else(|| format!("pixel input {} is not in the interface", lane.input))?;
            let fed = labelled(&vertex_interface.outputs, &varying.label)
                .and_then(|o| vertex_flow.outputs.get(&(o, lane.lane)));
            match fed {
                Some(vertex_reach) => {
                    for (l, v) in &vertex_reach.inputs {
                        strongest(&mut out.inputs, *l, (*v).min(*via));
                    }
                    for (t, v) in &vertex_reach.texels {
                        strongest(&mut out.texels, *t, (*v).min(*via));
                    }
                }
                None => strongest(&mut out.unlinked, *lane, *via),
            }
        }
        Ok(out)
    };
    let mut flow = Flow::default();
    for (key, reach) in &pixel_flow.outputs {
        flow.outputs.insert(*key, through(reach)?);
    }
    flow.discard = through(&pixel_flow.discard)?;
    Ok(flow)
}

/// The index of the varying labelled `label` (D3D's semantic names are case-insensitive).
fn labelled(varyings: &[crate::Varying], label: &Label) -> Option<usize> {
    varyings
        .iter()
        .position(|v| v.label.index == label.index && v.label.name.eq_ignore_ascii_case(&label.name))
}

fn strongest<K: Ord>(map: &mut BTreeMap<K, Via>, key: K, via: Via) {
    let entry = map.entry(key).or_insert(via);
    *entry = (*entry).max(via);
}

/// What reaches from `roots`, each with the via its path starts with: a path's via is its weakest step, a source's
/// the strongest of its paths. A texel's coordinate and controls are reached as [`Via::Coordinate`], a merge's
/// condition and a loop's exits as [`Via::Control`].
fn reach(values: &Values, roots: &[(NodeId, Via)]) -> Reach {
    let mut out = Reach::default();
    let mut best: HashMap<NodeId, Via> = HashMap::new();
    let mut work: Vec<(NodeId, Via)> = Vec::new();
    let push = |best: &mut HashMap<NodeId, Via>, work: &mut Vec<(NodeId, Via)>, id: NodeId, via: Via| {
        if best.get(&id).is_none_or(|b| *b < via) {
            best.insert(id, via);
            work.push((id, via));
        }
    };
    for &(id, via) in roots {
        push(&mut best, &mut work, id, via);
    }
    while let Some((id, via)) = work.pop() {
        if best.get(&id).is_some_and(|b| *b > via) {
            continue;
        }
        match values.node(id) {
            Node::Input { input, lane } => strongest(
                &mut out.inputs,
                Lane {
                    input: *input,
                    lane: *lane,
                },
                via,
            ),
            Node::Global { at, .. } => {
                if let At::Indexed { index, .. } = at {
                    for &i in index {
                        push(&mut best, &mut work, i, via);
                    }
                }
            }
            Node::Texel {
                resource,
                lane,
                coordinate,
                controls,
                ..
            } => {
                if let Some(resource) = resource {
                    strongest(
                        &mut out.texels,
                        TexelLane {
                            stage: values.stage,
                            resource: *resource,
                            lane: *lane,
                        },
                        via,
                    );
                }
                for &c in coordinate.iter().chain(controls) {
                    push(&mut best, &mut work, c, via.min(Via::Coordinate));
                }
            }
            Node::Query { .. } | Node::Literal(_) => {}
            Node::Op { operands, .. } => {
                for &o in operands {
                    push(&mut best, &mut work, o, via);
                }
            }
            Node::Merge { condition, arms } => {
                for &a in arms {
                    push(&mut best, &mut work, a, via);
                }
                for &c in condition {
                    push(&mut best, &mut work, c, via.min(Via::Control));
                }
            }
            Node::Loop { arms, exit } => {
                for &a in arms {
                    push(&mut best, &mut work, a, via);
                }
                for &c in exit {
                    push(&mut best, &mut work, c, via.min(Via::Control));
                }
            }
        }
    }
    out
}

/// An expression's value as the walk holds it: its lanes in the type's order (a struct's members, an array's
/// elements, a matrix's columns, flattened), or a pointer (resolved where it is loaded or stored through).
#[derive(Clone, Debug)]
enum Val {
    Lanes(Vec<NodeId>),
    Pointer,
}

/// What a pointer points at: the variable, and the candidate places of its lanes (one, or one per element a run-time
/// index may choose, with the index's lanes).
struct Place {
    root: Root,
    starts: Vec<usize>,
    width: usize,
    index: Vec<NodeId>,
    /// The pointee's element lanes when a run-time index into a runtime-sized array chose it: no fixed place.
    runtime_sized: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Root {
    Local(Handle<naga::LocalVariable>),
    Global(Handle<naga::GlobalVariable>),
}

/// The module's memory the walk follows: the locals, and the private and workgroup globals the program writes.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct State {
    locals: BTreeMap<Handle<naga::LocalVariable>, Vec<NodeId>>,
    globals: BTreeMap<Handle<naga::GlobalVariable>, Vec<NodeId>>,
}

/// What a loop's body collects: the states its iterations end in, by how they end.
#[derive(Default)]
struct LoopExits {
    breaks: Vec<(State, Vec<NodeId>)>,
    continues: Vec<State>,
}

enum Target {
    Loop(LoopExits),
    Switch(Vec<State>),
}

struct Walker<'a> {
    program: &'a Program,
    module: &'a naga::Module,
    function: &'a naga::Function,
    typifier: naga::front::Typifier,
    nodes: Vec<Node>,
    interned: HashMap<Node, NodeId>,
    cache: Vec<Option<Val>>,
    arguments: Vec<Vec<NodeId>>,
    /// The result's lanes, each the (output, lane) it is.
    result_lanes: Vec<Option<(usize, u8)>>,
    returns: Vec<(Vec<NodeId>, Vec<NodeId>)>,
    discards: Vec<Vec<NodeId>>,
    control: Vec<NodeId>,
    targets: Vec<Target>,
    resources: BTreeMap<Handle<naga::GlobalVariable>, ResourceId>,
}

impl<'a> Walker<'a> {
    fn new(program: &'a Program) -> Result<Self, String> {
        let module = &program.module;
        let entry = module
            .entry_points
            .first()
            .ok_or("the module has no entry point")?;
        let function = &entry.function;
        let mut walker = Walker {
            program,
            module,
            function,
            typifier: naga::front::Typifier::new(),
            nodes: Vec::new(),
            interned: HashMap::new(),
            cache: vec![None; function.expressions.len()],
            arguments: Vec::new(),
            result_lanes: Vec::new(),
            returns: Vec::new(),
            discards: Vec::new(),
            control: Vec::new(),
            targets: Vec::new(),
            resources: program.bindings.iter().map(|(id, g)| (*g, *id)).collect(),
        };
        let io = entry_io(&program.interface)?;
        // each argument's lanes: the input varying (or varyings) its binding carries
        for (i, argument) in function.arguments.iter().enumerate() {
            let mut lanes = Vec::new();
            let members: Vec<(Option<&naga::Binding>, Handle<naga::Type>)> =
                match (&argument.binding, &module.types[argument.ty].inner) {
                    (Some(b), _) => vec![(Some(b), argument.ty)],
                    (None, TypeInner::Struct { members, .. }) => {
                        members.iter().map(|m| (m.binding.as_ref(), m.ty)).collect()
                    }
                    _ => return Err(format!("entry argument {i} has no binding")),
                };
            for (binding, ty) in members {
                let width = walker.width(&module.types[ty].inner);
                let item = io
                    .arguments
                    .iter()
                    .find(|item| Some(&item.binding) == binding)
                    .ok_or_else(|| format!("entry argument {i}: no interface input has its binding"))?;
                for k in 0..width {
                    let (input, lane) = varying_lane(&program.interface.inputs, item, k, false)?
                        .ok_or_else(|| format!("entry argument {i}: lane {k} past its input"))?;
                    lanes.push(walker.intern(Node::Input { input, lane }));
                }
            }
            walker.arguments.push(lanes);
        }
        // the result's lanes: the output varying each member (or the one result) carries
        if let Some(result) = &function.result {
            let members: Vec<(Option<&naga::Binding>, Handle<naga::Type>)> =
                match (&result.binding, &module.types[result.ty].inner) {
                    (Some(b), _) => vec![(Some(b), result.ty)],
                    (None, TypeInner::Struct { members, .. }) => {
                        members.iter().map(|m| (m.binding.as_ref(), m.ty)).collect()
                    }
                    _ => return Err("the entry point's result has no binding".into()),
                };
            for (binding, ty) in members {
                let width = walker.width(&module.types[ty].inner);
                let item = io.results.iter().find(|item| Some(&item.binding) == binding);
                for k in 0..width {
                    walker.result_lanes.push(match item {
                        Some(item) => varying_lane(&program.interface.outputs, item, k, true)?,
                        // a member the interface does not list (a position the pipeline adds): not an output
                        None => None,
                    });
                }
            }
        }
        Ok(walker)
    }

    fn run(mut self) -> Result<Values, String> {
        let (module, function) = (self.module, self.function);
        let mut state = State::default();
        for (handle, local) in function.local_variables.iter() {
            let lanes = match local.init {
                Some(init) => self.lanes(init, &state)?,
                None => self.zero(&module.types[local.ty].inner),
            };
            state.locals.insert(handle, lanes);
        }
        let end = self.block(&function.body, state)?;
        // falling off the end of a function without a result is its return
        if end.is_some() && self.function.result.is_none() {
            self.returns.push((self.control.clone(), Vec::new()));
        }
        // the result: the last return's value, each earlier one where its conditions held
        let mut result: Option<Vec<NodeId>> = None;
        for (conditions, lanes) in std::mem::take(&mut self.returns).into_iter().rev() {
            result = Some(match result {
                Some(later) if !conditions.is_empty() && later.len() == lanes.len() => lanes
                    .iter()
                    .zip(&later)
                    .map(|(&early, &late)| self.merge(conditions.clone(), vec![early, late]))
                    .collect(),
                _ => lanes,
            });
        }
        let mut outputs = BTreeMap::new();
        if let Some(result) = result {
            for (k, node) in result.into_iter().enumerate() {
                if let Some(Some(key)) = self.result_lanes.get(k) {
                    outputs.insert(*key, node);
                }
            }
        }
        Ok(Values {
            stage: self.program.stage,
            nodes: self.nodes,
            outputs,
            discards: self.discards,
        })
    }

    fn intern(&mut self, node: Node) -> NodeId {
        if let Some(&id) = self.interned.get(&node) {
            return id;
        }
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node.clone());
        self.interned.insert(node, id);
        id
    }

    fn op(&mut self, op: Op, operands: Vec<NodeId>) -> NodeId {
        self.intern(Node::Op { op, operands })
    }

    fn literal(&mut self, v: f64) -> NodeId {
        self.intern(Node::Literal(Literal(v)))
    }

    /// One of `arms` as `condition` decides; the one value where they agree.
    fn merge(&mut self, condition: Vec<NodeId>, mut arms: Vec<NodeId>) -> NodeId {
        let mut seen = BTreeSet::new();
        arms.retain(|a| seen.insert(*a));
        if arms.len() == 1 {
            return arms[0];
        }
        self.intern(Node::Merge { condition, arms })
    }

    fn zero(&mut self, ty: &TypeInner) -> Vec<NodeId> {
        let zero = self.literal(0.);
        vec![zero; self.width(ty)]
    }

    /// The number of lanes a value of `ty` has.
    fn width(&self, ty: &TypeInner) -> usize {
        let types = &self.module.types;
        match ty {
            TypeInner::Scalar(_) | TypeInner::Atomic(_) => 1,
            TypeInner::Vector { size, .. } => *size as usize,
            TypeInner::Matrix { columns, rows, .. } => *columns as usize * *rows as usize,
            TypeInner::Array { base, size, .. } | TypeInner::BindingArray { base, size } => {
                let element = self.width(&types[*base].inner);
                match size {
                    ArraySize::Constant(n) => element * n.get() as usize,
                    _ => element,
                }
            }
            TypeInner::Struct { members, .. } => members.iter().map(|m| self.width(&types[m.ty].inner)).sum(),
            TypeInner::Pointer { base, .. } => self.width(&types[*base].inner),
            TypeInner::ValuePointer { size, .. } => size.map_or(1, |s| s as usize),
            _ => 0,
        }
    }

    /// Member or element `index` of a value of `ty`: its lanes' range and its type.
    fn member(&self, ty: &TypeInner, index: u32) -> Result<(std::ops::Range<usize>, TypeInner), String> {
        let types = &self.module.types;
        let i = index as usize;
        Ok(match ty {
            TypeInner::Vector { scalar, .. } => (i..i + 1, TypeInner::Scalar(*scalar)),
            TypeInner::ValuePointer { scalar, .. } => (i..i + 1, TypeInner::Scalar(*scalar)),
            TypeInner::Matrix { rows, scalar, .. } => (
                i * *rows as usize..(i + 1) * *rows as usize,
                TypeInner::Vector {
                    size: *rows,
                    scalar: *scalar,
                },
            ),
            TypeInner::Array { base, .. } | TypeInner::BindingArray { base, .. } => {
                let inner = types[*base].inner.clone();
                let w = self.width(&inner);
                (i * w..(i + 1) * w, inner)
            }
            TypeInner::Struct { members, .. } => {
                let mut at = 0;
                for (k, m) in members.iter().enumerate() {
                    let inner = types[m.ty].inner.clone();
                    let w = self.width(&inner);
                    if k == i {
                        return Ok((at..at + w, inner));
                    }
                    at += w;
                }
                return Err(format!("member {index} of a struct of {} members", members.len()));
            }
            TypeInner::Pointer { base, .. } => return self.member(&types[*base].inner.clone(), index),
            other => return Err(format!("member {index} of {other:?}")),
        })
    }

    /// The elements of `ty` a run-time index chooses among: their count (`None`: sized at run time) and the type.
    fn elements(&self, ty: &TypeInner) -> Result<(Option<usize>, TypeInner), String> {
        let types = &self.module.types;
        Ok(match ty {
            TypeInner::Vector { size, scalar } => (Some(*size as usize), TypeInner::Scalar(*scalar)),
            TypeInner::ValuePointer { size, scalar, .. } => {
                (Some(size.map_or(1, |s| s as usize)), TypeInner::Scalar(*scalar))
            }
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            } => (
                Some(*columns as usize),
                TypeInner::Vector {
                    size: *rows,
                    scalar: *scalar,
                },
            ),
            TypeInner::Array { base, size, .. } | TypeInner::BindingArray { base, size } => (
                match size {
                    ArraySize::Constant(n) => Some(n.get() as usize),
                    _ => None,
                },
                types[*base].inner.clone(),
            ),
            TypeInner::Pointer { base, .. } => return self.elements(&types[*base].inner.clone()),
            other => return Err(format!("an index into {other:?}")),
        })
    }

    fn ty(&mut self, h: Handle<Expression>) -> Result<TypeInner, String> {
        let ctx = naga::proc::ResolveContext::with_locals(
            self.module,
            &self.function.local_variables,
            &self.function.arguments,
        );
        self.typifier
            .grow(h, &self.function.expressions, &ctx)
            .map_err(|e| format!("the type of expression {}: {e}", h.index()))?;
        Ok(self.typifier.get(h, &self.module.types).clone())
    }

    /// The number of lanes of expression `h`'s value.
    fn width_of(&mut self, h: Handle<Expression>) -> Result<usize, String> {
        let ty = self.ty(h)?;
        Ok(self.width(&ty))
    }

    /// A literal index, where the expression is one.
    fn literal_index(&self, h: Handle<Expression>) -> Option<u32> {
        match self.function.expressions.try_get(h).ok()? {
            Expression::Literal(naga::Literal::U32(v)) => Some(*v),
            Expression::Literal(naga::Literal::I32(v)) => u32::try_from(*v).ok(),
            Expression::Literal(naga::Literal::U64(v)) => u32::try_from(*v).ok(),
            Expression::Literal(naga::Literal::I64(v)) => u32::try_from(*v).ok(),
            Expression::Literal(naga::Literal::AbstractInt(v)) => u32::try_from(*v).ok(),
            _ => None,
        }
    }

    fn lanes(&mut self, h: Handle<Expression>, state: &State) -> Result<Vec<NodeId>, String> {
        match self.value(h, state)? {
            Val::Lanes(l) => Ok(l),
            Val::Pointer => Err(format!("expression {} is a pointer, used as a value", h.index())),
        }
    }

    /// The value of `h` in `state`: an emitted expression's as its emit left it, any other evaluated now.
    fn value(&mut self, h: Handle<Expression>, state: &State) -> Result<Val, String> {
        if let Some(v) = &self.cache[h.index()] {
            return Ok(v.clone());
        }
        let v = self.evaluate(h, state)?;
        self.cache[h.index()] = Some(v.clone());
        Ok(v)
    }

    /// The lanes of `h`, a value of width `width`: a scalar repeated where it is one lane.
    fn broadcast(
        &mut self,
        h: Handle<Expression>,
        width: usize,
        state: &State,
    ) -> Result<Vec<NodeId>, String> {
        let l = self.lanes(h, state)?;
        Ok(if l.len() == 1 && width > 1 {
            vec![l[0]; width]
        } else {
            l
        })
    }

    fn evaluate(&mut self, h: Handle<Expression>, state: &State) -> Result<Val, String> {
        let expression = self.function.expressions[h].clone();
        let lanes = |v: Vec<NodeId>| Ok(Val::Lanes(v));
        match expression {
            Expression::Literal(l) => {
                let v = literal_value(l);
                lanes(vec![self.literal(v)])
            }
            Expression::Constant(c) => {
                let init = self.module.constants[c].init;
                lanes(self.global_expression(init)?)
            }
            Expression::Override(_) => Err("an override is not followed".into()),
            Expression::ZeroValue(ty) => {
                let inner = self.module.types[ty].inner.clone();
                lanes(self.zero(&inner))
            }
            Expression::Compose { components, .. } => {
                let mut out = Vec::new();
                for c in components {
                    out.extend(self.lanes(c, state)?);
                }
                lanes(out)
            }
            Expression::Splat { size, value } => {
                let v = self.lanes(value, state)?;
                lanes(vec![v[0]; size as usize])
            }
            Expression::Swizzle {
                size,
                vector,
                pattern,
            } => {
                let v = self.lanes(vector, state)?;
                let out = pattern[..size as usize]
                    .iter()
                    .map(|c| v.get(*c as usize).copied().ok_or("a swizzle past the vector"))
                    .collect::<Result<Vec<_>, _>>()?;
                lanes(out)
            }
            Expression::AccessIndex { base, index } => {
                if is_pointer(&self.ty(base)?) {
                    return Ok(Val::Pointer);
                }
                let ty = self.ty(base)?;
                let v = self.lanes(base, state)?;
                let (range, _) = self.member(&ty, index)?;
                lanes(v.get(range).ok_or("an access past the value")?.to_vec())
            }
            Expression::Access { base, index } => {
                if is_pointer(&self.ty(base)?) {
                    return Ok(Val::Pointer);
                }
                let ty = self.ty(base)?;
                let v = self.lanes(base, state)?;
                if let Some(i) = self.literal_index(index) {
                    let (range, _) = self.member(&ty, i)?;
                    return lanes(v.get(range).ok_or("an access past the value")?.to_vec());
                }
                let (count, element) = self.elements(&ty)?;
                let w = self.width(&element);
                let count = count.unwrap_or(1);
                let idx = self.lanes(index, state)?;
                let mut out = Vec::new();
                for k in 0..w {
                    let mut operands: Vec<NodeId> =
                        (0..count).filter_map(|e| v.get(e * w + k).copied()).collect();
                    operands.extend(&idx);
                    out.push(self.op(Op::Index, operands));
                }
                lanes(out)
            }
            Expression::FunctionArgument(i) => lanes(
                self.arguments
                    .get(i as usize)
                    .cloned()
                    .ok_or_else(|| format!("argument {i} of the entry point"))?,
            ),
            Expression::GlobalVariable(_) | Expression::LocalVariable(_) => Ok(Val::Pointer),
            Expression::Load { pointer } => lanes(self.load(pointer, state)?),
            Expression::ImageSample {
                image,
                gather,
                coordinate,
                array_index,
                offset,
                level,
                depth_ref,
                ..
            } => {
                let (resource, mut controls) = self.image(image, state)?;
                let coordinate = self.lanes(coordinate, state)?;
                if let Some(a) = array_index {
                    controls.extend(self.lanes(a, state)?);
                }
                if let Some(o) = offset {
                    controls.extend(self.lanes(o, state)?);
                }
                match level {
                    naga::SampleLevel::Auto | naga::SampleLevel::Zero => {}
                    naga::SampleLevel::Exact(e) | naga::SampleLevel::Bias(e) => {
                        controls.extend(self.lanes(e, state)?);
                    }
                    naga::SampleLevel::Gradient { x, y } => {
                        controls.extend(self.lanes(x, state)?);
                        controls.extend(self.lanes(y, state)?);
                    }
                }
                if let Some(d) = depth_ref {
                    controls.extend(self.lanes(d, state)?);
                }
                let read = match gather {
                    Some(c) => Read::Gather(c as u8),
                    None => Read::Sample,
                };
                let width = self.width_of(h)?;
                lanes(self.texel(resource, read, coordinate, controls, width))
            }
            Expression::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => {
                let (resource, mut controls) = self.image(image, state)?;
                let coordinate = self.lanes(coordinate, state)?;
                for e in [array_index, sample, level].into_iter().flatten() {
                    controls.extend(self.lanes(e, state)?);
                }
                let width = self.width_of(h)?;
                lanes(self.texel(resource, Read::Load, coordinate, controls, width))
            }
            Expression::ImageQuery { image, .. } => {
                let (resource, _) = self.image(image, state)?;
                let width = self.width_of(h)?;
                let q = self.intern(Node::Query { resource });
                lanes(vec![q; width])
            }
            Expression::ArrayLength(e) => {
                let place = self.place(e, state)?;
                let resource = match place.root {
                    Root::Global(g) => self.resources.get(&g).copied(),
                    Root::Local(_) => None,
                };
                lanes(vec![self.intern(Node::Query { resource })])
            }
            Expression::Unary { op, expr } => {
                let v = self.lanes(expr, state)?;
                lanes(v.into_iter().map(|x| self.op(Op::Unary(op), vec![x])).collect())
            }
            Expression::Binary { op, left, right } => {
                let (lt, rt, out) = (self.ty(left)?, self.ty(right)?, self.ty(h)?);
                let width = self.width(&out);
                if matches!(lt, TypeInner::Matrix { .. }) || matches!(rt, TypeInner::Matrix { .. }) {
                    // a matrix product: every lane of the result from every lane of both
                    let mut all = self.lanes(left, state)?;
                    all.extend(self.lanes(right, state)?);
                    let n = self.op(Op::Binary(op), all);
                    return lanes(vec![n; width]);
                }
                let l = self.broadcast(left, width, state)?;
                let r = self.broadcast(right, width, state)?;
                lanes(
                    l.into_iter()
                        .zip(r)
                        .map(|(a, b)| self.op(Op::Binary(op), vec![a, b]))
                        .collect(),
                )
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                let width = self.width_of(h)?;
                let c = self.broadcast(condition, width, state)?;
                let a = self.broadcast(accept, width, state)?;
                let r = self.broadcast(reject, width, state)?;
                lanes(
                    (0..width)
                        .map(|k| self.op(Op::Select, vec![c[k], a[k], r[k]]))
                        .collect(),
                )
            }
            Expression::Derivative { axis, ctrl, expr } => {
                let v = self.lanes(expr, state)?;
                lanes(
                    v.into_iter()
                        .map(|x| self.op(Op::Derivative(axis, ctrl), vec![x]))
                        .collect(),
                )
            }
            Expression::Relational { fun, argument } => {
                let v = self.lanes(argument, state)?;
                match fun {
                    RelationalFunction::All | RelationalFunction::Any => {
                        lanes(vec![self.op(Op::Relational(fun), v)])
                    }
                    _ => lanes(
                        v.into_iter()
                            .map(|x| self.op(Op::Relational(fun), vec![x]))
                            .collect(),
                    ),
                }
            }
            Expression::Math {
                fun,
                arg,
                arg1,
                arg2,
                arg3,
            } => {
                let width = self.width_of(h)?;
                let args: Vec<_> = [Some(arg), arg1, arg2, arg3].into_iter().flatten().collect();
                if componentwise(fun) {
                    let mut per = Vec::new();
                    for a in &args {
                        per.push(self.broadcast(*a, width, state)?);
                    }
                    let mut out = Vec::new();
                    for k in 0..width {
                        let operands = per.iter().map(|p| p.get(k).copied().unwrap_or(p[0])).collect();
                        out.push(self.op(Op::Math(fun), operands));
                    }
                    lanes(out)
                } else {
                    // a reduction or a function that mixes lanes: each lane of the result from every lane of each
                    // argument
                    let mut all = Vec::new();
                    for a in &args {
                        all.extend(self.lanes(*a, state)?);
                    }
                    let n = self.op(Op::Math(fun), all);
                    lanes(vec![n; width.max(1)])
                }
            }
            Expression::As { expr, kind, convert } => {
                let v = self.lanes(expr, state)?;
                match convert {
                    None => lanes(v),
                    Some(_) => lanes(
                        v.into_iter()
                            .map(|x| self.op(Op::Convert(kind), vec![x]))
                            .collect(),
                    ),
                }
            }
            Expression::CallResult(f) => Err(format!(
                "a call to {} is not followed",
                self.module.functions[f].name.as_deref().unwrap_or("a function")
            )),
            Expression::AtomicResult { .. }
            | Expression::WorkGroupUniformLoadResult { .. }
            | Expression::SubgroupBallotResult
            | Expression::SubgroupOperationResult { .. } => Err(format!(
                "expression {} is a statement's result the walk has not reached",
                h.index()
            )),
            other => Err(format!("{} is not followed", expression_kind(&other))),
        }
    }

    /// The lanes of a texel read: one node per lane of the result.
    fn texel(
        &mut self,
        resource: Option<ResourceId>,
        read: Read,
        coordinate: Vec<NodeId>,
        controls: Vec<NodeId>,
        width: usize,
    ) -> Vec<NodeId> {
        (0..width)
            .map(|lane| {
                self.intern(Node::Texel {
                    resource,
                    lane: lane as u8,
                    read,
                    coordinate: coordinate.clone(),
                    controls: controls.clone(),
                })
            })
            .collect()
    }

    /// The resource an image expression names, and the lanes of an index into a binding array of them.
    fn image(
        &mut self,
        image: Handle<Expression>,
        state: &State,
    ) -> Result<(Option<ResourceId>, Vec<NodeId>), String> {
        match self.function.expressions[image].clone() {
            Expression::GlobalVariable(g) => Ok((self.resources.get(&g).copied(), Vec::new())),
            Expression::Access { base, index } => {
                let (r, mut controls) = self.image(base, state)?;
                controls.extend(self.lanes(index, state)?);
                Ok((r, controls))
            }
            Expression::AccessIndex { base, .. } => self.image(base, state),
            other => Err(format!("an image that is {}", expression_kind(&other))),
        }
    }

    /// Where a pointer expression points.
    fn place(&mut self, pointer: Handle<Expression>, state: &State) -> Result<Place, String> {
        match self.function.expressions[pointer].clone() {
            Expression::LocalVariable(l) => {
                let w = self.width(&self.module.types[self.function.local_variables[l].ty].inner);
                Ok(Place {
                    root: Root::Local(l),
                    starts: vec![0],
                    width: w,
                    index: Vec::new(),
                    runtime_sized: false,
                })
            }
            Expression::GlobalVariable(g) => {
                let w = self.width(&self.module.types[self.module.global_variables[g].ty].inner);
                Ok(Place {
                    root: Root::Global(g),
                    starts: vec![0],
                    width: w,
                    index: Vec::new(),
                    runtime_sized: false,
                })
            }
            Expression::AccessIndex { base, index } => {
                let ty = self.ty(base)?;
                let mut place = self.place(base, state)?;
                let (range, _) = self.member(&ty, index)?;
                place.starts = place.starts.iter().map(|s| s + range.start).collect();
                place.width = range.len();
                Ok(place)
            }
            Expression::Access { base, index } => {
                let ty = self.ty(base)?;
                let mut place = self.place(base, state)?;
                if let Some(i) = self.literal_index(index) {
                    let (range, _) = self.member(&ty, i)?;
                    place.starts = place.starts.iter().map(|s| s + range.start).collect();
                    place.width = range.len();
                    return Ok(place);
                }
                let (count, element) = self.elements(&ty)?;
                let w = self.width(&element);
                place.index.extend(self.lanes(index, state)?);
                match count {
                    Some(n) => {
                        place.starts = place
                            .starts
                            .iter()
                            .flat_map(|s| (0..n).map(move |e| s + e * w))
                            .collect();
                    }
                    None => place.runtime_sized = true,
                }
                place.width = w;
                Ok(place)
            }
            other => Err(format!("a pointer that is {}", expression_kind(&other))),
        }
    }

    /// Whether a global is memory the program itself writes and reads (private or workgroup).
    fn followed(&self, g: Handle<naga::GlobalVariable>) -> bool {
        matches!(
            self.module.global_variables[g].space,
            naga::AddressSpace::Private | naga::AddressSpace::WorkGroup
        )
    }

    /// A global's lanes before the program writes it: its initializer's, else the global's own components.
    fn global_lanes(&mut self, g: Handle<naga::GlobalVariable>) -> Result<Vec<NodeId>, String> {
        let var = &self.module.global_variables[g];
        if let Some(init) = var.init {
            return self.global_expression(init);
        }
        let w = self.width(&self.module.types[var.ty].inner);
        let resource = self.resources.get(&g).copied();
        Ok((0..w)
            .map(|k| {
                self.intern(Node::Global {
                    resource,
                    at: At::Component(k as u32),
                })
            })
            .collect())
    }

    fn load(&mut self, pointer: Handle<Expression>, state: &State) -> Result<Vec<NodeId>, String> {
        let place = self.place(pointer, state)?;
        let memory: Option<Vec<NodeId>> = match place.root {
            Root::Local(l) => Some(
                state
                    .locals
                    .get(&l)
                    .cloned()
                    .ok_or("a local the walk does not hold")?,
            ),
            Root::Global(g) if self.followed(g) => Some(match state.globals.get(&g) {
                Some(lanes) => lanes.clone(),
                None => self.global_lanes(g)?,
            }),
            Root::Global(_) => None,
        };
        let Some(memory) = memory else {
            // a bound resource's (or constant data's) own components
            let Root::Global(g) = place.root else {
                unreachable!("locals are memory")
            };
            if self.module.global_variables[g].init.is_some() && place.index.is_empty() {
                let all = self.global_lanes(g)?;
                return Ok(all[place.starts[0]..place.starts[0] + place.width].to_vec());
            }
            let resource = self.resources.get(&g).copied();
            if place.starts.len() == 1 && !place.runtime_sized && place.index.is_empty() {
                return Ok((0..place.width)
                    .map(|k| {
                        self.intern(Node::Global {
                            resource,
                            at: At::Component((place.starts[0] + k) as u32),
                        })
                    })
                    .collect());
            }
            return Ok((0..place.width)
                .map(|k| {
                    self.intern(Node::Global {
                        resource,
                        at: At::Indexed {
                            lane: k as u32,
                            index: place.index.clone(),
                        },
                    })
                })
                .collect());
        };
        if place.runtime_sized {
            return Err("a run-time sized array in memory the program writes".into());
        }
        let mut out = Vec::new();
        for k in 0..place.width {
            let candidates: Vec<NodeId> = place
                .starts
                .iter()
                .map(|s| memory.get(s + k).copied().ok_or("a load past its variable"))
                .collect::<Result<_, _>>()?;
            out.push(if candidates.len() == 1 {
                candidates[0]
            } else {
                let mut operands = candidates;
                operands.extend(&place.index);
                self.op(Op::Index, operands)
            });
        }
        Ok(out)
    }

    fn store(
        &mut self,
        pointer: Handle<Expression>,
        value: Handle<Expression>,
        state: &mut State,
    ) -> Result<(), String> {
        let place = self.place(pointer, state)?;
        let value = self.lanes(value, state)?;
        let mut memory = match place.root {
            Root::Local(l) => state
                .locals
                .get(&l)
                .cloned()
                .ok_or("a local the walk does not hold")?,
            Root::Global(g) if self.followed(g) => match state.globals.get(&g) {
                Some(lanes) => lanes.clone(),
                None => self.global_lanes(g)?,
            },
            // a write to a storage buffer or image: not followed into a later read of it (module documentation)
            Root::Global(_) => return Ok(()),
        };
        if place.runtime_sized {
            return Err("a run-time sized array in memory the program writes".into());
        }
        let exact = place.starts.len() == 1;
        for (k, &lane) in value.iter().enumerate().take(place.width) {
            for s in &place.starts {
                let at = s + k;
                let old = *memory.get(at).ok_or("a store past its variable")?;
                memory[at] = if exact {
                    lane
                } else {
                    // a store through a run-time index may leave this element as it was
                    let mut operands = vec![old, lane];
                    operands.extend(&place.index);
                    self.op(Op::Index, operands)
                };
            }
        }
        match place.root {
            Root::Local(l) => {
                state.locals.insert(l, memory);
            }
            Root::Global(g) => {
                state.globals.insert(g, memory);
            }
        }
        Ok(())
    }

    /// The lanes of a constant expression of the module (a constant's or a global's initializer).
    fn global_expression(&mut self, h: Handle<Expression>) -> Result<Vec<NodeId>, String> {
        match self.module.global_expressions[h].clone() {
            Expression::Literal(l) => {
                let v = literal_value(l);
                Ok(vec![self.literal(v)])
            }
            Expression::Constant(c) => self.global_expression(self.module.constants[c].init),
            Expression::ZeroValue(ty) => {
                let inner = self.module.types[ty].inner.clone();
                Ok(self.zero(&inner))
            }
            Expression::Compose { components, .. } => {
                let mut out = Vec::new();
                for c in components {
                    out.extend(self.global_expression(c)?);
                }
                Ok(out)
            }
            Expression::Splat { size, value } => {
                let v = self.global_expression(value)?;
                Ok(vec![v[0]; size as usize])
            }
            other => Err(format!("a constant that is {}", expression_kind(&other))),
        }
    }

    /// The state after `states` meet, `condition` deciding which: a lane they disagree on is a merge.
    fn join(&mut self, condition: &[NodeId], states: Vec<State>) -> Option<State> {
        let mut states = states.into_iter();
        let first = states.next()?;
        let rest: Vec<State> = states.collect();
        if rest.is_empty() {
            return Some(first);
        }
        let mut out = first.clone();
        for (l, lanes) in first.locals {
            let joined = (0..lanes.len())
                .map(|k| {
                    let arms: Vec<NodeId> = std::iter::once(lanes[k])
                        .chain(
                            rest.iter()
                                .filter_map(|s| s.locals.get(&l).and_then(|v| v.get(k).copied())),
                        )
                        .collect();
                    self.merge(condition.to_vec(), arms)
                })
                .collect();
            out.locals.insert(l, joined);
        }
        let globals: BTreeSet<_> = std::iter::once(&first.globals)
            .chain(rest.iter().map(|s| &s.globals))
            .flat_map(|g| g.keys().copied())
            .collect();
        for g in globals {
            let Ok(initial) = self.global_lanes(g) else {
                continue;
            };
            let all: Vec<Vec<NodeId>> = std::iter::once(&first.globals)
                .chain(rest.iter().map(|s| &s.globals))
                .map(|m| m.get(&g).cloned().unwrap_or_else(|| initial.clone()))
                .collect();
            let joined = (0..initial.len())
                .map(|k| {
                    let arms = all.iter().filter_map(|v| v.get(k).copied()).collect();
                    self.merge(condition.to_vec(), arms)
                })
                .collect();
            out.globals.insert(g, joined);
        }
        Some(out)
    }

    /// Run `block` from `state`: the state it ends in, `None` where every path through it leaves (a return, a discard,
    /// a break or continue).
    fn block(&mut self, block: &naga::Block, state: State) -> Result<Option<State>, String> {
        let mut state = state;
        for statement in block.iter() {
            match statement {
                Statement::Emit(range) => {
                    for h in range.clone() {
                        let v = self.evaluate(h, &state)?;
                        self.cache[h.index()] = Some(v);
                    }
                }
                Statement::Block(inner) => match self.block(inner, state)? {
                    Some(s) => state = s,
                    None => return Ok(None),
                },
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    let c = self.lanes(*condition, &state)?;
                    self.control.extend(&c);
                    let a = self.block(accept, state.clone())?;
                    let r = self.block(reject, state)?;
                    self.control.truncate(self.control.len() - c.len());
                    match self.join(&c, a.into_iter().chain(r).collect()) {
                        Some(s) => state = s,
                        None => return Ok(None),
                    }
                }
                Statement::Switch { selector, cases } => {
                    let c = self.lanes(*selector, &state)?;
                    self.control.extend(&c);
                    self.targets.push(Target::Switch(Vec::new()));
                    let mut ends = Vec::new();
                    // a case that falls through runs on into the next case's body
                    let mut carried: Option<State> = None;
                    for case in cases {
                        let start = carried.take().unwrap_or_else(|| state.clone());
                        let end = self.block(&case.body, start)?;
                        if case.fall_through {
                            carried = end;
                        } else if let Some(end) = end {
                            ends.push(end);
                        }
                    }
                    ends.extend(carried);
                    let Some(Target::Switch(breaks)) = self.targets.pop() else {
                        unreachable!("the switch's own target")
                    };
                    ends.extend(breaks);
                    self.control.truncate(self.control.len() - c.len());
                    match self.join(&c, ends) {
                        Some(s) => state = s,
                        None => return Ok(None),
                    }
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => match self.run_loop(body, continuing, *break_if, state)? {
                    Some(s) => state = s,
                    None => return Ok(None),
                },
                Statement::Break => {
                    let conditions = self.control.clone();
                    match self.targets.last_mut() {
                        Some(Target::Loop(exits)) => exits.breaks.push((state, conditions)),
                        Some(Target::Switch(breaks)) => breaks.push(state),
                        None => return Err("a break outside a loop or switch".into()),
                    }
                    return Ok(None);
                }
                Statement::Continue => {
                    let target = self
                        .targets
                        .iter_mut()
                        .rev()
                        .find_map(|t| match t {
                            Target::Loop(exits) => Some(exits),
                            Target::Switch(_) => None,
                        })
                        .ok_or("a continue outside a loop")?;
                    target.continues.push(state);
                    return Ok(None);
                }
                Statement::Return { value } => {
                    let lanes = match value {
                        Some(v) => self.lanes(*v, &state)?,
                        None => Vec::new(),
                    };
                    self.returns.push((self.control.clone(), lanes));
                    return Ok(None);
                }
                Statement::Kill => {
                    self.discards.push(self.control.clone());
                    return Ok(None);
                }
                Statement::ControlBarrier(_) | Statement::MemoryBarrier(_) => {}
                Statement::Store { pointer, value } => self.store(*pointer, *value, &mut state)?,
                // a write to an image: not followed into a later read of it (module documentation)
                Statement::ImageStore { .. } | Statement::ImageAtomic { .. } => {}
                Statement::Atomic { pointer, result, .. } => {
                    if let Some(result) = result {
                        let lanes = self.load(*pointer, &state)?;
                        self.cache[result.index()] = Some(Val::Lanes(lanes));
                    }
                }
                Statement::WorkGroupUniformLoad { pointer, result } => {
                    let lanes = self.load(*pointer, &state)?;
                    self.cache[result.index()] = Some(Val::Lanes(lanes));
                }
                Statement::SubgroupBallot { result, predicate } => {
                    let operands = match predicate {
                        Some(p) => self.lanes(*p, &state)?,
                        None => Vec::new(),
                    };
                    let n = self.op(Op::Subgroup, operands);
                    self.cache[result.index()] = Some(Val::Lanes(vec![n; 4]));
                }
                Statement::SubgroupCollectiveOperation { argument, result, .. }
                | Statement::SubgroupGather { argument, result, .. } => {
                    let v = self.lanes(*argument, &state)?;
                    let out = v.into_iter().map(|x| self.op(Op::Subgroup, vec![x])).collect();
                    self.cache[result.index()] = Some(Val::Lanes(out));
                }
                Statement::Call { function, .. } => {
                    return Err(format!(
                        "a call to {} is not followed",
                        self.module.functions[*function]
                            .name
                            .as_deref()
                            .unwrap_or("a function")
                    ));
                }
                Statement::RayQuery { .. } => return Err("a ray query is not followed".into()),
                Statement::RayPipelineFunction(_) => {
                    return Err("a ray pipeline function is not followed".into());
                }
                Statement::CooperativeStore { .. } => {
                    return Err("a cooperative matrix store is not followed".into());
                }
            }
        }
        Ok(Some(state))
    }

    /// A loop to a fixed point: the lanes an iteration changes are loop values ([`Node::Loop`]), found by running the
    /// body until no further lane changes (the set only grows and is bounded by the memory's lanes).
    fn run_loop(
        &mut self,
        body: &naga::Block,
        continuing: &naga::Block,
        break_if: Option<Handle<Expression>>,
        entry: State,
    ) -> Result<Option<State>, String> {
        let base = self.control.len();
        let mut carried: BTreeSet<(Root, usize)> = BTreeSet::new();
        loop {
            // the loop values of this round, each a fresh node whose arms are filled after the round
            let mut phis: BTreeMap<(Root, usize), NodeId> = BTreeMap::new();
            let mut start = entry.clone();
            for &(root, k) in &carried {
                let id = NodeId(self.nodes.len() as u32);
                self.nodes.push(Node::Loop {
                    arms: Vec::new(),
                    exit: Vec::new(),
                });
                phis.insert((root, k), id);
                set_lane(&mut start, root, k, id);
            }
            self.targets.push(Target::Loop(LoopExits::default()));
            let end = self.block(body, start.clone())?;
            let Some(Target::Loop(mut exits)) = self.targets.pop() else {
                unreachable!("the loop's own target")
            };
            exits.continues.extend(end);
            let mut back = Vec::new();
            let mut exit_conditions: Vec<NodeId> = exits
                .breaks
                .iter()
                .flat_map(|(_, c)| c.get(base..).unwrap_or_default().to_vec())
                .collect();
            let mut leaving: Vec<State> = exits.breaks.into_iter().map(|(s, _)| s).collect();
            for state in exits.continues {
                if let Some(after) = self.block(continuing, state)? {
                    if let Some(b) = break_if {
                        let c = self.lanes(b, &after)?;
                        exit_conditions.extend(&c);
                        leaving.push(after.clone());
                    }
                    back.push(after);
                }
            }
            // the lanes an iteration leaves different from how it found them
            let mut changed = carried.clone();
            for b in &back {
                for (l, lanes) in &b.locals {
                    for (k, v) in lanes.iter().enumerate() {
                        if start.locals.get(l).and_then(|s| s.get(k)) != Some(v) {
                            changed.insert((Root::Local(*l), k));
                        }
                    }
                }
                for (g, lanes) in &b.globals {
                    for (k, v) in lanes.iter().enumerate() {
                        if start.globals.get(g).and_then(|s| s.get(k)) != Some(v) {
                            changed.insert((Root::Global(*g), k));
                        }
                    }
                }
            }
            if changed != carried {
                carried = changed;
                continue;
            }
            for (&(root, k), &id) in &phis {
                let mut arms = vec![lane_of(&entry, root, k).unwrap_or(id)];
                arms.extend(back.iter().filter_map(|b| lane_of(b, root, k)));
                let mut seen = BTreeSet::new();
                arms.retain(|a| seen.insert(*a));
                self.nodes[id.0 as usize] = Node::Loop {
                    arms,
                    exit: exit_conditions.clone(),
                };
            }
            return Ok(self.join(&exit_conditions, leaving));
        }
    }
}

fn set_lane(state: &mut State, root: Root, k: usize, id: NodeId) {
    let lanes = match root {
        Root::Local(l) => state.locals.get_mut(&l),
        Root::Global(g) => state.globals.get_mut(&g),
    };
    if let Some(slot) = lanes.and_then(|l| l.get_mut(k)) {
        *slot = id;
    }
}

fn lane_of(state: &State, root: Root, k: usize) -> Option<NodeId> {
    match root {
        Root::Local(l) => state.locals.get(&l)?.get(k).copied(),
        Root::Global(g) => state.globals.get(&g)?.get(k).copied(),
    }
}

/// Lane `k` of an argument or result member: its varying and lane. An array of control points is the same element's
/// lanes point after point; the clip distances are several varyings back to back. `None` for an output lane past its
/// varying's width (a colour target carried with four lanes where the element has fewer); an input lane past it is
/// still that input's lane (a system value carried with its API's width).
fn varying_lane(
    list: &[crate::Varying],
    item: &crate::IoItem,
    k: usize,
    output: bool,
) -> Result<Option<(usize, u8)>, String> {
    let width = |v: usize| -> Result<usize, String> {
        Ok(list.get(v).ok_or("a varying past the interface")?.value.width as usize)
    };
    match (&item.ty, item.varyings.as_slice()) {
        (crate::IoType::Array(..), [v]) => Ok(Some((*v, (k % width(*v)?.max(1)) as u8))),
        (_, [v]) => Ok((!output || k < width(*v)?).then_some((*v, k as u8))),
        (_, varyings) => {
            let mut at = k;
            for &v in varyings {
                let w = width(v)?;
                if at < w {
                    return Ok(Some((v, at as u8)));
                }
                at -= w;
            }
            Err(format!("lane {k} past the item's varyings"))
        }
    }
}

fn is_pointer(ty: &TypeInner) -> bool {
    matches!(ty, TypeInner::Pointer { .. } | TypeInner::ValuePointer { .. })
}

fn literal_value(l: naga::Literal) -> f64 {
    match l {
        naga::Literal::F64(v) => v,
        naga::Literal::F32(v) => f64::from(v),
        naga::Literal::F16(v) => f64::from(f32::from(v)),
        naga::Literal::U32(v) => f64::from(v),
        naga::Literal::I32(v) => f64::from(v),
        naga::Literal::U16(v) => f64::from(v),
        naga::Literal::I16(v) => f64::from(v),
        naga::Literal::U64(v) => v as f64,
        naga::Literal::I64(v) => v as f64,
        naga::Literal::Bool(v) => f64::from(u8::from(v)),
        naga::Literal::AbstractInt(v) => v as f64,
        naga::Literal::AbstractFloat(v) => v,
    }
}

/// Whether lane `k` of `fun`'s result is made of lane `k` of each argument only.
fn componentwise(fun: MathFunction) -> bool {
    use MathFunction as M;
    matches!(
        fun,
        M::Abs
            | M::Min
            | M::Max
            | M::Clamp
            | M::Saturate
            | M::Cos
            | M::Cosh
            | M::Sin
            | M::Sinh
            | M::Tan
            | M::Tanh
            | M::Acos
            | M::Asin
            | M::Atan
            | M::Atan2
            | M::Asinh
            | M::Acosh
            | M::Atanh
            | M::Radians
            | M::Degrees
            | M::Ceil
            | M::Floor
            | M::Round
            | M::Fract
            | M::Trunc
            | M::Ldexp
            | M::Exp
            | M::Exp2
            | M::Log
            | M::Log2
            | M::Pow
            | M::Sign
            | M::Fma
            | M::Mix
            | M::Step
            | M::SmoothStep
            | M::Sqrt
            | M::InverseSqrt
            | M::QuantizeToF16
            | M::CountTrailingZeros
            | M::CountLeadingZeros
            | M::CountOneBits
            | M::ReverseBits
            | M::ExtractBits
            | M::InsertBits
            | M::FirstTrailingBit
            | M::FirstLeadingBit
    )
}

/// An expression's variant, to name it in an error.
fn expression_kind(e: &Expression) -> String {
    let debug = format!("{e:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("an expression")
        .to_owned()
}
