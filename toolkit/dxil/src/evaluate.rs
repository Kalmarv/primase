//! Evaluation of a DXIL pixel shader's entry function: the LLVM instructions and the `dx.op` intrinsics of the
//! bounded subset, block by block along the taken branches. Anything outside it is refused by name before any
//! evaluation ([`Shader::census`] lists what a shader uses).
use crate::module::{BinaryOp, CastOp, Constant, Function, Module, Op, Type, TypeId, Value, ValueId};
use crate::reflection::{Element, Range};
use crate::{Chunk, Result, error};
use primase_shader::{Execution, PixelShader, Reflection, Sample, Sampler};
use std::collections::BTreeMap;

/// Instructions one evaluation may execute (the subset has no loops; this bounds a malformed body).
const MAX_STEPS: usize = 1 << 20;

/// `dx.op` opcodes of the subset, by number, with their names (DXIL operation table).
const OPS: &[(u64, &str)] = &[
    (4, "LoadInput"),
    (5, "StoreOutput"),
    (6, "FAbs"),
    (7, "Saturate"),
    (8, "IsNaN"),
    (9, "IsInf"),
    (10, "IsFinite"),
    (12, "Cos"),
    (13, "Sin"),
    (21, "Exp"),
    (22, "Frc"),
    (23, "Log"),
    (24, "Sqrt"),
    (25, "Rsqrt"),
    (26, "Round_ne"),
    (27, "Round_ni"),
    (28, "Round_pi"),
    (29, "Round_z"),
    (35, "FMax"),
    (36, "FMin"),
    (37, "IMax"),
    (38, "IMin"),
    (39, "UMax"),
    (40, "UMin"),
    (46, "FMad"),
    (48, "IMad"),
    (49, "UMad"),
    (54, "Dot2"),
    (55, "Dot3"),
    (56, "Dot4"),
    (57, "CreateHandle"),
    (59, "CBufferLoadLegacy"),
    (60, "Sample"),
];
/// Names of `dx.op` opcodes outside the subset that shaders are known to use, for refusal messages.
fn op_name(code: u64) -> String {
    let known: &[(u64, &str)] = &[
        (61, "SampleBias"),
        (62, "SampleLevel"),
        (63, "SampleGrad"),
        (64, "SampleCmp"),
        (65, "SampleCmpLevelZero"),
        (66, "TextureLoad"),
        (82, "Discard"),
        (83, "DerivCoarseX"),
        (84, "DerivCoarseY"),
        (85, "DerivFineX"),
        (86, "DerivFineY"),
        (58, "CBufferLoad"),
        (47, "Fma"),
        (14, "Tan"),
        (15, "Acos"),
        (16, "Asin"),
        (17, "Atan"),
    ];
    OPS.iter()
        .chain(known)
        .find(|(c, _)| *c == code)
        .map_or_else(|| format!("dx.op {code}"), |(_, n)| (*n).to_owned())
}

/// What a shader's entry function uses: LLVM instructions and `dx.op` operations by name, and what the subset
/// refuses (empty when it can be evaluated).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub instructions: BTreeMap<String, usize>,
    pub operations: BTreeMap<String, usize>,
    pub refused: BTreeMap<String, usize>,
}

/// A checked SM6 pixel shader: its program, entry function and the bindings its intrinsics name.
#[derive(Debug)]
pub struct Shader {
    module: Module,
    entry: usize,
    inputs: Vec<Element>,
    outputs: Vec<Element>,
    ranges: Vec<Range>,
    reflection: Reflection,
    constant_buffers: Vec<(u32, u32)>,
    textures: Vec<u32>,
    samplers: Vec<u32>,
    input_count: usize,
    census: Census,
}

#[derive(Clone, Debug, PartialEq)]
enum V {
    /// A scalar's bits: integers (sign-extended to 64 bits from their width are not assumed; masked to width),
    /// half/float/double by their bits.
    Bits(u64),
    Aggregate(Vec<V>),
    Handle {
        class: u8,
        slot: u32,
    },
    Undef,
}

fn ftz(x: f32) -> f32 {
    if x.is_subnormal() { 0f32.copysign(x) } else { x }
}
fn float(v: &V, at: usize) -> Result<f32> {
    match v {
        V::Bits(b) => Ok(ftz(f32::from_bits(*b as u32))),
        V::Undef => Err(error(at, "an undefined value is used")),
        _ => Err(error(at, "expected a scalar")),
    }
}
fn bits(v: &V, at: usize) -> Result<u64> {
    match v {
        V::Bits(b) => Ok(*b),
        V::Undef => Err(error(at, "an undefined value is used")),
        _ => Err(error(at, "expected a scalar")),
    }
}
fn fv(x: f32) -> V {
    V::Bits(u64::from(ftz(x).to_bits()))
}

impl Shader {
    /// Parse a container: the `DXIL` program, its entry point's signatures, resources and the reflection (named
    /// from the `STAT` part when present). Refuses a program outside the subset ([`Shader::census`]).
    pub fn parse(container: &[u8]) -> Result<Self> {
        let chunks: Vec<Chunk<'_>> = crate::chunks(container)?;
        let program = |kind: &[u8; 4]| -> Result<Option<Module>> {
            let Some(c) = chunks.iter().find(|c| &c.kind == kind) else {
                return Ok(None);
            };
            let part = crate::program(c.data)?;
            let blocks = crate::bitstream::read(part.bitcode, *b"BC\xc0\xde")?;
            Module::read(&blocks).map(Some)
        };
        let module = program(b"DXIL")?.ok_or_else(|| error(0, "no DXIL part"))?;
        let named = program(b"STAT")?;
        let entry_value = crate::reflection::entry(&module)?;
        let entry = module
            .functions
            .iter()
            .position(|f| f.value == entry_value)
            .ok_or_else(|| error(0, "entry point has no body"))?;
        let (inputs, outputs) = crate::reflection::elements(&module)?;
        let ranges: Vec<Range> = crate::reflection::ranges(&module)?
            .into_iter()
            .map(|r| r.0)
            .collect();
        let reflection = crate::reflection::reflection(&chunks, &module, named.as_ref())?;
        let constant_buffers = ranges
            .iter()
            .filter(|r| r.class == 2)
            .map(|r| (r.lower, r.extra.div_ceil(16)))
            .collect();
        let textures = ranges.iter().filter(|r| r.class == 0).map(|r| r.lower).collect();
        let samplers = ranges.iter().filter(|r| r.class == 3).map(|r| r.lower).collect();
        let input_count = inputs
            .iter()
            .filter(|e| e.placed)
            .map(|e| (e.row + e.rows) as usize)
            .max()
            .unwrap_or(0);
        let mut shader = Self {
            module,
            entry,
            inputs,
            outputs,
            ranges,
            reflection,
            constant_buffers,
            textures,
            samplers,
            input_count,
            census: Census::default(),
        };
        shader.census = shader.take_census();
        if let Some((what, _)) = shader.census.refused.iter().next() {
            return Err(error(0, format!("outside the evaluated subset: {what}")));
        }
        Ok(shader)
    }
    pub fn census(&self) -> &Census {
        &self.census
    }
    /// The entry point's input signature elements (packing, semantics, component types, interpolation).
    pub fn input_elements(&self) -> &[Element] {
        &self.inputs
    }
    /// The entry point's output signature elements.
    pub fn output_elements(&self) -> &[Element] {
        &self.outputs
    }
    /// HLSL of a vertex shader that feeds this pixel shader, for checking it on a rasterizer: one clockwise
    /// triangle covering the pixel at the origin of a one-pixel viewport (SV_Position (0.5, 0.5, 0.5, 1)), and
    /// every other input read from `cbuffer Case : register(<case_register>) { uint4 r[32]; }`, row by input
    /// register, lanes from the element's first column. Elements are declared in signature order with the pixel
    /// shader's types and interpolation modes so the compiler packs them alike; system values the rasterizer
    /// generates (front face, sample index, ...) are left out.
    pub fn passthrough_vertex_shader(&self, case_register: &str) -> String {
        let scalar = |t: u8| match t {
            5 | 3 | 7 => "uint",
            4 | 2 | 6 => "int",
            _ => "float",
        };
        let cast = |t: u8| match t {
            5 | 3 | 7 => "",
            4 | 2 | 6 => "asint",
            _ => "asfloat",
        };
        let interpolation = |i: u8| match i {
            1 => "nointerpolation ",
            3 => "centroid ",
            4 => "noperspective ",
            5 => "noperspective centroid ",
            6 => "sample ",
            7 => "noperspective sample ",
            _ => "",
        };
        let lanes = ["x", "y", "z", "w"];
        let mut fields = String::new();
        let mut body = String::new();
        for e in &self.inputs {
            if e.system_value != 0 && e.system_value != 3 {
                continue;
            }
            let width = if e.columns > 1 {
                e.columns.to_string()
            } else {
                String::new()
            };
            let field = format!("e{}", e.id);
            fields.push_str(&format!(
                "    {}{}{} {} : {}{};\n",
                if e.system_value == 3 {
                    ""
                } else {
                    interpolation(e.interpolation)
                },
                scalar(e.component_type),
                width,
                field,
                e.name,
                if e.system_value == 3 {
                    String::new()
                } else {
                    e.index.to_string()
                }
            ));
            if e.system_value == 3 {
                body.push_str(&format!("    o.{field} = float4(p, 0.5, 1.0);\n"));
            } else {
                let swizzle: String = (0..e.columns).map(|c| lanes[usize::from(e.column + c)]).collect();
                body.push_str(&format!(
                    "    o.{field} = {}(r[{}].{swizzle});\n",
                    cast(e.component_type),
                    e.row
                ));
            }
        }
        format!(
            "cbuffer Case : register({case_register}) {{ uint4 r[32]; }};\n\
             struct Out\n{{\n{fields}}};\n\
             Out main(uint id : SV_VertexID)\n{{\n    Out o;\n    \
             float2 p = float2(id == 1 ? 3.0 : -1.0, id == 2 ? -3.0 : 1.0);\n{body}    return o;\n}}\n"
        )
    }
    fn function(&self) -> &Function {
        &self.module.functions[self.entry]
    }
    fn type_id(&self, f: &Function, v: ValueId) -> Option<TypeId> {
        if v < f.first {
            self.module.value_types.get(v).copied()
        } else {
            f.local_types.get(v - f.first).copied()
        }
    }
    fn local_type(&self, f: &Function, v: ValueId) -> Option<&Type> {
        self.module.types.get(self.type_id(f, v)?)
    }
    /// The `dx.op` opcode of a call: the callee must be a `dx.op.*` declaration and the first argument a constant.
    fn opcode(&self, f: &Function, function: ValueId, args: &[ValueId]) -> std::result::Result<u64, String> {
        let name = self.module.name(function).unwrap_or("");
        if !name.starts_with("dx.op.") {
            return Err(format!("call of {name}"));
        }
        match args.first().and_then(|&a| self.constant(f, a)) {
            Some(Constant::Integer(c)) => Ok(*c as u64),
            _ => Err(format!("{name} without a constant opcode")),
        }
    }
    fn constant<'a>(&'a self, f: &'a Function, v: ValueId) -> Option<&'a Constant> {
        let value = if v < f.first {
            self.module.values.get(v)?
        } else {
            f.locals.get(v - f.first)?
        };
        match value {
            Value::Constant(c) => Some(c),
            _ => None,
        }
    }
    fn take_census(&self) -> Census {
        let f = self.function();
        let mut c = Census::default();
        for inst in &f.instructions {
            let name = match &inst.op {
                Op::Binary { op, lhs, .. } => {
                    let float = matches!(
                        self.local_type(f, *lhs),
                        Some(Type::Float | Type::Half | Type::Double)
                    );
                    let n = format!("{op:?}");
                    if float { format!("F{n}") } else { n }
                }
                Op::Cast { op, .. } => format!("{op:?}"),
                Op::Compare { predicate, .. } => {
                    if *predicate < 32 {
                        "FCmp".into()
                    } else {
                        "ICmp".into()
                    }
                }
                Op::Select { .. } => "Select".into(),
                Op::ExtractValue { .. } => "ExtractValue".into(),
                Op::InsertValue { .. } => "InsertValue".into(),
                Op::Phi { .. } => "Phi".into(),
                Op::Return(_) => "Ret".into(),
                Op::Branch { .. } | Op::CondBranch { .. } => "Br".into(),
                Op::Call { function, args } => {
                    match self.opcode(f, *function, args) {
                        Ok(code) => {
                            let n = op_name(code);
                            if !OPS.iter().any(|(k, _)| *k == code) {
                                *c.refused.entry(n.clone()).or_default() += 1;
                            }
                            *c.operations.entry(n).or_default() += 1;
                        }
                        Err(e) => *c.refused.entry(e).or_default() += 1,
                    }
                    "Call".into()
                }
            };
            // Half and double arithmetic and integer types wider than 32 bits are outside the subset.
            if let Some((_, t)) = inst.result
                && matches!(self.module.types.get(t), Some(Type::Half | Type::Double))
            {
                *c.refused.entry("half or double value".into()).or_default() += 1;
            }
            if let Op::Cast {
                op: CastOp::PtrToInt | CastOp::IntToPtr,
                ..
            } = inst.op
            {
                *c.refused.entry("pointer cast".into()).or_default() += 1;
            }
            *c.instructions.entry(name).or_default() += 1;
        }
        c
    }

    fn value(&self, f: &Function, locals: &[V], v: ValueId, at: usize) -> Result<V> {
        if v >= f.first {
            return locals
                .get(v - f.first)
                .cloned()
                .ok_or_else(|| error(at, "value out of range"));
        }
        match self.module.values.get(v) {
            Some(Value::Constant(c)) => self.constant_value(c, at),
            _ => Err(error(at, "a module value that is not a constant")),
        }
    }
    fn constant_value(&self, c: &Constant, at: usize) -> Result<V> {
        Ok(match c {
            Constant::Integer(i) => V::Bits(*i as u64),
            Constant::Float(b) => V::Bits(*b),
            Constant::Null => V::Bits(0),
            Constant::Undef => V::Undef,
            _ => return Err(error(at, "an aggregate constant operand")),
        })
    }
    fn width(&self, t: TypeId) -> u32 {
        match self.module.types.get(t) {
            Some(Type::Integer(w)) => *w,
            _ => 32,
        }
    }

    /// Evaluate like [`PixelShader::execute`], and also return, per output register, the mask of written lanes whose
    /// value depends on an operation whose result Direct3D leaves to the implementation within a tolerance (Exp, Log,
    /// Sin, Cos, Rsqrt): another implementation may differ there by a few units in the last place.
    pub fn execute_marked(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<(Execution, Vec<u8>)> {
        self.evaluate(inputs, buffers, sampler)
    }

    fn evaluate(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<(Execution, Vec<u8>)> {
        if inputs.len() < self.input_count {
            return Err(error(0, "missing input registers"));
        }
        for &(slot, rows) in &self.constant_buffers {
            let given: Vec<_> = buffers.iter().filter(|(s, _)| *s == slot).collect();
            if given.len() != 1 || given[0].1.len() != rows as usize {
                return Err(error(0, format!("cb{slot} requires exactly {rows} rows")));
            }
        }
        let f = self.function();
        let output_rows = self
            .outputs
            .iter()
            .filter(|e| e.placed)
            .map(|e| (e.row + e.rows) as usize)
            .max()
            .unwrap_or(0);
        let mut outputs = vec![[0u32; 4]; output_rows];
        let mut masks = vec![0u8; output_rows];
        let mut inexact_masks = vec![0u8; output_rows];
        // The body's own constants hold their values from the start; arguments and results are set as they run.
        let mut locals = f
            .locals
            .iter()
            .map(|v| match v {
                Value::Constant(c) => self.constant_value(c, 0),
                _ => Ok(V::Undef),
            })
            .collect::<Result<Vec<V>>>()?;
        // Whether each local depends on an implementation-defined approximation.
        let mut inexact = vec![false; f.locals.len()];
        let mut block = 0usize;
        let mut previous: Option<usize> = None;
        let mut steps = 0usize;
        let mut samples = 0usize;
        let mut i = 0usize;
        // Start of each block in the instruction list.
        let starts: Vec<usize> = (0..f.blocks)
            .map(|b| {
                f.instructions
                    .iter()
                    .position(|x| x.block == b)
                    .unwrap_or(f.instructions.len())
            })
            .collect();
        loop {
            let inst = f
                .instructions
                .get(i)
                .ok_or_else(|| error(0, "function ends without return"))?;
            steps += 1;
            if steps > MAX_STEPS {
                return Err(error(inst.at, "evaluation step limit"));
            }
            let at = inst.at;
            let get = |v: ValueId, locals: &[V]| self.value(f, locals, v, at);
            let tainted = |v: &ValueId| *v >= f.first && inexact.get(v - f.first).copied().unwrap_or(false);
            let from_operands = match &inst.op {
                Op::Binary { lhs, rhs, .. } | Op::Compare { lhs, rhs, .. } => tainted(lhs) || tainted(rhs),
                Op::Cast { value, .. } => tainted(value),
                Op::Select {
                    condition,
                    then,
                    otherwise,
                } => tainted(condition) || tainted(then) || tainted(otherwise),
                Op::ExtractValue { aggregate, .. } => tainted(aggregate),
                Op::InsertValue { aggregate, value, .. } => tainted(aggregate) || tainted(value),
                Op::Phi { incoming } => incoming.iter().any(|(v, _)| tainted(v)),
                Op::Call { args, .. } => args.iter().any(tainted),
                _ => false,
            };
            let result: Option<V> = match &inst.op {
                Op::Return(_) => break,
                Op::Branch { target } => {
                    previous = Some(block);
                    block = *target;
                    i = starts[block];
                    continue;
                }
                Op::CondBranch {
                    condition,
                    then,
                    otherwise,
                } => {
                    let c = bits(&get(*condition, &locals)?, at)? & 1;
                    previous = Some(block);
                    block = if c == 1 { *then } else { *otherwise };
                    i = starts[block];
                    continue;
                }
                Op::Phi { incoming } => {
                    let from = previous.ok_or_else(|| error(at, "phi in the entry block"))?;
                    let (v, _) = incoming
                        .iter()
                        .find(|(_, b)| *b == from)
                        .ok_or_else(|| error(at, "phi has no value for the predecessor"))?;
                    Some(get(*v, &locals)?)
                }
                Op::Binary { op, lhs, rhs } => {
                    let a = get(*lhs, &locals)?;
                    let b = get(*rhs, &locals)?;
                    let is_float = matches!(self.local_type(f, *lhs), Some(Type::Float));
                    Some(if is_float {
                        let (x, y) = (float(&a, at)?, float(&b, at)?);
                        fv(match op {
                            BinaryOp::Add => x + y,
                            BinaryOp::Sub => x - y,
                            BinaryOp::Mul => x * y,
                            BinaryOp::SDiv | BinaryOp::UDiv => x / y,
                            BinaryOp::SRem | BinaryOp::URem => x % y,
                            other => return Err(error(at, format!("float {other:?}"))),
                        })
                    } else {
                        let w = self.type_id(f, *lhs).map_or(32, |t| self.width(t));
                        let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                        let (x, y) = (bits(&a, at)? & mask, bits(&b, at)? & mask);
                        let sx = ((x << (64 - w)) as i64) >> (64 - w);
                        let sy = ((y << (64 - w)) as i64) >> (64 - w);
                        let r = match op {
                            BinaryOp::Add => x.wrapping_add(y),
                            BinaryOp::Sub => x.wrapping_sub(y),
                            BinaryOp::Mul => x.wrapping_mul(y),
                            BinaryOp::UDiv => {
                                x.checked_div(y).ok_or_else(|| error(at, "division by zero"))?
                            }
                            BinaryOp::URem => {
                                x.checked_rem(y).ok_or_else(|| error(at, "division by zero"))?
                            }
                            BinaryOp::SDiv => {
                                sx.checked_div(sy).ok_or_else(|| error(at, "division by zero"))? as u64
                            }
                            BinaryOp::SRem => {
                                sx.checked_rem(sy).ok_or_else(|| error(at, "division by zero"))? as u64
                            }
                            BinaryOp::Shl => x.wrapping_shl((y % u64::from(w)) as u32),
                            BinaryOp::LShr => x.wrapping_shr((y % u64::from(w)) as u32),
                            BinaryOp::AShr => sx.wrapping_shr((y % u64::from(w)) as u32) as u64,
                            BinaryOp::And => x & y,
                            BinaryOp::Or => x | y,
                            BinaryOp::Xor => x ^ y,
                        };
                        V::Bits(r & mask)
                    })
                }
                Op::Cast { op, value } => {
                    let v = get(*value, &locals)?;
                    let (_, to) = inst.result.ok_or_else(|| error(at, "cast without a result"))?;
                    let from_w = match self.local_type(f, *value) {
                        Some(Type::Integer(w)) => *w,
                        _ => 32,
                    };
                    let to_w = self.width(to);
                    let mask = |w: u32| if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                    let sext = |b: u64, w: u32| (((b << (64 - w)) as i64) >> (64 - w)) as u64;
                    Some(match op {
                        CastOp::Trunc => V::Bits(bits(&v, at)? & mask(to_w)),
                        CastOp::ZExt => V::Bits(bits(&v, at)? & mask(from_w)),
                        CastOp::SExt => V::Bits(sext(bits(&v, at)?, from_w) & mask(to_w)),
                        CastOp::FpToUi => {
                            let x = float(&v, at)?;
                            // Truncation toward zero, clamped (NaN to zero), as dxbc's ftou.
                            V::Bits((x as u64).min(mask(to_w)))
                        }
                        CastOp::FpToSi => V::Bits((float(&v, at)? as i64 as u64) & mask(to_w)),
                        CastOp::UiToFp => fv((bits(&v, at)? & mask(from_w)) as f32),
                        CastOp::SiToFp => fv(sext(bits(&v, at)?, from_w) as i64 as f32),
                        CastOp::BitCast => v,
                        other => return Err(error(at, format!("cast {other:?}"))),
                    })
                }
                Op::Compare { predicate, lhs, rhs } => {
                    let a = get(*lhs, &locals)?;
                    let b = get(*rhs, &locals)?;
                    let r = if *predicate < 32 {
                        let (x, y) = (float(&a, at)?, float(&b, at)?);
                        let unordered = x.is_nan() || y.is_nan();
                        match predicate {
                            0 => false,
                            1 => !unordered && x == y,
                            2 => !unordered && x > y,
                            3 => !unordered && x >= y,
                            4 => !unordered && x < y,
                            5 => !unordered && x <= y,
                            6 => !unordered && x != y,
                            7 => !unordered,
                            8 => unordered,
                            9 => unordered || x == y,
                            10 => unordered || x > y,
                            11 => unordered || x >= y,
                            12 => unordered || x < y,
                            13 => unordered || x <= y,
                            14 => unordered || x != y,
                            15 => true,
                            _ => return Err(error(at, "float predicate")),
                        }
                    } else {
                        let w = match self.local_type(f, *lhs) {
                            Some(Type::Integer(w)) => *w,
                            _ => 32,
                        };
                        let m = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                        let (x, y) = (bits(&a, at)? & m, bits(&b, at)? & m);
                        let (sx, sy) = (
                            ((x << (64 - w)) as i64) >> (64 - w),
                            ((y << (64 - w)) as i64) >> (64 - w),
                        );
                        match predicate {
                            32 => x == y,
                            33 => x != y,
                            34 => x > y,
                            35 => x >= y,
                            36 => x < y,
                            37 => x <= y,
                            38 => sx > sy,
                            39 => sx >= sy,
                            40 => sx < sy,
                            41 => sx <= sy,
                            _ => return Err(error(at, "integer predicate")),
                        }
                    };
                    Some(V::Bits(u64::from(r)))
                }
                Op::Select {
                    condition,
                    then,
                    otherwise,
                } => {
                    let c = bits(&get(*condition, &locals)?, at)? & 1;
                    Some(if c == 1 {
                        get(*then, &locals)?
                    } else {
                        get(*otherwise, &locals)?
                    })
                }
                Op::ExtractValue { aggregate, indices } => {
                    let mut v = get(*aggregate, &locals)?;
                    for &k in indices {
                        v = match v {
                            V::Aggregate(mut items) if (k as usize) < items.len() => {
                                items.swap_remove(k as usize)
                            }
                            _ => return Err(error(at, "extractvalue of a non-aggregate")),
                        };
                    }
                    Some(v)
                }
                Op::InsertValue { .. } => return Err(error(at, "insertvalue")),
                Op::Call { function, args } => {
                    let code = self.opcode(f, *function, args).map_err(|e| error(at, e))?;
                    let arg = |n: usize, locals: &[V]| -> Result<V> {
                        get(*args.get(n).ok_or_else(|| error(at, "missing argument"))?, locals)
                    };
                    let mut stored = None;
                    let r = self.intrinsic(
                        code,
                        &arg,
                        &locals,
                        at,
                        inputs,
                        buffers,
                        sampler,
                        (&mut outputs, &mut masks),
                        &mut samples,
                        &mut stored,
                    )?;
                    // A stored lane is inexact exactly when the value last stored to it is.
                    if let Some((reg, lane)) = stored {
                        if from_operands {
                            inexact_masks[reg] |= 1 << lane;
                        } else {
                            inexact_masks[reg] &= !(1 << lane);
                        }
                    }
                    r
                }
            };
            if let (Some((id, _)), Some(v)) = (inst.result, result) {
                locals[id - f.first] = v;
                let approximate = matches!(&inst.op, Op::Call { function, args }
                    if matches!(self.opcode(f, *function, args), Ok(12 | 13 | 21 | 23 | 25)));
                inexact[id - f.first] = from_operands || approximate;
            }
            i += 1;
        }
        Ok((
            Execution {
                outputs,
                output_masks: masks,
                instructions: steps,
                samples,
            },
            inexact_masks,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn intrinsic(
        &self,
        code: u64,
        arg: &dyn Fn(usize, &[V]) -> Result<V>,
        locals: &[V],
        at: usize,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
        (outputs, masks): (&mut [[u32; 4]], &mut [u8]),
        samples: &mut usize,
        stored: &mut Option<(usize, usize)>,
    ) -> Result<Option<V>> {
        let a = |n: usize| arg(n, locals);
        let af = |n: usize| -> Result<f32> { float(&a(n)?, at) };
        let ai = |n: usize| -> Result<u64> { bits(&a(n)?, at) };
        let element = |list: &[Element], id: u64| -> Result<Element> {
            list.iter()
                .find(|e| u64::from(e.id) == id)
                .filter(|e| e.placed)
                .cloned()
                .ok_or_else(|| error(at, format!("signature element {id}")))
        };
        Ok(Some(match code {
            4 => {
                let e = element(&self.inputs, ai(1)?)?;
                let (row, col) = (ai(2)?, ai(3)?);
                if row >= u64::from(e.rows) || col >= u64::from(e.columns) {
                    return Err(error(at, "input element index"));
                }
                let reg = inputs
                    .get((u64::from(e.row) + row) as usize)
                    .ok_or_else(|| error(at, "input register"))?;
                V::Bits(u64::from(reg[usize::from(e.column) + col as usize]))
            }
            5 => {
                let e = element(&self.outputs, ai(1)?)?;
                let (row, col) = (ai(2)?, ai(3)?);
                if row >= u64::from(e.rows) || col >= u64::from(e.columns) {
                    return Err(error(at, "output element index"));
                }
                let r = (u64::from(e.row) + row) as usize;
                let lane = usize::from(e.column) + col as usize;
                outputs[r][lane] = bits(&a(4)?, at)? as u32;
                masks[r] |= 1 << lane;
                *stored = Some((r, lane));
                return Ok(None);
            }
            6 => fv(af(1)?.abs()),
            7 => fv(af(1)?.clamp(0., 1.).max(0.)),
            8 => V::Bits(u64::from(af(1)?.is_nan())),
            9 => V::Bits(u64::from(af(1)?.is_infinite())),
            10 => V::Bits(u64::from(af(1)?.is_finite())),
            12 => fv(af(1)?.cos()),
            13 => fv(af(1)?.sin()),
            21 => fv(af(1)?.exp2()),
            22 => {
                let x = af(1)?;
                fv(x - x.floor())
            }
            23 => fv(af(1)?.log2()),
            24 => fv(af(1)?.sqrt()),
            25 => fv(1. / af(1)?.sqrt()),
            26 => fv(af(1)?.round_ties_even()),
            27 => fv(af(1)?.floor()),
            28 => fv(af(1)?.ceil()),
            29 => fv(af(1)?.trunc()),
            35 => fv(af(1)?.max(af(2)?)),
            36 => fv(af(1)?.min(af(2)?)),
            37 => V::Bits(((ai(1)? as u32 as i32).max(ai(2)? as u32 as i32)) as u32 as u64),
            38 => V::Bits(((ai(1)? as u32 as i32).min(ai(2)? as u32 as i32)) as u32 as u64),
            39 => V::Bits((ai(1)? as u32).max(ai(2)? as u32) as u64),
            40 => V::Bits((ai(1)? as u32).min(ai(2)? as u32) as u64),
            // Separately rounded multiply and add, as dxbc's mad.
            46 => fv(ftz(af(1)? * af(2)?) + af(3)?),
            48 | 49 => V::Bits(u64::from(
                (ai(1)? as u32)
                    .wrapping_mul(ai(2)? as u32)
                    .wrapping_add(ai(3)? as u32),
            )),
            54..=56 => {
                let n = (code - 52) as usize;
                // Lane-order products and sequential additions, each rounded, as dxbc's dp2/3/4.
                let mut sum = 0f32;
                for k in 0..n {
                    let p = ftz(af(1 + k)? * af(1 + n + k)?);
                    sum = if k == 0 { p } else { ftz(sum + p) };
                }
                fv(sum)
            }
            57 => {
                let class = ai(1)? as u8;
                let id = ai(2)? as u32;
                let index = ai(3)? as u32;
                let range = self
                    .ranges
                    .iter()
                    .find(|r| r.class == class && r.id == id)
                    .ok_or_else(|| error(at, "handle of an undeclared range"))?;
                if index < range.lower || index - range.lower >= range.count.max(1) {
                    return Err(error(at, "handle index outside its range"));
                }
                V::Handle { class, slot: index }
            }
            59 => {
                let V::Handle { class: 2, slot } = a(1)? else {
                    return Err(error(at, "constant load from a non-buffer handle"));
                };
                let row = ai(2)? as usize;
                let buffer = buffers
                    .iter()
                    .find(|(s, _)| *s == slot)
                    .ok_or_else(|| error(at, format!("cb{slot} not bound")))?
                    .1;
                let r = buffer
                    .get(row)
                    .ok_or_else(|| error(at, "constant row out of range"))?;
                V::Aggregate(r.iter().map(|&x| V::Bits(u64::from(x))).collect())
            }
            60 => {
                let V::Handle {
                    class: 0,
                    slot: texture,
                } = a(1)?
                else {
                    return Err(error(at, "sample of a non-texture handle"));
                };
                let V::Handle { class: 3, slot: s } = a(2)? else {
                    return Err(error(at, "sample with a non-sampler handle"));
                };
                let uv = [af(3)?, af(4)?];
                for k in [7, 8, 9] {
                    // Offsets must be absent or zero; there is no LOD clamp.
                    match a(k)? {
                        V::Undef | V::Bits(0) => {}
                        _ => return Err(error(at, "sample offsets")),
                    }
                }
                if a(10)? != V::Undef {
                    return Err(error(at, "sample LOD clamp"));
                }
                *samples += 1;
                let rgba = sampler
                    .sample(Sample {
                        bias: 0.0,
                        texture,
                        sampler: s,
                        uv,
                        gradients: None,
                    })
                    .map_err(|e| error(at, e))?;
                V::Aggregate(rgba.iter().map(|&x| fv(x)).chain([V::Bits(0)]).collect())
            }
            other => return Err(error(at, format!("{} is not evaluated", op_name(other)))),
        }))
    }
}

impl PixelShader for Shader {
    fn reflection(&self) -> &Reflection {
        &self.reflection
    }
    fn input_count(&self) -> usize {
        self.input_count
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &self.constant_buffers
    }
    fn textures(&self) -> &[u32] {
        &self.textures
    }
    fn samplers(&self) -> &[u32] {
        &self.samplers
    }
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, primase_shader::Error> {
        self.evaluate(inputs, buffers, sampler)
            .map(|(e, _)| e)
            .map_err(|e| primase_shader::Error {
                bytecode: "DXIL",
                at: e.bit,
                detail: e.detail,
            })
    }
}
