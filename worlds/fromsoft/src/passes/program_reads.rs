//! What a material's compiled programs read, from the programs themselves: which material texture slots decide whether
//! a pixel is drawn at all (the alpha test), which colour the G-buffer's albedo, and where each is sampled (which vertex
//! UV set, which pair of it). A preview chooses its textures from this, not from slot numbers or shader names.
//!
//! The programs are read lifted ([`lift`]: DXIL and DXBC alike), as the value graph of their entry point
//! ([`iridia::flow::Values`]): each texel names a texture resource, which is a material slot of the shader's
//! metaparam where the metaparam names it ([`slot_of`]); its coordinates are made of an interpolant (`TEXCOORD4.xy`)
//! scaled and offset by the material's constants; each discard is guarded by the conditions that decide it; render
//! target 2 is the albedo (the layout of the G-buffer the programs write: target 0 the normal, which reads the normal
//! maps and the normal and tangent interpolants, target 1 the metallic and gloss, which reads the metallic map, target 2
//! the albedo, which reads the albedo maps). The vertex program then says which vertex input each interpolant is made
//! of.
//!
//! The albedo's base ([`Reads::base`]) is the slot the program blends the others over: Elden Ring's `C[DetailBlend]`
//! (`_6_Gbuf.ppo`) writes `saturate(detail * 4.55 * lerp(t7, luma(t7), p))`, `detail` a lerp of three detail albedos
//! (`Texture2D_1`, `_10`, `_4`, the shared `aat100_*` metal, leather or fur and mud maps, each at its own tiling) by masks
//! made of the normal map's alpha (`Texture2D_0`), and `t7` the character's own albedo (`Texture2D_7`): a capture of
//! the running game binds `c3251_K_*_a` there and the `aat100_*` maps to the detail slots. Below that product the
//! program blends a decal over it and the result toward a constant colour by masks made of the normal and metallic
//! maps, then multiplies the material's colour and the instance's. Read as dataflow: down the values every path to an
//! albedo slot passes through (past the clamps, the engine's terms and those blends), the first that is one slot, a
//! product with one factor that is one albedo slot, or a blend of two layers: an overlay drawn by its own alpha (`[S2]`'s
//! second layer, `Texture2D_8`) has the base of what it is drawn over. Where a product has several such factors (`_0`'s
//! `Texture2D_1 * 4.55 * lerp(Texture2D_7, luma, p)`, `C[Fur]`'s strand card times its colour), or a blend's layers by
//! another slot's mask both have one (`_14` blends `Texture2D_10` over its `Texture2D_1 * Texture2D_7` layer by the
//! normal map's alpha), the base is the one in the shader's main UV group, the one holding an albedo, a normal and a
//! metallic map (the metaparam's groups: `Texture2D_7` with `_0` and `_3`; `C[Fur]`'s colour `Texture2D_8` with `_0`
//! and `_3`). A two-sided cloth that colours its back face from another layer (`_18`, by `SV_IsFrontFace`) has its
//! front face's base.
//!
//! What it does not read is named in [`Reads::left_out`]: a discard against an engine texture (the screen-door dither of
//! a fade) is not a material slot's coverage.
use crate::metaparam::{Metaparam, Texture, texture_number};
use primase_core::shader::{Resource, ResourceId, ResourceKind, ShaderFormat};
use iridia::flow::{BinaryOperator, Node, NodeId, Op, Read, UnaryOperator, Values};
use iridia::{BoundSizes, Program};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The render target the programs write the albedo to.
const ALBEDO_TARGET: u32 = 2;
/// The metaparam's kinds of an albedo, a normal and a metallic slot ([`Texture::kind`]).
const ALBEDO_KIND: u8 = 1;
const NORMAL_KIND: u8 = 4;
const METALLIC_KIND: u8 = 14;
/// The pixel program's input that says the face drawn is a front face.
const FRONT_FACE: &str = "SV_IsFrontFace";

/// A vertex input component: `TEXCOORD0.z` is `("TEXCOORD", 0, 2)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VertexInput {
    pub name: String,
    pub index: u32,
    pub component: u8,
}

/// One material texture slot a program samples.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SlotRead {
    /// The slot's name in the shader's metaparam (the key of the material's binding of it).
    pub texture: String,
    /// The vertex input the slot's `u` coordinate is made of, where the program makes it of one.
    pub u: Option<VertexInput>,
    /// The vertex input its `v` coordinate is made of.
    pub v: Option<VertexInput>,
}

/// What one pixel program reads of the material.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// The slots whose alpha reaches an alpha test: the pixel is not drawn where it compares below the reference.
    pub coverage: Vec<SlotRead>,
    /// Whether a vertex colour's alpha reaches the alpha test too.
    pub coverage_vertex_alpha: bool,
    /// The slots whose colour reaches the albedo.
    pub colour: Vec<SlotRead>,
    /// The slot the albedo is made of with the other colour slots blended over it (the module's dataflow rule), where
    /// every lane of the albedo names the same one.
    pub base: Option<SlotRead>,
    /// What decides a discard besides the coverage slots, stated rather than followed.
    pub left_out: BTreeSet<&'static str>,
}

/// A program's container of `format` (a game's [`crate::Game::shader_format`]) lifted: DXBC or DXIL.
pub fn lift(format: ShaderFormat, bytes: &[u8]) -> Result<Program, String> {
    lift_with(format, bytes, &BoundSizes::new())
}

/// [`lift`] with the constant buffers `bound` names bound at the sizes it states (a stage's [`BoundSizes::of`]).
pub fn lift_with(format: ShaderFormat, bytes: &[u8], bound: &BoundSizes) -> Result<Program, String> {
    match format {
        ShaderFormat::Dxbc => primase_dxbc::lift_with(bytes, bound).map_err(|e| e.to_string()),
        ShaderFormat::Dxil => primase_dxil::lift_with(bytes, bound).map_err(|e| e.to_string()),
        other => Err(format!("a FromSoftware program of format {other:?}")),
    }
}

/// How a texture resource of a program was matched to the metaparam's slot ([`slot_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotMatch {
    /// The resource's name is the slot's name.
    Name,
    /// The resource's name and the slot's hold the same `Texture2D_N`.
    Number,
    /// The resource's register is the slot's index.
    Register,
}

/// The metaparam slot a texture resource of a program stands for, and how it was matched: the slot of the same name;
/// else the slot whose name holds the same `Texture2D_N` (the material's binding: Elden Ring's metaparam names
/// `C[DetailBlend]`'s slot 1 `C_DetailBlend__snp_Texture2D_7_AlbedoMap` and the program the texture at its register 1
/// `C_DetailBlend_snp_0_Texture2D_7`); else, for a resource the program names otherwise, the slot whose index is its
/// register. `None`: no slot of the metaparam (an engine texture).
pub fn slot_of(resource: &Resource, meta: &Metaparam) -> Option<(u32, SlotMatch)> {
    if !matches!(resource.kind, ResourceKind::Texture { .. }) {
        return None;
    }
    let name = &resource.provenance.name;
    if let Some(t) = meta.textures.iter().find(|t| &t.name == name) {
        return Some((u32::from(t.slot), SlotMatch::Name));
    }
    if let Some(n) = texture_number(name) {
        return meta
            .textures
            .iter()
            .find(|t| texture_number(&t.name) == Some(n))
            .map(|t| (u32::from(t.slot), SlotMatch::Number));
    }
    let register = resource.provenance.d3d.as_ref()?.register;
    meta.textures
        .iter()
        .any(|t| u32::from(t.slot) == register)
        .then_some((register, SlotMatch::Register))
}

/// An input component: its element's name and index, and the component.
type Interpolant = (String, u32, u8);

/// A texture leaf: the material slot it is (`None`: an engine texture) and the lane.
type TextureLane = (Option<u32>, u8);

/// A sample of a material slot and the inputs its coordinates are made of.
struct Sample {
    slot: u32,
    u: Option<Interpolant>,
    v: Option<Interpolant>,
}

/// The two layers of a blend `a + (b - a) * t` as sums of signed terms, and its weight ([`Analysis::lerp`]).
struct Layers {
    under: Vec<(NodeId, bool)>,
    over: Vec<(NodeId, bool)>,
    weight: NodeId,
}

/// One step of what a value is made of ([`Analysis::step`]).
enum Step {
    /// A component of a sampled texel: the texture and the lane.
    Texture(TextureLane),
    /// A vertex input component.
    Input(Interpolant),
    /// The values it is computed from.
    Of(Vec<NodeId>),
}

/// A value's leaves: the texture components it is made of and the input components it reads.
#[derive(Default)]
struct Leaves {
    textures: BTreeSet<TextureLane>,
    inputs: BTreeSet<Interpolant>,
}

/// A program's value graph, with its texture resources' slots.
struct Analysis<'a> {
    program: &'a Program,
    values: Values,
    slots: BTreeMap<ResourceId, u32>,
}

/// The values below one that reach an albedo slot's colour ([`Analysis::colour_dominators`]).
struct Reaching<'w, 'a> {
    analysis: &'w Analysis<'a>,
    /// The albedo slots.
    albedo: &'w BTreeSet<u32>,
    /// Whether each value visited reaches one.
    reach: HashMap<NodeId, bool>,
    /// The reaching values each is computed from.
    from: HashMap<NodeId, Vec<NodeId>>,
    /// The reaching values in postorder.
    order: Vec<NodeId>,
}

impl Reaching<'_, '_> {
    fn visit(&mut self, id: NodeId) -> bool {
        if let Some(&r) = self.reach.get(&id) {
            return r;
        }
        self.reach.insert(id, false);
        let r = match self.analysis.step(id) {
            // an albedo map's alpha is a mask (a coverage, an overlay's weight), not its colour
            Step::Texture((slot, lane)) => lane < 3 && slot.is_some_and(|s| self.albedo.contains(&s)),
            Step::Input(_) => false,
            Step::Of(values) => {
                let mut r = false;
                for x in values {
                    if self.visit(x) {
                        self.from.entry(id).or_default().push(x);
                        r = true;
                    }
                }
                r
            }
        };
        self.reach.insert(id, r);
        if r {
            self.order.push(id);
        }
        r
    }
}

impl<'a> Analysis<'a> {
    fn new(program: &'a Program, meta: Option<&Metaparam>) -> Result<Self, String> {
        let values = Values::of_program(program)?;
        let slots = match meta {
            Some(meta) => program
                .interface
                .resources
                .iter()
                .filter_map(|r| Some((r.id, slot_of(r, meta)?.0)))
                .collect(),
            None => BTreeMap::new(),
        };
        Ok(Analysis {
            program,
            values,
            slots,
        })
    }

    /// What a value is made of, one step down: a texture component (a sample's slot and lane), a vertex input
    /// component, or the values it is computed from. A constant's value is the material's or the engine's, and a value
    /// the pipeline supplies the pipeline's, not a texture's: each is made of none. A merge or a loop value is made of its arms (what decides among them is a
    /// condition, not a part of the value).
    fn step(&self, id: NodeId) -> Step {
        match self.values.node(id) {
            Node::Texel {
                resource,
                lane,
                read: Read::Sample,
                ..
            } => Step::Texture((resource.and_then(|r| self.slots.get(&r).copied()), *lane)),
            Node::Texel {
                coordinate, controls, ..
            } => Step::Of(coordinate.iter().chain(controls).copied().collect()),
            Node::Input { input, lane } => match self.program.interface.inputs.get(*input) {
                Some(v) => Step::Input((v.label.name.clone(), v.label.index, *lane)),
                None => Step::Of(Vec::new()),
            },
            Node::Global { .. } | Node::Supplied { .. } | Node::Query { .. } | Node::Literal(_) => {
                Step::Of(Vec::new())
            }
            Node::Op { operands, .. } => Step::Of(operands.clone()),
            Node::Merge { arms, .. } | Node::Loop { arms, .. } => Step::Of(arms.clone()),
        }
    }

    /// What a value is made of; `deciding` also follows what decides among a merge's arms and a loop's iterations
    /// (what decides a discard).
    fn leaves(&self, id: NodeId, deciding: bool, seen: &mut BTreeSet<NodeId>, out: &mut Leaves) {
        if !seen.insert(id) {
            return;
        }
        if deciding {
            match self.values.node(id) {
                Node::Merge { condition, .. } => {
                    for &c in condition {
                        self.leaves(c, deciding, seen, out);
                    }
                }
                Node::Loop { exit, .. } => {
                    for &c in exit {
                        self.leaves(c, deciding, seen, out);
                    }
                }
                _ => {}
            }
        }
        match self.step(id) {
            Step::Texture(t) => {
                out.textures.insert(t);
            }
            Step::Input(input) => {
                out.inputs.insert(input);
            }
            Step::Of(values) => {
                for x in values {
                    self.leaves(x, deciding, seen, out);
                }
            }
        }
    }

    fn leaves_of(&self, id: NodeId) -> Leaves {
        let mut out = Leaves::default();
        self.leaves(id, false, &mut BTreeSet::new(), &mut out);
        out
    }

    /// The material slots a value is made of.
    fn slots_in(&self, id: NodeId) -> BTreeSet<u32> {
        self.leaves_of(id)
            .textures
            .into_iter()
            .filter_map(|(slot, _)| slot)
            .collect()
    }

    /// The operation a value is: its operation and operands.
    fn op(&self, id: NodeId) -> Option<(Op, &[NodeId])> {
        match self.values.node(id) {
            Node::Op { op, operands } => Some((*op, operands)),
            _ => None,
        }
    }

    fn binary(&self, id: NodeId, which: BinaryOperator) -> Option<(NodeId, NodeId)> {
        match self.op(id)? {
            (Op::Binary(op), [l, r]) if op == which => Some((*l, *r)),
            _ => None,
        }
    }

    /// The slot a value of the albedo is made of with the other material slots blended over it (`slots`: the shader's
    /// slots by number): down the values every path from it to an albedo slot passes through
    /// ([`Self::colour_dominators`]: the clamps, conversions, the engine's terms and blends toward a constant colour by
    /// masks of other kinds of slot), the first that is made of one slot (that one), a product that names one
    /// ([`Self::product_base`]), a blend of two layers that does ([`Self::blend_base`]), or a choice by the face drawn
    /// (the base of its front face's colour, [`Self::front_face`]).
    fn base(&self, id: NodeId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        for d in self.colour_dominators(id, slots) {
            if let [one] = self.slots_in(d).into_iter().collect::<Vec<_>>()[..] {
                return Some(one);
            }
            if let Some(one) = self.product_base(d, slots) {
                return Some(one);
            }
            if let Some(one) = self.blend_base(d, slots) {
                return Some(one);
            }
            if let Some(front) = self.front_face(d) {
                return self.base(front, slots);
            }
        }
        None
    }

    /// The factors of a product: its operands, and theirs where they are products.
    fn factors(&self, id: NodeId, out: &mut Vec<NodeId>) {
        match self.binary(id, BinaryOperator::Multiply) {
            Some((l, r)) => {
                self.factors(l, out);
                self.factors(r, out);
            }
            None => out.push(id),
        }
    }

    /// The base of a product of factors made of material slots: the one factor made of one albedo slot that no other
    /// factor reads. Where several are (a detail map multiplied over the material's own albedo: `C[DetailBlend]_0`'s
    /// `Texture2D_1 * 4.55 * lerp(Texture2D_7, luma, p)`), the one in the shader's main texture set ([`main_set`]).
    fn product_base(&self, id: NodeId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        self.binary(id, BinaryOperator::Multiply)?;
        let mut factors = Vec::new();
        self.factors(id, &mut factors);
        let made_of: Vec<BTreeSet<u32>> = factors
            .into_iter()
            .map(|f| self.slots_in(f))
            .filter(|s| !s.is_empty())
            .collect();
        let candidates: Vec<u32> = made_of
            .iter()
            .filter_map(|s| match s.iter().copied().collect::<Vec<_>>()[..] {
                [one] if slots.get(&one).is_some_and(|t| t.kind == ALBEDO_KIND) => Some(one),
                _ => None,
            })
            .filter(|one| made_of.iter().filter(|s| s.contains(one)).count() == 1)
            .collect();
        match candidates[..] {
            [one] => Some(one),
            [_, _, ..] => match candidates
                .iter()
                .copied()
                .filter(|r| main_set(*r, slots))
                .collect::<Vec<_>>()[..]
            {
                [one] => Some(one),
                _ => None,
            },
            [] => None,
        }
    }

    /// A sum's terms with their signs (`true` added, `false` subtracted): the operands of its additions and
    /// subtractions and the operand of a negation, and theirs where they are sums. A value that is no sum is its one
    /// term.
    fn terms(&self, id: NodeId, positive: bool, out: &mut Vec<(NodeId, bool)>) {
        if let Some((l, r)) = self.binary(id, BinaryOperator::Add) {
            self.terms(l, positive, out);
            self.terms(r, positive, out);
        } else if let Some((l, r)) = self.binary(id, BinaryOperator::Subtract) {
            self.terms(l, positive, out);
            self.terms(r, !positive, out);
        } else if let Some((Op::Unary(UnaryOperator::Negate), [x])) = self.op(id) {
            self.terms(*x, !positive, out);
        } else {
            out.push((id, positive));
        }
    }

    /// The layers of a blend `a + (b - a) * t` written as a sum of terms, however the compiler arranged it (a blend of
    /// blends is folded: `C[DetailBlend][S2]_2` writes `((P - L) + (c * t8 - P) * t8.a) * w + L`): one term is a product
    /// one of whose factors, `D`, is a sum holding the negation of each of the other terms (`a`); `b` is the rest of `D`
    /// (`D + a`). Returns `a`'s terms, `b`'s and the weight `t`.
    fn lerp(&self, terms: &[(NodeId, bool)]) -> Option<Layers> {
        for (i, &(product, positive)) in terms.iter().enumerate() {
            let Some((lhs, rhs)) = self.binary(product, BinaryOperator::Multiply) else {
                continue;
            };
            let under: Vec<(NodeId, bool)> = terms
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, t)| *t)
                .collect();
            if under.is_empty() {
                continue;
            }
            for (difference, weight) in [(lhs, rhs), (rhs, lhs)] {
                let mut over = Vec::new();
                self.terms(difference, positive, &mut over);
                // each term of `a` cancels one negated term of `D`
                let mut matched = true;
                for &(term, sign) in &under {
                    match over.iter().position(|&(o, s)| o == term && s != sign) {
                        Some(k) => {
                            over.remove(k);
                        }
                        None => {
                            matched = false;
                            break;
                        }
                    }
                }
                if matched && !over.is_empty() {
                    return Some(Layers { under, over, weight });
                }
            }
        }
        None
    }

    /// The material slots a sum of terms is made of.
    fn slots_of(&self, terms: &[(NodeId, bool)]) -> BTreeSet<u32> {
        terms.iter().flat_map(|(t, _)| self.slots_in(*t)).collect()
    }

    /// The base of a sum of terms: of one added term, that term's ([`Self::base`]); of a blend, the blend's.
    fn terms_base(&self, terms: &[(NodeId, bool)], slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        match terms {
            [(one, true)] => self.base(*one, slots),
            _ => self.layers_base(terms, slots),
        }
    }

    /// The base of a value that is a blend of two layers ([`Self::lerp`]).
    fn blend_base(&self, id: NodeId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        let mut terms = Vec::new();
        self.terms(id, true, &mut terms);
        if terms.len() < 2 {
            return None;
        }
        self.layers_base(&terms, slots)
    }

    /// The base of a blend of two layers `a + (b - a) * t`: where the weight is made of `b`'s own slots and `b` of none
    /// of `a`'s, `b` is an overlay drawn by its own alpha (`C[DetailBlend][S2]`'s `Texture2D_8`) and the base is `a`'s;
    /// otherwise, of the two layers' bases, the one in the shader's main texture set ([`main_set`]: `C[DetailBlend]_14`
    /// blends `Texture2D_10`, a detail albedo, over its `Texture2D_1 * Texture2D_7` layer by the normal map's alpha).
    fn layers_base(&self, terms: &[(NodeId, bool)], slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        let Layers { under, over, weight } = self.lerp(terms)?;
        let (a, b, t) = (self.slots_of(&under), self.slots_of(&over), self.slots_in(weight));
        if !t.is_empty() && t.is_subset(&b) && b.is_disjoint(&a) {
            return self.terms_base(&under, slots);
        }
        let bases: BTreeSet<u32> = [self.terms_base(&under, slots), self.terms_base(&over, slots)]
            .into_iter()
            .flatten()
            .filter(|r| main_set(*r, slots))
            .collect();
        match bases.into_iter().collect::<Vec<_>>()[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    /// The front face's value of a choice by the face drawn: the accepted value of a select whose condition is made of
    /// the front-facing flag alone (`C[DetailBlend]_18`'s two-sided cloth colours its back face from `Texture2D_8`, its
    /// front from its `Texture2D_1 * Texture2D_7` layer).
    fn front_face(&self, id: NodeId) -> Option<NodeId> {
        let Some((Op::Select, [condition, accept, _])) = self.op(id) else {
            return None;
        };
        let leaves = self.leaves_of(*condition);
        (leaves.textures.is_empty()
            && !leaves.inputs.is_empty()
            && leaves.inputs.iter().all(|(name, _, _)| name == FRONT_FACE))
        .then_some(*accept)
    }

    /// The values every path from `id` down to an albedo slot's colour (a red, green or blue lane of its sample) passes
    /// through, `id` first, nearest first: the dominators of those lanes in the dataflow rooted at `id` (Cooper, Harvey
    /// and Kennedy's iteration over the values that reach one, in reverse postorder). Empty when `id` reaches none.
    fn colour_dominators(&self, id: NodeId, slots: &BTreeMap<u32, &Texture>) -> Vec<NodeId> {
        let albedo: BTreeSet<u32> = slots
            .iter()
            .filter(|(_, t)| t.kind == ALBEDO_KIND)
            .map(|(slot, _)| *slot)
            .collect();
        let mut walk = Reaching {
            analysis: self,
            albedo: &albedo,
            reach: HashMap::new(),
            from: HashMap::new(),
            order: Vec::new(),
        };
        if !walk.visit(id) {
            return Vec::new();
        }
        // reverse postorder from `id`, the albedo samples all leading to one sink past the last
        let Reaching { from, mut order, .. } = walk;
        order.reverse();
        let index: HashMap<NodeId, usize> = order.iter().enumerate().map(|(n, v)| (*v, n)).collect();
        let sink = order.len();
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); sink + 1];
        for (n, v) in order.iter().enumerate() {
            for x in from.get(v).into_iter().flatten() {
                preds[index[x]].push(n);
            }
            if matches!(self.step(*v), Step::Texture(..)) {
                preds[sink].push(n);
            }
        }
        let mut doms: Vec<Option<usize>> = vec![None; sink + 1];
        doms[0] = Some(0);
        let intersect = |doms: &[Option<usize>], mut a: usize, mut b: usize| {
            while a != b {
                while a > b {
                    a = doms[a].unwrap_or(0);
                }
                while b > a {
                    b = doms[b].unwrap_or(0);
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for n in 1..=sink {
                let new = preds[n]
                    .iter()
                    .filter(|p| doms[**p].is_some())
                    .fold(None, |q: Option<usize>, &p| {
                        Some(q.map_or(p, |q| intersect(&doms, p, q)))
                    });
                if new.is_some() && doms[n] != new {
                    doms[n] = new;
                    changed = true;
                }
            }
        }
        let mut chain = Vec::new();
        let mut at = doms[sink];
        while let Some(n) = at {
            chain.push(order[n]);
            at = if n == 0 { None } else { doms[n] };
        }
        chain.reverse();
        chain
    }

    /// The vertex input component a value is made of, when it is made of exactly one input.
    fn only_input(&self, id: NodeId) -> Option<Interpolant> {
        let leaves = self.leaves_of(id);
        let mut it = leaves.inputs.into_iter();
        let one = it.next()?;
        it.next().is_none().then_some(one)
    }

    /// Every node the outputs and the discards are made of, in the order the program computes them.
    fn reachable(&self) -> BTreeSet<NodeId> {
        let mut seen = BTreeSet::new();
        let mut work: Vec<NodeId> = self.values.outputs().values().copied().collect();
        work.extend(self.values.discards().iter().flatten());
        while let Some(id) = work.pop() {
            if seen.insert(id) {
                work.extend(self.values.operands(id));
            }
        }
        seen
    }

    /// Every sample of a material slot: its slot and the interpolant components its coordinates are made of, in the
    /// order the program computes them.
    fn samples(&self) -> Vec<Sample> {
        self.reachable()
            .into_iter()
            .filter_map(|id| match self.values.node(id) {
                Node::Texel {
                    resource: Some(r),
                    read: Read::Sample,
                    coordinate,
                    ..
                } => Some(Sample {
                    slot: *self.slots.get(r)?,
                    u: coordinate.first().and_then(|c| self.only_input(*c)),
                    v: coordinate.get(1).and_then(|c| self.only_input(*c)),
                }),
                _ => None,
            })
            .collect()
    }

    /// The output varying labelled `name`, `index`.
    fn output(&self, name: &str, index: u32) -> Option<usize> {
        self.program
            .interface
            .outputs
            .iter()
            .position(|v| v.label.index == index && v.label.name.eq_ignore_ascii_case(name))
    }
}

/// Whether a slot's UV group ([`Texture::uv_group`]) holds the shader's main texture set, an albedo with a normal and a
/// metallic map: a detail map's group holds a detail normal map at most.
fn main_set(slot: u32, slots: &BTreeMap<u32, &Texture>) -> bool {
    let Some(t) = slots.get(&slot) else {
        return false;
    };
    let holds = |kind| slots.values().any(|s| s.uv_group == t.uv_group && s.kind == kind);
    holds(NORMAL_KIND) && holds(METALLIC_KIND)
}

/// What the pixel program `pixel` and the vertex program `vertex` (lifted, [`lift`]) read of the material whose
/// shader's metaparam is `meta`.
pub fn read(pixel: &Program, vertex: &Program, meta: &Metaparam) -> Result<Reads, String> {
    let ps = Analysis::new(pixel, Some(meta))?;
    let vs = Analysis::new(vertex, None)?;
    let names: BTreeMap<u32, &str> = meta
        .textures
        .iter()
        .map(|t| (u32::from(t.slot), t.name.as_str()))
        .collect();
    let mut reads = Reads::default();

    // where each interpolant component comes from in the vertex program
    let mut origin: BTreeMap<Interpolant, Interpolant> = BTreeMap::new();
    for (&(output, lane), &value) in vs.values.outputs() {
        let label = &vertex.interface.outputs[output].label;
        if let Some(one) = vs.only_input(value) {
            origin.insert((label.name.clone(), label.index, lane), one);
        }
    }
    let vertex_input = |from: &Option<Interpolant>| {
        from.as_ref()
            .and_then(|k| origin.get(k))
            .map(|(name, index, component)| VertexInput {
                name: name.clone(),
                index: *index,
                component: *component,
            })
    };
    let samples = ps.samples();
    let read_of = |slot: u32| {
        let sample = samples.iter().find(|s| s.slot == slot)?;
        Some(SlotRead {
            texture: (*names.get(&slot)?).to_owned(),
            u: vertex_input(&sample.u),
            v: vertex_input(&sample.v),
        })
    };

    // the alpha test: what decides each discard
    let mut coverage = BTreeSet::new();
    for conditions in ps.values.discards() {
        for &condition in conditions {
            let mut leaves = Leaves::default();
            ps.leaves(condition, true, &mut BTreeSet::new(), &mut leaves);
            for &(slot, lane) in &leaves.textures {
                match slot {
                    Some(slot) if lane == 3 => {
                        coverage.insert(slot);
                    }
                    Some(_) => {}
                    None => {
                        reads
                            .left_out
                            .insert("a discard against an engine texture (a screen-door dither)");
                    }
                }
            }
            for interpolant in &leaves.inputs {
                if let Some(v) = origin.get(interpolant)
                    && v.0 == "COLOR"
                    && v.2 == 3
                {
                    reads.coverage_vertex_alpha = true;
                }
            }
        }
    }
    for slot in coverage {
        if let Some(read) = read_of(slot) {
            reads.coverage.push(read);
        }
    }

    // the albedo, and its base in each lane
    let textures: BTreeMap<u32, &Texture> = meta.textures.iter().map(|t| (u32::from(t.slot), t)).collect();
    let mut colour = BTreeSet::new();
    let mut bases = BTreeSet::new();
    if let Some(albedo) = ps.output("SV_Target", ALBEDO_TARGET) {
        for lane in 0..4u8 {
            let Some(value) = ps.values.output(albedo, lane) else {
                continue;
            };
            let lane_colour: BTreeSet<u32> = ps
                .leaves_of(value)
                .textures
                .into_iter()
                .filter_map(|(slot, _)| slot)
                .collect();
            // the colour's lanes (x, y, z) name the base; the alpha is other data (a lane of no material slot has none)
            if lane < 3 && !lane_colour.is_empty() {
                bases.insert(ps.base(value, &textures));
            }
            colour.extend(lane_colour);
        }
    }
    if let [Some(base)] = bases.into_iter().collect::<Vec<_>>()[..] {
        reads.base = read_of(base);
    }
    for slot in colour {
        if let Some(read) = read_of(slot) {
            reads.colour.push(read);
        }
    }
    Ok(reads)
}

/// The flat index of a vertex UV input's pair among the UV sets the geometry lowering keeps (`TexCoord(n)`): the
/// pairs of every vertex UV input of lower index come first, an input of `columns` components holding `columns / 2`
/// (rounded up) pairs, as the FLVER's UV members hold them (a four component member two sets, the others one).
pub fn uv_set(vertex_inputs: &[(u32, u8)], input: &VertexInput) -> Option<u8> {
    if input.name != "TEXCOORD" {
        return None;
    }
    let before: u32 = vertex_inputs
        .iter()
        .filter(|(index, _)| *index < input.index)
        .map(|(_, columns)| u32::from(*columns).div_ceil(2))
        .sum();
    let pair = u32::from(input.component / 2);
    u8::try_from(before + pair).ok()
}

/// The UV inputs (index, components) of a vertex program.
pub fn vertex_uv_inputs(vertex: &Program) -> Vec<(u32, u8)> {
    let mut out: Vec<(u32, u8)> = vertex
        .interface
        .inputs
        .iter()
        .filter(|v| v.label.name.eq_ignore_ascii_case("TEXCOORD"))
        .map(|v| (v.label.index, v.value.width))
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::{VertexInput, texture_number, uv_set};

    fn input(index: u32, component: u8) -> VertexInput {
        VertexInput {
            name: "TEXCOORD".into(),
            index,
            component,
        }
    }

    /// `C[Fur]`'s vertex program takes `TEXCOORD0` (four components: the sets of two UV pairs) and `TEXCOORD1` (two): the
    /// pairs are, in order, `TexCoord(0)`, `TexCoord(1)` and `TexCoord(2)`.
    #[test]
    fn the_uv_pairs_of_a_vertex_program_are_numbered_in_the_order_of_its_inputs() {
        let inputs = [(0, 4), (1, 2)];
        assert_eq!(uv_set(&inputs, &input(0, 0)), Some(0));
        assert_eq!(uv_set(&inputs, &input(0, 3)), Some(1));
        assert_eq!(uv_set(&inputs, &input(1, 1)), Some(2));
        assert_eq!(
            uv_set(
                &inputs,
                &VertexInput {
                    name: "COLOR".into(),
                    index: 0,
                    component: 0
                }
            ),
            None
        );
    }

    /// A material texture's number is the `N` of its `Texture2D_N`, whatever follows it.
    #[test]
    fn a_texture_resource_is_numbered_by_its_texture2d_n() {
        assert_eq!(
            texture_number("C_DetailBlend__snp_Texture2D_7_AlbedoMap_0"),
            Some(7)
        );
        assert_eq!(texture_number("Fur_NTC_snp_Texture2D_10_NormalMap"), Some(10));
        assert_eq!(texture_number("g_ShadowMap"), None);
    }
}
