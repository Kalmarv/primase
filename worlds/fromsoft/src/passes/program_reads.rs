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
//! What it does not read is named in [`Reads::left_out`]: a discard against an engine texture (the screen-door dither of
//! a fade) is not a material slot's coverage.
use crate::metaparam::Metaparam;
use primase_dxil::module::{Constant, Function, Instruction, Module, Op, Value, ValueId};
use primase_dxil::reflection::{self, Element, Range};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The render target the programs write the albedo to.
const ALBEDO_TARGET: u32 = 2;
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

    /// What a value is made of.
    fn leaves(&self, id: ValueId, seen: &mut BTreeSet<ValueId>, out: &mut Leaves) {
        if !seen.insert(id) {
            return;
        }
        let Some(i) = self.defs.get(&id) else {
            return;
        };
        match &i.op {
            Op::ExtractValue { aggregate, indices } => {
                if let Some(agg) = self.defs.get(aggregate)
                    && let Some((code, args)) = self.call(&agg.op)
                    && SAMPLES.contains(&code)
                    && let (Some(register), Some(&lane)) =
                        (args.get(1).and_then(|h| self.register(*h)), indices.first())
                {
                    out.textures.insert((register, lane as u8));
                    return;
                }
                self.leaves(*aggregate, seen, out);
            }
            Op::Call { args, .. } => {
                let Some((code, a)) = self.call(&i.op) else {
                    for x in args {
                        self.leaves(*x, seen, out);
                    }
                    return;
                };
                match code {
                    LOAD_INPUT => {
                        if let (Some(sig), Some(col)) = (
                            a.get(1).and_then(|v| self.int(*v)),
                            a.get(3).and_then(|v| self.int(*v)),
                        ) && let Some(e) = u32::try_from(sig).ok().and_then(|s| self.inputs.get(&s))
                        {
                            let component = (col - i64::from(e.column)).clamp(0, 255) as u8;
                            out.inputs.insert((e.name.clone(), e.index, component));
                        }
                    }
                    CBUFFER_LOAD => {}
                    _ => {
                        for x in a.iter().skip(1) {
                            self.leaves(*x, seen, out);
                        }
                    }
                }
            }
            op => {
                for x in op.operands() {
                    self.leaves(x, seen, out);
                }
            }
        }
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

    // the albedo
    let mut colour = BTreeSet::new();
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
        for (register, _) in ps.leaves_of(value).textures {
            if slot.contains_key(&register) {
                colour.insert(register);
            }
        }
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
