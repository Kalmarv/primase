//! Structural evidence for an alpha reference, without following discard control back into the output.
use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::{AlphaComparison, ResourceId};

use crate::flow::{
    At, BinaryOperator as B, MathFunction as M, Node, NodeId, Op, Read, UnaryOperator as U, Values,
};
use crate::{ProgramGraph, SystemValue};

/// The producer's established dither texture and its scale word, in the reference's buffer. These are graph resource
/// ids, not register numbers. The matcher knows no resource names. The producer must supply a finite scale and a
/// neutral 0.5 texture answer when freezing the test off: the recognized perturbation is then exactly zero.
#[derive(Clone, Copy, Debug)]
pub struct AlphaDither {
    pub texture: ResourceId,
    pub scale_word: u32,
}

/// How a discard compares an output alpha (or its pre-transform value) against a buffer word. The accepted dither
/// form is `S + (D.a - 0.5) * scale`, where S reaches a target's alpha through data only and D is a different texture
/// from every texture source of S. Coordinates, conditions, index expressions and unrelated output lanes do not
/// establish an alpha use. Other perturbations remain unconfirmed.
///
/// Where no test of the word is of an output alpha, but one is an exact expression (texture samples, constants and
/// arithmetic: [`alpha_tests`]) holding a texture's alpha lane, the discard still compares the word:
/// [`AlphaComparison::Unwritten`], the program writing no alpha the test could be a cutoff of. The expression is the
/// test; a consumer evaluates it ([`AlphaExpr::linear`]).
pub fn discard_compares(
    graph: &ProgramGraph,
    buffer: ResourceId,
    word: u32,
    dither: Option<AlphaDither>,
) -> Option<AlphaComparison> {
    let v = &graph.values;
    let mut alpha = BTreeMap::new();
    for (&(output, lane), &id) in v.outputs() {
        if lane == 3
            && graph.interface.outputs.get(output).is_some_and(|o| {
                matches!(o.system, Some(SystemValue::Target(_)))
                    || (o.system.is_none() && o.label.name.eq_ignore_ascii_case("SV_Target"))
            })
        {
            alpha_data(v, id, false, &mut alpha);
        }
    }
    let direct = |id| {
        alpha.get(&id).and_then(|&transformed| {
            has_signal(v, id).then_some(if transformed {
                AlphaComparison::Transformed
            } else {
                AlphaComparison::Output
            })
        })
    };
    let classify = |id| {
        if let Some(compared) = direct(id) {
            return Some(compared);
        }
        let d = dither?;
        let (a, b) = binary(v, id, B::Add)?;
        for (signal, perturbation) in [(a, b), (b, a)] {
            if direct(signal).is_some()
                && perturbation_is_dither(v, perturbation, buffer, d)
                && !v.reached_from([signal]).iter().any(|&n| {
                    matches!(v.node(n), Node::Texel { resource: Some(r), .. } if *r == d.texture)
                        || is_word(v, n, buffer, d.scale_word)
                })
            {
                return Some(AlphaComparison::Dithered {
                    scale_word: d.scale_word,
                });
            }
        }
        None
    };
    let mut result = None;
    // a test of the word whose value is an exact expression of texture alphas and constants, which reaches no target
    let mut unwritten = false;
    for id in v.reached_from(v.discards().iter().flatten().copied()) {
        let Some((signal, reference)) = comparison(v, id) else {
            continue;
        };
        if buffer_word(v, reference) != Some((buffer, word)) {
            continue;
        }
        let compared = classify(signal);
        if compared.is_none() {
            let tested = AlphaExpr::of(v, signal, 0);
            unwritten |= tested.has_alpha_texel() && tested.is_exact();
        }
        if let Some(compared) = compared {
            // If several tests use this reference, retain the qualification rather than silently promising equality.
            result = Some(match (result, compared) {
                (Some(d @ AlphaComparison::Dithered { .. }), _) => d,
                (_, d @ AlphaComparison::Dithered { .. }) => d,
                (Some(AlphaComparison::Transformed), _) | (_, AlphaComparison::Transformed) => {
                    AlphaComparison::Transformed
                }
                _ => AlphaComparison::Output,
            });
        }
    }
    result.or(unwritten.then_some(AlphaComparison::Unwritten))
}

/// A discard's comparison, normalized to `signal < reference` (including `clip(signal - reference)` and
/// `reference - signal > 0`) where `reference` is a constant-buffer word: `(signal, reference)`.
fn comparison(v: &Values, id: NodeId) -> Option<(NodeId, NodeId)> {
    let Node::Op {
        op: Op::Binary(op),
        operands,
    } = v.node(id)
    else {
        return None;
    };
    if operands.len() != 2 {
        return None;
    }
    let (a, b) = match op {
        B::Less | B::LessEqual => (operands[0], operands[1]),
        B::Greater | B::GreaterEqual => (operands[1], operands[0]),
        _ => return None,
    };
    if buffer_word(v, b).is_some() {
        Some((a, b))
    } else if literal(v, b, 0.) {
        difference(v, a).filter(|&(_, r)| buffer_word(v, r).is_some())
    } else if literal(v, a, 0.) {
        difference(v, b)
            .filter(|&(r, _)| buffer_word(v, r).is_some())
            .map(|(r, s)| (s, r))
    } else {
        None
    }
}

fn buffer_word(v: &Values, id: NodeId) -> Option<(ResourceId, u32)> {
    match v.node(id) {
        Node::Global {
            resource: Some(r),
            at: At::Component(w),
        } => Some((*r, *w)),
        _ => None,
    }
}

fn binary(v: &Values, id: NodeId, wanted: B) -> Option<(NodeId, NodeId)> {
    match v.node(id) {
        Node::Op {
            op: Op::Binary(op),
            operands,
        } if *op == wanted && operands.len() == 2 => Some((operands[0], operands[1])),
        _ => None,
    }
}

fn literal(v: &Values, id: NodeId, wanted: f64) -> bool {
    matches!(v.node(id), Node::Literal(n) if n.0 == wanted)
}

fn is_word(v: &Values, id: NodeId, buffer: ResourceId, word: u32) -> bool {
    matches!(v.node(id), Node::Global { resource: Some(r), at: At::Component(w) } if *r == buffer && *w == word)
}

fn difference(v: &Values, id: NodeId) -> Option<(NodeId, NodeId)> {
    if let Some(pair) = binary(v, id, B::Subtract) {
        return Some(pair);
    }
    let (a, b) = binary(v, id, B::Add)?;
    for (positive, negative) in [(a, b), (b, a)] {
        if let Node::Op {
            op: Op::Unary(U::Negate),
            operands,
        } = v.node(negative)
            && operands.len() == 1
        {
            return Some((positive, operands[0]));
        }
    }
    None
}

fn perturbation_is_dither(v: &Values, id: NodeId, buffer: ResourceId, d: AlphaDither) -> bool {
    let Some((a, b)) = binary(v, id, B::Multiply) else {
        return false;
    };
    [(a, b), (b, a)].iter().any(|&(centred, scale)| {
        if !is_word(v, scale, buffer, d.scale_word) {
            return false;
        }
        let texel = if let Some((a, b)) = binary(v, centred, B::Subtract) {
            literal(v, b, 0.5).then_some(a)
        } else if let Some((a, b)) = binary(v, centred, B::Add) {
            if literal(v, a, -0.5) {
                Some(b)
            } else {
                literal(v, b, -0.5).then_some(a)
            }
        } else {
            None
        };
        texel.is_some_and(
            |t| matches!(v.node(t), Node::Texel { resource: Some(r), lane: 3, .. } if *r == d.texture),
        )
    })
}

fn alpha_data(v: &Values, id: NodeId, transformed: bool, out: &mut BTreeMap<NodeId, bool>) {
    if out.get(&id).is_some_and(|old| !*old || transformed) {
        return;
    }
    out.insert(id, transformed);
    match v.node(id) {
        Node::Merge { arms, .. } => {
            for &arm in arms {
                alpha_data(v, arm, transformed, out);
            }
        }
        Node::Op {
            op: Op::Select,
            operands,
        } => {
            for &arm in operands.iter().skip(1) {
                alpha_data(v, arm, transformed, out);
            }
        }
        Node::Op {
            op: Op::Math(M::Saturate),
            operands,
        } => {
            for &operand in operands {
                alpha_data(v, operand, true, out);
            }
        }
        Node::Op {
            op: Op::Binary(B::Multiply),
            operands,
        } => {
            for &operand in operands {
                alpha_data(v, operand, true, out);
            }
        }
        Node::Op {
            op: Op::Math(M::Clamp),
            operands,
        } if operands.len() == 3 && literal(v, operands[1], 0.) && literal(v, operands[2], 1.) => {
            alpha_data(v, operands[0], true, out);
        }
        _ => {}
    }
}

fn has_signal(v: &Values, id: NodeId) -> bool {
    let mut pending = vec![id];
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        match v.node(id) {
            Node::Texel { .. } | Node::Input { .. } => return true,
            Node::Op {
                op: Op::Select,
                operands,
            } => pending.extend(operands.iter().skip(1)),
            Node::Op { operands, .. } => pending.extend(operands),
            Node::Merge { arms, .. } => pending.extend(arms),
            _ => {}
        }
    }
    false
}

/// A value of a program's alpha test, as the program's own graph states it: texture samples, constant-buffer words and
/// literals with the exact arithmetic between them. Anything else (a vertex input, a select, a function) is
/// [`AlphaExpr::Other`], named, never guessed at.
#[derive(Clone, Debug, PartialEq)]
pub enum AlphaExpr {
    Literal(f64),
    /// A word of a constant buffer: the buffer's resource and the word's index.
    Word {
        buffer: ResourceId,
        word: u32,
    },
    /// A lane of a texture sample. `sample` identifies the read (two samples of one texture at different coordinates are
    /// two values).
    Texel {
        sample: NodeId,
        resource: ResourceId,
        lane: u8,
    },
    Negate(Box<AlphaExpr>),
    Add(Box<AlphaExpr>, Box<AlphaExpr>),
    Subtract(Box<AlphaExpr>, Box<AlphaExpr>),
    Multiply(Box<AlphaExpr>, Box<AlphaExpr>),
    /// What the expression is made of that this does not follow, as the graph names it.
    Other(String),
}

/// How deep an alpha test's expression is followed before it is [`AlphaExpr::Other`].
const MAX_EXPR_DEPTH: u32 = 48;

impl AlphaExpr {
    fn of(v: &Values, id: NodeId, depth: u32) -> AlphaExpr {
        if depth > MAX_EXPR_DEPTH {
            return AlphaExpr::Other(format!(
                "an expression more than {MAX_EXPR_DEPTH} operations deep"
            ));
        }
        let below = |n: NodeId| Box::new(AlphaExpr::of(v, n, depth + 1));
        match v.node(id) {
            Node::Literal(n) => AlphaExpr::Literal(n.0),
            Node::Global {
                resource: Some(buffer),
                at: At::Component(word),
            } => AlphaExpr::Word {
                buffer: *buffer,
                word: *word,
            },
            Node::Texel {
                resource: Some(resource),
                lane,
                read: Read::Sample,
                ..
            } => AlphaExpr::Texel {
                sample: id,
                resource: *resource,
                lane: *lane,
            },
            Node::Op {
                op: Op::Unary(U::Negate),
                operands,
            } if operands.len() == 1 => AlphaExpr::Negate(below(operands[0])),
            Node::Op {
                op: Op::Binary(op @ (B::Add | B::Subtract | B::Multiply)),
                operands,
            } if operands.len() == 2 => {
                let (a, b) = (below(operands[0]), below(operands[1]));
                match op {
                    B::Add => AlphaExpr::Add(a, b),
                    B::Subtract => AlphaExpr::Subtract(a, b),
                    _ => AlphaExpr::Multiply(a, b),
                }
            }
            other => AlphaExpr::Other(format!("{other:?}")),
        }
    }

    fn walk(&self, f: &mut dyn FnMut(&AlphaExpr)) {
        f(self);
        match self {
            AlphaExpr::Negate(a) => a.walk(f),
            AlphaExpr::Add(a, b) | AlphaExpr::Subtract(a, b) | AlphaExpr::Multiply(a, b) => {
                a.walk(f);
                b.walk(f);
            }
            _ => {}
        }
    }

    /// Whether the expression holds the alpha lane (lane 3) of a texture sample.
    pub fn has_alpha_texel(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| found |= matches!(e, AlphaExpr::Texel { lane: 3, .. }));
        found
    }

    /// Whether the expression is made only of what it follows (no [`AlphaExpr::Other`]).
    pub fn is_exact(&self) -> bool {
        let mut exact = true;
        self.walk(&mut |e| exact &= !matches!(e, AlphaExpr::Other(_)));
        exact
    }

    /// The expression's texture samples and constant words, once each, in the order it reads them.
    pub fn leaves(&self) -> Vec<AlphaExpr> {
        let mut out: Vec<AlphaExpr> = Vec::new();
        self.walk(&mut |e| {
            if matches!(e, AlphaExpr::Word { .. } | AlphaExpr::Texel { .. }) && !out.contains(e) {
                out.push(e.clone());
            }
        });
        out
    }
}

impl std::fmt::Display for AlphaExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlphaExpr::Literal(n) => write!(f, "{n}"),
            AlphaExpr::Word { buffer, word } => write!(f, "cb{}[{word}]", buffer.0),
            AlphaExpr::Texel { resource, lane, .. } => write!(
                f,
                "tex{}.{}",
                resource.0,
                ["x", "y", "z", "w"][usize::from(*lane & 3)]
            ),
            AlphaExpr::Negate(a) => write!(f, "-({a})"),
            AlphaExpr::Add(a, b) => write!(f, "({a} + {b})"),
            AlphaExpr::Subtract(a, b) => write!(f, "({a} - {b})"),
            AlphaExpr::Multiply(a, b) => write!(f, "({a} * {b})"),
            AlphaExpr::Other(what) => write!(f, "<{what}>"),
        }
    }
}

/// One discard of a program: it discards where `tested` is below the constant-buffer word `word` of `buffer`.
#[derive(Clone, Debug, PartialEq)]
pub struct AlphaTest {
    pub buffer: ResourceId,
    pub word: u32,
    pub tested: AlphaExpr,
}

/// Every comparison of a discard's condition against a constant-buffer word, as the program's graph states it, normalized
/// to `tested < word` ([`comparison`]'s forms). The caller picks the ones it means by what they are made of; this names
/// no resource.
pub fn alpha_tests(graph: &ProgramGraph) -> Vec<AlphaTest> {
    let v = &graph.values;
    let mut tests = Vec::new();
    for id in v.reached_from(v.discards().iter().flatten().copied()) {
        let Some((signal, reference)) = comparison(v, id) else {
            continue;
        };
        let Some((buffer, word)) = buffer_word(v, reference) else {
            continue;
        };
        let test = AlphaTest {
            buffer,
            word,
            tested: AlphaExpr::of(v, signal, 0),
        };
        if !tests.contains(&test) {
            tests.push(test);
        }
    }
    tests
}

/// What a consumer states of a leaf of an alpha test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Known {
    /// The leaf's value.
    Value(f64),
    /// A texture sample that varies over the surface: the one value the cutoff is solved for.
    Varies,
    /// Not stated.
    Unknown,
}

/// An alpha test's tested value as `scale * sample + offset`, with at most one varying sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Linear {
    /// The varying sample (its read, texture and lane); `None` where the value is a constant.
    pub sample: Option<(NodeId, ResourceId, u8)>,
    pub scale: f64,
    pub offset: f64,
}

impl Linear {
    fn constant(offset: f64) -> Linear {
        Linear {
            sample: None,
            scale: 0.,
            offset,
        }
    }

    /// Where the test discards `scale * sample + offset < reference`: the lowest sample value kept, or `None` where no
    /// sample is discarded. An error where a mask cannot say it: every sample is discarded, or the test keeps the
    /// samples below a value.
    pub fn cutoff(&self, reference: f64) -> Result<Option<f64>, String> {
        if self.sample.is_none() || self.scale == 0. {
            return if self.offset < reference {
                Err(format!(
                    "the test discards every sample ({} < {reference})",
                    self.offset
                ))
            } else {
                Ok(None)
            };
        }
        if self.scale < 0. {
            return Err(format!(
                "the test keeps the samples below a value (scale {} on the sample)",
                self.scale
            ));
        }
        Ok(Some((reference - self.offset) / self.scale))
    }
}

impl AlphaExpr {
    /// The expression as `scale * sample + offset`, the leaves valued by `known`. An error names why it is not one: a
    /// leaf nobody states, a product of two varying samples, two different varying samples, or what the expression is
    /// made of that this does not follow.
    pub fn linear(&self, known: &mut dyn FnMut(&AlphaExpr) -> Known) -> Result<Linear, String> {
        match self {
            AlphaExpr::Literal(n) => Ok(Linear::constant(*n)),
            AlphaExpr::Word { .. } | AlphaExpr::Texel { .. } => match known(self) {
                Known::Value(n) => Ok(Linear::constant(n)),
                Known::Varies => match self {
                    AlphaExpr::Texel {
                        sample,
                        resource,
                        lane,
                    } => Ok(Linear {
                        sample: Some((*sample, *resource, *lane)),
                        scale: 1.,
                        offset: 0.,
                    }),
                    _ => Err(format!("{self} is a constant word, not a sample that varies")),
                },
                Known::Unknown => Err(format!("{self} is not stated")),
            },
            AlphaExpr::Negate(a) => {
                let a = a.linear(known)?;
                Ok(Linear {
                    sample: a.sample,
                    scale: -a.scale,
                    offset: -a.offset,
                })
            }
            AlphaExpr::Add(a, b) => combine(a.linear(known)?, b.linear(known)?, 1.),
            AlphaExpr::Subtract(a, b) => combine(a.linear(known)?, b.linear(known)?, -1.),
            AlphaExpr::Multiply(a, b) => {
                let (x, y) = (a.linear(known)?, b.linear(known)?);
                match (x.sample, y.sample) {
                    (Some(_), Some(_)) => Err(format!("{self} multiplies two varying samples")),
                    (Some(_), None) => Ok(Linear {
                        sample: x.sample,
                        scale: x.scale * y.offset,
                        offset: x.offset * y.offset,
                    }),
                    (None, Some(_)) => Ok(Linear {
                        sample: y.sample,
                        scale: y.scale * x.offset,
                        offset: y.offset * x.offset,
                    }),
                    (None, None) => Ok(Linear::constant(x.offset * y.offset)),
                }
            }
            AlphaExpr::Other(what) => Err(format!("the test is made of {what}")),
        }
    }
}

/// `x + sign * y`.
fn combine(x: Linear, y: Linear, sign: f64) -> Result<Linear, String> {
    let sample = match (x.sample, y.sample) {
        (Some(a), Some(b)) if a.0 != b.0 => {
            return Err("the test adds two different varying samples".into());
        }
        (a, b) => a.or(b),
    };
    Ok(Linear {
        sample,
        scale: x.scale + sign * y.scale,
        offset: x.offset + sign * y.offset,
    })
}
