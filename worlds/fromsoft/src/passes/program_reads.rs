//! What an Elden Ring material's compiled programs read, from the programs themselves (DXIL): which material texture
//! slots decide whether a pixel is drawn at all (the alpha test), which colour the G-buffer's albedo, and where each is
//! sampled (which vertex UV set, which pair of it). A preview chooses its textures from this, not from slot numbers or
//! shader names.
//!
//! The pixel program is read as a dataflow: each `dx.op.sample*` names a texture register (the metaparam's texture
//! slot number is the register) and its coordinates are made of an interpolant (`TEXCOORD4.xy`) scaled and offset by
//! the material's constants; each `discard` is guarded by the compares that decide it (a flag made of several, a
//! `phi` of constants, is followed to the branches that select its constants); each store to render target 2 is the
//! albedo (the layout of the G-buffer the programs write: target 0 the normal, which reads the normal maps and the
//! normal and tangent interpolants, target 1 the metallic and gloss, which reads the metallic map, target 2 the albedo,
//! which reads the albedo maps). The vertex program then says which vertex input each interpolant is made of.
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
use crate::metaparam::{Metaparam, Texture};
use primase_dxil::module::{BinaryOp, Constant, Function, Instruction, Module, Op, Value, ValueId};
use primase_dxil::reflection::{self, Element, Range};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The render target the programs write the albedo to.
const ALBEDO_TARGET: u32 = 2;
/// The metaparam's kinds of an albedo, a normal and a metallic slot ([`Texture::kind`]).
const ALBEDO_KIND: u8 = 1;
const NORMAL_KIND: u8 = 4;
const METALLIC_KIND: u8 = 14;
/// The pixel program's input that says the face drawn is a front face.
const FRONT_FACE: &str = "SV_IsFrontFace";
/// `dx.op` codes: sample, sample bias, sample level, sample grad.
const SAMPLES: [u64; 4] = [60, 61, 62, 63];
const LOAD_INPUT: u64 = 4;
const STORE_OUTPUT: u64 = 5;
const CBUFFER_LOAD: u64 = 59;
const DISCARD: u64 = 82;

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

/// A program's dataflow.
struct Analysis<'a> {
    module: &'a Module,
    function: &'a Function,
    defs: HashMap<ValueId, &'a Instruction>,
    ranges: Vec<Range>,
    inputs: BTreeMap<u32, Element>,
    outputs: BTreeMap<u32, Element>,
}

/// An input component: its element's name and index, and the component.
type Interpolant = (String, u32, u8);

/// A sample of a texture register and the inputs its coordinates are made of.
struct Sample {
    register: u32,
    u: Option<Interpolant>,
    v: Option<Interpolant>,
}

/// One step of what a value is made of ([`Analysis::step`]).
enum Step {
    /// A texture component: the sample's register and lane.
    Texture(u32, u8),
    /// A vertex input component.
    Input(Interpolant),
    /// The values it is computed from.
    Of(Vec<ValueId>),
}

/// The values below one that reach an albedo slot's colour ([`Analysis::colour_dominators`]).
struct Reaching<'w, 'a> {
    analysis: &'w Analysis<'a>,
    /// The albedo slots' registers.
    albedo: &'w BTreeSet<u32>,
    /// Whether each value visited reaches one.
    reach: HashMap<ValueId, bool>,
    /// The reaching values each is computed from.
    from: HashMap<ValueId, Vec<ValueId>>,
    /// The reaching values in postorder.
    order: Vec<ValueId>,
}

impl Reaching<'_, '_> {
    fn visit(&mut self, id: ValueId) -> bool {
        if let Some(&r) = self.reach.get(&id) {
            return r;
        }
        self.reach.insert(id, false);
        let r = match self.analysis.step(id) {
            // an albedo map's alpha is a mask (a coverage, an overlay's weight), not its colour
            Step::Texture(register, lane) => lane < 3 && self.albedo.contains(&register),
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

/// A value's leaves: the texture components it is made of (register, component) and the input components it reads
/// (element name and index, component).
#[derive(Default)]
struct Leaves {
    textures: BTreeSet<(u32, u8)>,
    inputs: BTreeSet<(String, u32, u8)>,
}

impl<'a> Analysis<'a> {
    fn new(module: &'a Module) -> Result<Self, String> {
        let entry = reflection::entry(module).map_err(|e| e.to_string())?;
        let function = module
            .functions
            .iter()
            .find(|f| f.value == entry)
            .ok_or("no entry function")?;
        let ranges = reflection::ranges(module)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(r, _, _)| r)
            .collect();
        let (inputs, outputs) = reflection::elements(module).map_err(|e| e.to_string())?;
        let mut defs = HashMap::new();
        for i in &function.instructions {
            if let Some((id, _)) = i.result {
                defs.insert(id, i);
            }
        }
        Ok(Analysis {
            module,
            function,
            defs,
            ranges,
            inputs: inputs.into_iter().map(|e| (e.id, e)).collect(),
            outputs: outputs.into_iter().map(|e| (e.id, e)).collect(),
        })
    }

    fn value(&self, id: ValueId) -> Option<&'a Value> {
        if id < self.function.first {
            self.module.values.get(id)
        } else {
            self.function.locals.get(id - self.function.first)
        }
    }

    fn int(&self, id: ValueId) -> Option<i64> {
        match self.value(id)? {
            Value::Constant(Constant::Integer(i)) => Some(*i),
            Value::Constant(Constant::Null) => Some(0),
            _ => None,
        }
    }

    /// The `dx.op` an operation calls: its code and arguments.
    fn call(&self, op: &'a Op) -> Option<(u64, &'a [ValueId])> {
        let Op::Call { function, args } = op else {
            return None;
        };
        if !self.module.name(*function)?.starts_with("dx.op.") {
            return None;
        }
        let code = u64::try_from(self.int(*args.first()?)?).ok()?;
        Some((code, args))
    }

    /// The register of the shader resource a handle value names.
    fn register(&self, handle: ValueId) -> Option<u32> {
        let (code, args) = self.call(&self.defs.get(&handle)?.op)?;
        match code {
            // createHandle: class, range id
            57 => {
                let class = u8::try_from(self.int(*args.get(1)?)?).ok()?;
                let id = u32::try_from(self.int(*args.get(2)?)?).ok()?;
                self.ranges
                    .iter()
                    .find(|r| r.class == class && r.id == id)
                    .map(|r| r.lower)
            }
            // createHandleFromBinding: the binding's lower bound
            217 => {
                let Value::Constant(Constant::Aggregate(a)) = self.value(*args.get(1)?)? else {
                    return None;
                };
                u32::try_from(self.int(*a.first()?)?).ok()
            }
            // annotateHandle
            216 => self.register(*args.get(1)?),
            _ => None,
        }
    }

    /// What a value is made of, one step down: a texture component (a sample's register and lane), a vertex input
    /// component, or the values it is computed from (a constant buffer load is made of none: its value is the
    /// material's or the engine's, not a texture's).
    fn step(&self, id: ValueId) -> Step {
        let Some(i) = self.defs.get(&id) else {
            return Step::Of(Vec::new());
        };
        match &i.op {
            Op::ExtractValue { aggregate, indices } => {
                if let Some(agg) = self.defs.get(aggregate)
                    && let Some((code, args)) = self.call(&agg.op)
                    && SAMPLES.contains(&code)
                    && let (Some(register), Some(&lane)) =
                        (args.get(1).and_then(|h| self.register(*h)), indices.first())
                {
                    return Step::Texture(register, lane as u8);
                }
                Step::Of(vec![*aggregate])
            }
            Op::Call { args, .. } => {
                let Some((code, a)) = self.call(&i.op) else {
                    return Step::Of(args.clone());
                };
                match code {
                    LOAD_INPUT => {
                        if let (Some(sig), Some(col)) = (
                            a.get(1).and_then(|v| self.int(*v)),
                            a.get(3).and_then(|v| self.int(*v)),
                        ) && let Some(e) = u32::try_from(sig).ok().and_then(|s| self.inputs.get(&s))
                        {
                            let component = (col - i64::from(e.column)).clamp(0, 255) as u8;
                            return Step::Input((e.name.clone(), e.index, component));
                        }
                        Step::Of(Vec::new())
                    }
                    CBUFFER_LOAD => Step::Of(Vec::new()),
                    _ => Step::Of(a.get(1..).unwrap_or_default().to_vec()),
                }
            }
            op => Step::Of(op.operands()),
        }
    }

    /// What a value is made of.
    fn leaves(&self, id: ValueId, seen: &mut BTreeSet<ValueId>, out: &mut Leaves) {
        if !seen.insert(id) {
            return;
        }
        match self.step(id) {
            Step::Texture(register, lane) => {
                out.textures.insert((register, lane));
            }
            Step::Input(input) => {
                out.inputs.insert(input);
            }
            Step::Of(values) => {
                for x in values {
                    self.leaves(x, seen, out);
                }
            }
        }
    }

    /// The material slots (registers of `slots`) a value is made of.
    fn slots_in<V>(&self, id: ValueId, slots: &BTreeMap<u32, V>) -> BTreeSet<u32> {
        self.leaves_of(id)
            .textures
            .into_iter()
            .map(|(register, _)| register)
            .filter(|register| slots.contains_key(register))
            .collect()
    }

    /// The operation a value is the result of.
    fn op(&self, id: ValueId) -> Option<&'a Op> {
        self.defs.get(&id).map(|i| &i.op)
    }

    /// The slot a value of the albedo is made of with the other material slots blended over it (`slots`: the shader's
    /// slots by register): down the values every path from it to an albedo slot passes through
    /// ([`Self::colour_dominators`]: the clamps, conversions, the engine's terms and blends toward a constant colour by
    /// masks of other kinds of slot), the first that is made of one slot (that one), a product that names one
    /// ([`Self::product_base`]), a blend of two layers that does ([`Self::blend_base`]), or a choice by the face drawn
    /// (the base of its front face's colour, [`Self::front_face`]).
    fn base(&self, id: ValueId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        for d in self.colour_dominators(id, slots) {
            if let [one] = self.slots_in(d, slots).into_iter().collect::<Vec<_>>()[..] {
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
    fn factors(&self, id: ValueId, out: &mut Vec<ValueId>) {
        match self.op(id) {
            Some(Op::Binary {
                op: BinaryOp::Mul,
                lhs,
                rhs,
            }) => {
                self.factors(*lhs, out);
                self.factors(*rhs, out);
            }
            _ => out.push(id),
        }
    }

    /// The base of a product of factors made of material slots: the one factor made of one albedo slot that no other
    /// factor reads. Where several are (a detail map multiplied over the material's own albedo: `C[DetailBlend]_0`'s
    /// `Texture2D_1 * 4.55 * lerp(Texture2D_7, luma, p)`), the one in the shader's main texture set ([`main_set`]).
    fn product_base(&self, id: ValueId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        if !matches!(
            self.op(id),
            Some(Op::Binary {
                op: BinaryOp::Mul,
                ..
            })
        ) {
            return None;
        }
        let mut factors = Vec::new();
        self.factors(id, &mut factors);
        let made_of: Vec<BTreeSet<u32>> = factors
            .into_iter()
            .map(|f| self.slots_in(f, slots))
            .filter(|s| !s.is_empty())
            .collect();
        let candidates: Vec<u32> = made_of
            .iter()
            .filter_map(|s| match s.iter().copied().collect::<Vec<_>>()[..] {
                [one] if slots[&one].kind == ALBEDO_KIND => Some(one),
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

    /// The layers of a blend `a + (b - a) * t`: `a`, `b` and the weight `t`.
    fn lerp(&self, id: ValueId) -> Option<(ValueId, ValueId, ValueId)> {
        let Some(Op::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        }) = self.op(id)
        else {
            return None;
        };
        for (a, blend) in [(*lhs, *rhs), (*rhs, *lhs)] {
            let Some(Op::Binary {
                op: BinaryOp::Mul,
                lhs: m0,
                rhs: m1,
            }) = self.op(blend)
            else {
                continue;
            };
            for (difference, t) in [(*m0, *m1), (*m1, *m0)] {
                if let Some(Op::Binary {
                    op: BinaryOp::Sub,
                    lhs: b,
                    rhs: from,
                }) = self.op(difference)
                    && *from == a
                {
                    return Some((a, *b, t));
                }
            }
        }
        None
    }

    /// The base of a blend of two layers `a + (b - a) * t`: where the weight is made of `b`'s own slots and `b` of none
    /// of `a`'s, `b` is an overlay drawn by its own alpha (`C[DetailBlend][S2]`'s `Texture2D_8`) and the base is `a`'s;
    /// otherwise, of the two layers' bases, the one in the shader's main texture set ([`main_set`]: `C[DetailBlend]_14`
    /// blends `Texture2D_10`, a detail albedo, over its `Texture2D_1 * Texture2D_7` layer by the normal map's alpha).
    fn blend_base(&self, id: ValueId, slots: &BTreeMap<u32, &Texture>) -> Option<u32> {
        let (a, b, t) = self.lerp(id)?;
        let (under, over, weight) = (
            self.slots_in(a, slots),
            self.slots_in(b, slots),
            self.slots_in(t, slots),
        );
        if !weight.is_empty() && weight.is_subset(&over) && over.is_disjoint(&under) {
            return self.base(a, slots);
        }
        let bases: BTreeSet<u32> = [self.base(a, slots), self.base(b, slots)]
            .into_iter()
            .flatten()
            .filter(|r| main_set(*r, slots))
            .collect();
        match bases.into_iter().collect::<Vec<_>>()[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    /// The front face's value of a choice by the face drawn: `then` of a `select` whose condition is made of the
    /// front-facing flag alone (`C[DetailBlend]_18`'s two-sided cloth colours its back face from `Texture2D_8`, its
    /// front from its `Texture2D_1 * Texture2D_7` layer).
    fn front_face(&self, id: ValueId) -> Option<ValueId> {
        let Some(Op::Select { condition, then, .. }) = self.op(id) else {
            return None;
        };
        let leaves = self.leaves_of(*condition);
        (leaves.textures.is_empty()
            && !leaves.inputs.is_empty()
            && leaves.inputs.iter().all(|(name, _, _)| name == FRONT_FACE))
        .then_some(*then)
    }

    /// The values every path from `id` down to an albedo slot's colour (a red, green or blue lane of its sample) passes
    /// through, `id` first, nearest first: the dominators of those lanes in the dataflow rooted at `id` (Cooper, Harvey
    /// and Kennedy's iteration over the values that reach one, in reverse postorder). Empty when `id` reaches none.
    fn colour_dominators(&self, id: ValueId, slots: &BTreeMap<u32, &Texture>) -> Vec<ValueId> {
        let albedo: BTreeSet<u32> = slots
            .iter()
            .filter(|(_, t)| t.kind == ALBEDO_KIND)
            .map(|(register, _)| *register)
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
        let index: HashMap<ValueId, usize> = order.iter().enumerate().map(|(n, v)| (*v, n)).collect();
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

    fn leaves_of(&self, id: ValueId) -> Leaves {
        let mut out = Leaves::default();
        self.leaves(id, &mut BTreeSet::new(), &mut out);
        out
    }

    /// The conditions that decide a discard in `block`: the compares of the branches into it; where one is a `phi` of
    /// constants (a flag several compares set), the compares of the branches that end the blocks the constants come from.
    fn discard_conditions(&self, block: usize) -> Vec<ValueId> {
        let terminator = |b: usize| {
            self.function.instructions.iter().find_map(|i| match &i.op {
                Op::CondBranch { condition, .. } if i.block == b => Some(*condition),
                _ => None,
            })
        };
        let mut out = Vec::new();
        for i in &self.function.instructions {
            let Op::CondBranch {
                condition,
                then,
                otherwise,
            } = &i.op
            else {
                continue;
            };
            if *then != block && *otherwise != block {
                continue;
            }
            match self.defs.get(condition).map(|d| &d.op) {
                Some(Op::Phi { incoming }) => {
                    out.extend(incoming.iter().filter_map(|(_, from)| terminator(*from)));
                }
                _ => out.push(*condition),
            }
        }
        out
    }

    /// The vertex input component a texture coordinate lane is made of, when it is made of exactly one input.
    fn only_input(&self, id: ValueId) -> Option<Interpolant> {
        let leaves = self.leaves_of(id);
        let mut it = leaves.inputs.into_iter();
        let one = it.next()?;
        it.next().is_none().then_some(one)
    }

    /// Every sample of a material slot: its register and the interpolant components its coordinates are made of.
    fn samples(&self) -> Vec<Sample> {
        self.function
            .instructions
            .iter()
            .filter_map(|i| {
                let (code, args) = self.call(&i.op)?;
                SAMPLES.contains(&code).then_some(())?;
                let register = self.register(*args.get(1)?)?;
                Some(Sample {
                    register,
                    u: self.only_input(*args.get(3)?),
                    v: self.only_input(*args.get(4)?),
                })
            })
            .collect()
    }
}

/// Whether a slot's UV group ([`Texture::uv_group`]) holds the shader's main texture set, an albedo with a normal and a
/// metallic map: a detail map's group holds a detail normal map at most.
fn main_set(register: u32, slots: &BTreeMap<u32, &Texture>) -> bool {
    let group = &slots[&register].uv_group;
    let holds = |kind| slots.values().any(|t| &t.uv_group == group && t.kind == kind);
    holds(NORMAL_KIND) && holds(METALLIC_KIND)
}

/// The slots of the metaparam by register.
fn slots(meta: &Metaparam) -> BTreeMap<u32, &str> {
    meta.textures
        .iter()
        .map(|t| (u32::from(t.slot), t.name.as_str()))
        .collect()
}

/// What the pixel program `pixel` and the vertex program `vertex` (DXIL containers) read of the material whose
/// shader's metaparam is `meta`.
pub fn read(pixel: &[u8], vertex: &[u8], meta: &Metaparam) -> Result<Reads, String> {
    let chunks = primase_dxcontainer::chunks(pixel).map_err(|e| e.to_string())?;
    let module = reflection::module(&chunks, b"DXIL")
        .map_err(|e| e.to_string())?
        .ok_or("no DXIL module")?;
    let ps = Analysis::new(&module)?;
    let slot = slots(meta);
    let vs_chunks = primase_dxcontainer::chunks(vertex).map_err(|e| e.to_string())?;
    let vs_module = reflection::module(&vs_chunks, b"DXIL")
        .map_err(|e| e.to_string())?
        .ok_or("no DXIL module")?;
    let vs = Analysis::new(&vs_module)?;
    let mut reads = Reads::default();

    // where each interpolant component comes from in the vertex program
    let mut origin: BTreeMap<Interpolant, Interpolant> = BTreeMap::new();
    for i in &vs.function.instructions {
        let Some((code, args)) = vs.call(&i.op) else {
            continue;
        };
        if code != STORE_OUTPUT {
            continue;
        }
        let (Some(sig), Some(col), Some(&value)) = (
            args.get(1).and_then(|v| vs.int(*v)),
            args.get(3).and_then(|v| vs.int(*v)),
            args.get(4),
        ) else {
            continue;
        };
        let Some(e) = u32::try_from(sig).ok().and_then(|s| vs.outputs.get(&s)) else {
            continue;
        };
        if let Some(one) = vs.only_input(value) {
            origin.insert(
                (
                    e.name.clone(),
                    e.index,
                    (col - i64::from(e.column)).clamp(0, 255) as u8,
                ),
                one,
            );
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
    let read_of = |register: u32| {
        let sample = samples.iter().find(|s| s.register == register)?;
        Some(SlotRead {
            texture: (*slot.get(&register)?).to_owned(),
            u: vertex_input(&sample.u),
            v: vertex_input(&sample.v),
        })
    };

    // the alpha test
    let mut coverage = BTreeSet::new();
    for i in &ps.function.instructions {
        let Some((code, _)) = ps.call(&i.op) else {
            continue;
        };
        if code != DISCARD {
            continue;
        }
        for condition in ps.discard_conditions(i.block) {
            let leaves = ps.leaves_of(condition);
            for &(register, lane) in &leaves.textures {
                if lane == 3 && slot.contains_key(&register) {
                    coverage.insert(register);
                } else if !slot.contains_key(&register) {
                    reads
                        .left_out
                        .insert("a discard against an engine texture (a screen-door dither)");
                }
            }
            for (name, index, component) in &leaves.inputs {
                if let Some(v) = origin.get(&(name.clone(), *index, *component))
                    && v.0 == "COLOR"
                    && v.2 == 3
                {
                    reads.coverage_vertex_alpha = true;
                }
            }
        }
    }
    for register in coverage {
        if let Some(read) = read_of(register) {
            reads.coverage.push(read);
        }
    }

    // the albedo, and its base in each lane
    let textures: BTreeMap<u32, &Texture> = meta.textures.iter().map(|t| (u32::from(t.slot), t)).collect();
    let mut colour = BTreeSet::new();
    let mut bases = BTreeSet::new();
    for i in &ps.function.instructions {
        let Some((code, args)) = ps.call(&i.op) else {
            continue;
        };
        if code != STORE_OUTPUT {
            continue;
        }
        let (Some(sig), Some(&value)) = (args.get(1).and_then(|v| ps.int(*v)), args.get(4)) else {
            continue;
        };
        let Some(e) = u32::try_from(sig).ok().and_then(|s| ps.outputs.get(&s)) else {
            continue;
        };
        if e.name != "SV_Target" || e.index != ALBEDO_TARGET {
            continue;
        }
        let mut lane_colour = BTreeSet::new();
        for (register, _) in ps.leaves_of(value).textures {
            if slot.contains_key(&register) {
                lane_colour.insert(register);
            }
        }
        // the colour's lanes (x, y, z) name the base; the alpha is other data (a lane of no material slot has none)
        let colour_lane = args
            .get(3)
            .and_then(|v| ps.int(*v))
            .is_some_and(|c| (0..3).contains(&(c - i64::from(e.column))));
        if colour_lane && !lane_colour.is_empty() {
            bases.insert(ps.base(value, &textures));
        }
        colour.extend(lane_colour);
    }
    if let [Some(base)] = bases.into_iter().collect::<Vec<_>>()[..] {
        reads.base = read_of(base);
    }
    for register in colour {
        if let Some(read) = read_of(register) {
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
pub fn vertex_uv_inputs(vertex: &[u8]) -> Result<Vec<(u32, u8)>, String> {
    let chunks = primase_dxcontainer::chunks(vertex).map_err(|e| e.to_string())?;
    let module = reflection::module(&chunks, b"DXIL")
        .map_err(|e| e.to_string())?
        .ok_or("no DXIL module")?;
    let (inputs, _) = reflection::elements(&module).map_err(|e| e.to_string())?;
    let mut out: Vec<(u32, u8)> = inputs
        .iter()
        .filter(|e| e.name == "TEXCOORD")
        .map(|e| (e.index, e.columns))
        .collect();
    out.sort_unstable();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{VertexInput, uv_set};

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
}
