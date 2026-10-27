//! Which sampler each texture sample of a program reads through, from the program lifted by [`primase_dxil::lift`].
//!
//! The programs sample through one bindless heap of samplers (`g_Bindless_SamplerState`, an unbounded array of
//! samplers at register 0 of space 2) indexed at run time by a word of a constant buffer: a sample is
//! `textureSample(t14, s0[cb3[3].w], uv)` ("the texture at t14 in space 1, through the sampler whose heap index is the
//! fourth lane of the fourth row of the buffer at register 3 of space 0"). That buffer's constants are the program's
//! `gDynamicSamplerIndex_<name>` words ([`crate::rage::sgd2_table`]), so the sample names the sampler the program
//! means by its constant: the word at that offset. The heap's contents are the engine's. Some programs sample through
//! a sampler they declare instead ([`Through::Direct`]).
use std::collections::BTreeMap;

use naga::{BinaryOperator, Expression, Function, Handle, Literal, Statement};

/// A constant-buffer word: the buffer's bind point (space, register) and the word's byte offset in it.
pub type Word = ((u32, u32), u32);

/// What a texture sample reads through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Through {
    /// An element of the sampler heap, indexed by a constant-buffer word (the buffer's space and register and the
    /// word's byte offset in it).
    Heap { buffer: (u32, u32), offset: u32 },
    /// A sampler the program declares, by its bind point (space, register).
    Direct((u32, u32)),
}

/// One texture sample: the texture's bind point (space, register), the constant-buffer word that indexes it when the
/// texture is an array the program indexes at run time (a bindless heap of textures: the buffer's space and register
/// and the word's byte offset in it), and what it is read through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SamplerUse {
    pub texture: (u32, u32),
    pub image: Option<Word>,
    pub through: Through,
}

/// What the samples of a program are, and the ones that could not be traced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamplerUses {
    pub uses: Vec<SamplerUse>,
    /// Samples whose texture or sampler index is not one of the two shapes above, by what they are.
    pub untraced: Vec<String>,
    /// Samples (counted per sample, not per use) of a texture array whose index is computed, not a constant-buffer
    /// word: the sampler is traced, the element is the program's.
    pub computed_images: usize,
}

/// The (space, register) a global variable is bound at: the binding number holds the space in its high 16 bits.
fn bind_point(module: &naga::Module, g: Handle<naga::GlobalVariable>) -> Option<(u32, u32)> {
    let b = module.global_variables.try_get(g).ok()?.binding?;
    Some((b.binding >> 16, b.binding & 0xffff))
}

/// The global an expression addresses: through loads and element accesses.
fn global_of(function: &Function, mut at: Handle<Expression>) -> Option<Handle<naga::GlobalVariable>> {
    for _ in 0..16 {
        match function.expressions.try_get(at).ok()? {
            Expression::GlobalVariable(g) => return Some(*g),
            Expression::Load { pointer } => at = *pointer,
            Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => at = *base,
            _ => return None,
        }
    }
    None
}

fn literal(function: &Function, at: Handle<Expression>) -> Option<u32> {
    match function.expressions.try_get(at).ok()? {
        Expression::Literal(Literal::U32(v)) => Some(*v),
        Expression::Literal(Literal::I32(v)) => u32::try_from(*v).ok(),
        _ => None,
    }
}

/// A function and what is stored into its local variables as a whole (`v = value`): the heap index of a sample that
/// is picked by a branch is kept in a variable.
struct Assigned<'a> {
    function: &'a Function,
    stores: BTreeMap<usize, Vec<Handle<Expression>>>,
}

impl<'a> Assigned<'a> {
    fn new(function: &'a Function) -> Self {
        fn collect(
            function: &Function,
            block: &naga::Block,
            out: &mut BTreeMap<usize, Vec<Handle<Expression>>>,
        ) {
            for statement in block {
                match statement {
                    Statement::Store { pointer, value } => {
                        if let Ok(Expression::LocalVariable(v)) = function.expressions.try_get(*pointer) {
                            out.entry(v.index()).or_default().push(*value);
                        }
                    }
                    Statement::Block(inner) => collect(function, inner, out),
                    Statement::If { accept, reject, .. } => {
                        collect(function, accept, out);
                        collect(function, reject, out);
                    }
                    Statement::Switch { cases, .. } => {
                        cases.iter().for_each(|c| collect(function, &c.body, out))
                    }
                    Statement::Loop { body, continuing, .. } => {
                        collect(function, body, out);
                        collect(function, continuing, out);
                    }
                    _ => {}
                }
            }
        }
        let mut stores = BTreeMap::new();
        collect(function, &function.body, &mut stores);
        Self { function, stores }
    }

    /// The constant-buffer words an index expression can be: `cb[row].lane`, the same plus and minus literals that
    /// cancel (what the lifter writes for an index with a base), and the loads of a local variable whose every
    /// assignment is one of those. `None` when any way the expression can be computed is not.
    fn words(&self, module: &naga::Module, at: Handle<Expression>) -> Option<Vec<Word>> {
        let mut out = Vec::new();
        self.collect_words(module, at, 0, 0, &mut out)?;
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    fn collect_words(
        &self,
        module: &naga::Module,
        mut at: Handle<Expression>,
        mut delta: i64,
        depth: u32,
        out: &mut Vec<Word>,
    ) -> Option<()> {
        let function = self.function;
        if depth > 8 {
            return None;
        }
        for _ in 0..16 {
            match function.expressions.try_get(at).ok()? {
                Expression::Binary {
                    op: op @ (BinaryOperator::Add | BinaryOperator::Subtract),
                    left,
                    right,
                } => {
                    let sign = if *op == BinaryOperator::Add { 1 } else { -1 };
                    if let Some(c) = literal(function, *right) {
                        delta = delta.checked_add(sign * i64::from(c))?;
                        at = *left;
                    } else if let (BinaryOperator::Add, Some(c)) = (op, literal(function, *left)) {
                        delta = delta.checked_add(i64::from(c))?;
                        at = *right;
                    } else {
                        return None;
                    }
                }
                Expression::Load { pointer } => {
                    let Expression::LocalVariable(v) = function.expressions.try_get(*pointer).ok()? else {
                        return None;
                    };
                    let stored = self.stores.get(&v.index())?;
                    for value in stored {
                        self.collect_words(module, *value, delta, depth + 1, out)?;
                    }
                    return Some(());
                }
                Expression::AccessIndex { base, index: lane } => {
                    if delta != 0 {
                        return None;
                    }
                    // the row: a load of `cb[row]`
                    let Expression::Load { pointer } = function.expressions.try_get(*base).ok()? else {
                        return None;
                    };
                    let row = match function.expressions.try_get(*pointer).ok()? {
                        Expression::AccessIndex { index, .. } => *index,
                        Expression::Access { index, .. } => literal(function, *index)?,
                        _ => return None,
                    };
                    let g = global_of(function, *pointer)?;
                    let offset = row.checked_mul(16)?.checked_add(lane.checked_mul(4)?)?;
                    out.push((bind_point(module, g)?, offset));
                    return Some(());
                }
                _ => return None,
            }
        }
        None
    }
}

/// The index of the array element an image expression is, if it is one: `t[index]`, through loads.
fn image_index(function: &Function, mut at: Handle<Expression>) -> Option<Handle<Expression>> {
    for _ in 0..16 {
        match function.expressions.try_get(at).ok()? {
            Expression::Load { pointer } => at = *pointer,
            Expression::Access { index, .. } => return Some(*index),
            Expression::AccessIndex { base, .. } => at = *base,
            _ => return None,
        }
    }
    None
}

/// The kind of an expression, for the account of what was not traced: a load names what it loads from, an operation
/// its operands.
fn variant(function: &Function, at: Handle<Expression>) -> String {
    let Ok(e) = function.expressions.try_get(at) else {
        return "out of range".to_owned();
    };
    match e {
        Expression::Load { pointer } => format!("a load of {}", variant(function, *pointer)),
        Expression::Binary { op, left, right } => format!(
            "{op:?} of {} and {}",
            variant(function, *left),
            variant(function, *right)
        ),
        Expression::AccessIndex { base, .. } => format!("an element of {}", variant(function, *base)),
        Expression::Access { base, index } => format!(
            "an element of {} by {}",
            variant(function, *base),
            variant(function, *index)
        ),
        other => format!("{other:?}")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// The samples of a DXIL container's program, or why it cannot be lifted.
pub fn uses(container: &[u8]) -> Result<SamplerUses, String> {
    let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
    uses_program(&program)
}

/// The resolve pass already has this program lifted for constants, outputs and inspection facts.
pub(super) fn uses_program(program: &primase_shader::Program) -> Result<SamplerUses, String> {
    let function = &program
        .module
        .entry_points
        .first()
        .ok_or("a program without an entry point")?
        .function;
    let assigned = Assigned::new(function);
    let mut out = SamplerUses::default();
    for (_, expression) in function.expressions.iter() {
        let Expression::ImageSample { image, sampler, .. } = expression else {
            continue;
        };
        let texture = global_of(function, *image).and_then(|g| bind_point(&program.module, g));
        let element = match function.expressions.try_get(*sampler) {
            Ok(Expression::Access { index, .. }) => Some(*index),
            _ => None,
        };
        let through: Option<Vec<Through>> = match element {
            Some(i) => assigned.words(&program.module, i).map(|words| {
                words
                    .into_iter()
                    .map(|(buffer, offset)| Through::Heap { buffer, offset })
                    .collect()
            }),
            None => global_of(function, *sampler)
                .and_then(|g| bind_point(&program.module, g))
                .map(|at| vec![Through::Direct(at)]),
        };
        // the words that index the texture, where the texture is an array indexed at run time; an index computed from
        // something else (a frame counter masked into a noise array) names no word
        let images: Vec<Option<Word>> = match image_index(function, *image) {
            None => vec![None],
            Some(i) => match assigned.words(&program.module, i) {
                Some(words) => words.into_iter().map(Some).collect(),
                None => {
                    out.computed_images += 1;
                    vec![None]
                }
            },
        };
        let through_read = through.is_some();
        match texture.zip(through) {
            Some((texture, through)) if !through.is_empty() => {
                for image in images {
                    for through in &through {
                        let u = SamplerUse {
                            texture,
                            image,
                            through: *through,
                        };
                        if !out.uses.contains(&u) {
                            out.uses.push(u);
                        }
                    }
                }
            }
            _ => out.untraced.push(match (texture, element) {
                (None, _) => "a texture that is no bound global".to_owned(),
                (_, None) if global_of(function, *sampler).is_none() => {
                    format!(
                        "a sampler that is {}, not a declared sampler",
                        variant(function, *sampler)
                    )
                }
                (_, Some(i)) if !through_read => {
                    format!(
                        "a heap index that is {}, not a constant-buffer word",
                        variant(function, i)
                    )
                }
                _ => "a sample with no sampler to trace".to_owned(),
            }),
        }
    }
    out.uses.sort_unstable();
    out.untraced.sort_unstable();
    out.untraced.dedup();
    Ok(out)
}
