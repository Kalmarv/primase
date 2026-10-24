//! What a pixel program's own discards and its targets' stated blend show an output lane is used for: the evidence that
//! binds a lane its source states no role for ([`OutputRole::Unknown`]) to [`OutputRole::Coverage`] or
//! [`OutputRole::Opacity`]. A world's output bindings call it after stating their own roles; a lane with a role keeps
//! it, and nothing is read from a program's or a target's name.
use primase_core::shader::{BlendFactor, OutputBinding, OutputRole, RenderState};

use crate::flow::Node;
use crate::{Interface, ProgramGraph, SystemValue};

/// The lanes [`bind_output_uses`] bound, for a pass's log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputUses {
    pub coverage: u32,
    pub opacity: u32,
}

/// Bind the Unknown lanes of a pixel stage's `outputs` that the evidence shows a use for:
///
/// - [`OutputRole::Opacity`], the alpha lane (lane 3) of a target whose stated blend weighs the colour by the source
///   alpha (`state`'s blend for the target: the colour equation's source or destination factor `SrcAlpha` or
///   `OneMinusSrcAlpha`);
/// - else [`OutputRole::Coverage`], a lane whose written value is among what a discard's conditions are made of
///   (`graph`, the program's value graph; a literal is no evidence, as equal literals are one node). No graph, no
///   Coverage.
///
/// `interface` is the stage's program's: an output is a target by its system value `SV_Target<n>`, or by its label's
/// index where it has none. An Unknown binding keeps its why on the lanes not bound.
pub fn bind_output_uses(
    outputs: &mut Vec<OutputBinding>,
    interface: &Interface,
    graph: Option<&ProgramGraph>,
    state: &RenderState,
) -> OutputUses {
    let tested = graph.map(|g| {
        g.values
            .reached_from(g.values.discards().iter().flatten().copied())
    });
    let mut uses = OutputUses::default();
    let mut bound = Vec::with_capacity(outputs.len());
    for o in outputs.drain(..) {
        let OutputRole::Unknown { why } = &o.role else {
            bound.push(o);
            continue;
        };
        let target = interface
            .outputs
            .iter()
            .find(|v| v.label == o.label)
            .and_then(|v| match v.system {
                None => Some(v.label.index),
                Some(SystemValue::Target(n)) => Some(n),
                Some(_) => None,
            });
        let (mut opacity, mut coverage) = (0u8, 0u8);
        for lane in 0..4u8 {
            let bit = 1u8 << lane;
            if o.lanes & bit == 0 || target.is_none() {
                continue;
            }
            if lane == 3 && target.is_some_and(|t| weighs_by_source_alpha(state, t)) {
                opacity |= bit;
            } else if let (Some(g), Some(tested)) = (graph, &tested)
                && let Some(id) = g.values.output_labelled(&g.interface, &o.label, lane)
                && !matches!(g.values.node(id), Node::Literal(_))
                && tested.contains(&id)
            {
                coverage |= bit;
            }
        }
        let rest = o.lanes & !(opacity | coverage);
        if rest != 0 {
            bound.push(OutputBinding {
                label: o.label.clone(),
                lanes: rest,
                role: OutputRole::unknown(why.clone()),
            });
        }
        for (lanes, role) in [(coverage, OutputRole::Coverage), (opacity, OutputRole::Opacity)] {
            if lanes != 0 {
                bound.push(OutputBinding {
                    label: o.label.clone(),
                    lanes,
                    role,
                });
            }
        }
        uses.coverage += coverage.count_ones();
        uses.opacity += opacity.count_ones();
    }
    *outputs = bound;
    uses
}

/// Whether a discard of the program compares a value it writes to an output with word `word` of constant buffer `buffer`
/// (as `graph` numbers its resources): among what a discard's conditions are made of, a comparison (`<`, `<=`, `>`,
/// `>=`) of the word itself and an output lane's value, either directly (`alpha < reference`) or as their difference
/// against zero (`clip(alpha - reference)`). What makes a constant an alpha test's reference
/// ([`primase_core::shader::ConstantRole::AlphaReference`]).
pub fn discard_compares(graph: &ProgramGraph, buffer: primase_core::shader::ResourceId, word: u32) -> bool {
    use crate::flow::{At, BinaryOperator, Op};
    let values = &graph.values;
    let written: std::collections::BTreeSet<_> = values.outputs().values().copied().collect();
    let is_word = |n| matches!(values.node(n), Node::Global { resource: Some(r), at: At::Component(w) } if *r == buffer && *w == word);
    let pair = |a, b| (is_word(a) && written.contains(&b)) || (is_word(b) && written.contains(&a));
    let zero = |n| matches!(values.node(n), Node::Literal(l) if l.0 == 0.);
    let difference = |n| match values.node(n) {
        Node::Op {
            op: Op::Binary(BinaryOperator::Subtract),
            operands,
        } if operands.len() == 2 => pair(operands[0], operands[1]),
        _ => false,
    };
    values
        .reached_from(values.discards().iter().flatten().copied())
        .into_iter()
        .any(|id| match values.node(id) {
            Node::Op {
                op:
                    Op::Binary(
                        BinaryOperator::Less
                        | BinaryOperator::LessEqual
                        | BinaryOperator::Greater
                        | BinaryOperator::GreaterEqual,
                    ),
                operands,
            } if operands.len() == 2 => {
                let (a, b) = (operands[0], operands[1]);
                pair(a, b) || (zero(b) && difference(a)) || (zero(a) && difference(b))
            }
            _ => false,
        })
}

/// Whether target `target`'s stated blend weighs the colour by the source alpha: the state's blend for that target
/// (render target 0's is also the state's own `blend`).
fn weighs_by_source_alpha(state: &RenderState, target: u32) -> bool {
    let blend = usize::try_from(target)
        .ok()
        .and_then(|t| state.targets.get(t).copied().flatten())
        .and_then(|t| t.blend)
        .or(if target == 0 { state.blend } else { None });
    blend.is_some_and(|b| {
        [b.colour.source, b.colour.destination]
            .iter()
            .any(|f| matches!(f, BlendFactor::SrcAlpha | BlendFactor::OneMinusSrcAlpha))
    })
}
