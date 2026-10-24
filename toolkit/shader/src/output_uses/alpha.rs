//! Structural evidence for an alpha reference, without following discard control back into the output.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::shader::{AlphaComparison, ResourceId};

use crate::flow::{At, BinaryOperator as B, MathFunction as M, Node, NodeId, Op, UnaryOperator as U, Values};
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
    for id in v.reached_from(v.discards().iter().flatten().copied()) {
        let Node::Op {
            op: Op::Binary(op),
            operands,
        } = v.node(id)
        else {
            continue;
        };
        if operands.len() != 2 {
            continue;
        }
        let (a, b) = match op {
            B::Less | B::LessEqual => (operands[0], operands[1]),
            B::Greater | B::GreaterEqual => (operands[1], operands[0]),
            _ => continue,
        };
        // Normalize to alpha < reference, including clip(alpha-reference) and reference-alpha > 0.
        let signal = if is_word(v, b, buffer, word) {
            Some(a)
        } else if literal(v, b, 0.) {
            difference(v, a).and_then(|(s, r)| is_word(v, r, buffer, word).then_some(s))
        } else if literal(v, a, 0.) {
            difference(v, b).and_then(|(r, s)| is_word(v, r, buffer, word).then_some(s))
        } else {
            None
        };
        if let Some(compared) = signal.and_then(classify) {
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
    result
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
