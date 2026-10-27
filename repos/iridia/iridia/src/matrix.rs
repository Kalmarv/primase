//! How a program holds a matrix in a constant buffer: the arrangement of its lines, read from what the program does.
//!
//! A matrix constant is 16-byte lines, and whether each line is a row or a column of the matrix the program applies
//! depends on the declared packing (`row_major`, `column_major`) and on the side the program multiplies on (`mul(M, v)`
//! or `mul(v, M)`), which no layout records. The program's arithmetic shows it: a line that is a row is combined with
//! a vector lane by lane into one sum (`dp4 r.x, v, cb[i]`), and a line that is a column is scaled by one lane of a
//! vector and summed lane by lane with the other columns (`mul r, v.xxxx, cb[i]`, `mad r, v.yyyy, cb[i + 1], r`).
//! [`lines`] reads which, in the value graph, whatever the packing, the side or the bytecode's lowering (a dot product
//! or the multiplies and adds it is made of).
//!
//! The matrix is the one the program applies to column vectors (`M * v`), the form core's matrix roles are stated in:
//! [`Majorness::Row`] is each line one of its rows ([`MatrixLines::Rows`] in a binding).
use std::collections::{BTreeSet, HashMap};

use naga::{BinaryOperator, MathFunction, UnaryOperator};
pub use primase_core::shader::Unshown;
use primase_core::shader::{ConstantBinding, MatrixLines};

use crate::flow::{At, Literal, Node, NodeId, Op, Values};
use crate::{ConstantType, Interface, Majorness, ProgramGraph, ResourceId, ResourceKind};

/// Which words of a constant buffer are the matrix's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Words {
    /// The words of these places (row `r`, lane `l` is word `4r + l`), read where the program states the place: one
    /// matrix.
    At(std::ops::Range<u32>),
    /// Every word the program reads at a place it computes: the matrices of a palette it chooses by an index. A
    /// matrix's line is told by what the index adds to the element's first (`3 * joint + 1` is the second line of the
    /// joint's matrix), so a blend of several elements' same line (`M[a] * wa + M[b] * wb`, a skinning program's) is
    /// one line, as the matrix it makes is.
    Indexed,
}

/// A word of the matrix: its line (a row of the buffer, or the line of the element the index adds) and its lane.
type Word = (i64, u32);

/// An index's line in its element: the whole number it adds to the rest (`base + 2` is line 2), 0 for an index that
/// adds none.
fn line_of(values: &Values, index: &[NodeId]) -> i64 {
    let literal = |id: NodeId| match values.node(id) {
        Node::Literal(Literal(v)) if v.fract() == 0.0 => Some(*v as i64),
        _ => None,
    };
    match *index {
        [id] => match values.node(id) {
            Node::Op {
                op: Op::Binary(BinaryOperator::Add),
                operands,
            } => match *operands.as_slice() {
                [a, b] => literal(b).or_else(|| literal(a)).unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        },
        _ => 0,
    }
}

/// The arrangement of the lines of the matrix at `words` of the constant buffer `resource`, as the program `values`
/// combines them: [`Majorness::Row`] where every sum of the matrix's words times other values takes them from one line,
/// [`Majorness::Column`] where every such sum takes them from one lane of several lines; otherwise why not
/// ([`Unshown`]).
pub fn lines(values: &Values, resource: ResourceId, words: &Words) -> Result<Majorness, Unshown> {
    let read = |id: NodeId| -> Option<Word> {
        let Node::Global {
            resource: Some(r),
            at,
        } = values.node(id)
        else {
            return None;
        };
        if *r != resource {
            return None;
        }
        match (words, at) {
            (Words::At(range), At::Component(c)) if range.contains(c) => Some((i64::from(c / 4), c % 4)),
            (Words::Indexed, At::Indexed { lane, index }) => {
                Some((line_of(values, index) + i64::from(lane / 4), lane % 4))
            }
            _ => None,
        }
    };
    let order = post_order(values);
    // the word of the matrix a node is, and the node that reads it: a read, or a choice (a select, a branch's merge)
    // whose ways that are the matrix's are that one word and whose other ways are made of none of the matrix (the
    // lifter's guard of an index out of range, `select(in range, word, 0)`; a program that takes another matrix's word
    // on a path where it does not apply this one, as the source's cloth programs do by a flag)
    let mut words: HashMap<NodeId, (NodeId, Word)> = HashMap::new();
    // the reads of the matrix's words a node is made of
    let mut made_of: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();
    // the matrix's words a node sums, each times a value not made of the matrix
    let mut terms: HashMap<NodeId, BTreeSet<Word>> = HashMap::new();
    let (mut along, mut across, mut seen) = (0usize, 0usize, false);
    for &id in &order {
        seen |= read(id).is_some();
        let ways: &[NodeId] = match values.node(id) {
            Node::Op {
                op: Op::Select,
                operands,
            } => operands.get(1..).unwrap_or(&[]),
            Node::Merge { arms, .. } => arms,
            _ => &[],
        };
        let chosen = || -> Option<(NodeId, Word)> {
            let mut matrix = ways.iter().filter_map(|w| words.get(w));
            let first = *matrix.next()?;
            let one = matrix.all(|(_, w)| *w == first.1);
            let others_clear = ways
                .iter()
                .filter(|w| !words.contains_key(w))
                .all(|w| made_of.get(w).is_none_or(BTreeSet::is_empty));
            (one && others_clear).then_some(first)
        };
        if let Some(found) = read(id).map(|w| (id, w)).or_else(chosen) {
            words.insert(id, found);
        }
        let word = |o: NodeId| words.get(&o).copied();
        let mut reads: BTreeSet<NodeId> = values
            .operands(id)
            .iter()
            .filter_map(|o| made_of.get(o))
            .flatten()
            .copied()
            .collect();
        reads.extend(word(id).map(|(at, _)| at));
        made_of.insert(id, reads);
        let of = |o: NodeId| terms.get(&o).cloned().unwrap_or_default();
        let reads_of = |o: NodeId| made_of.get(&o).cloned().unwrap_or_default();
        // a product's terms: a word of the matrix times a value not made of that word (another vector, or one the matrix
        // made of other words, as a program that applies two of a palette's matrices in turn), or such a sum scaled by
        // a value made of none of its words; never a word times itself (`dot(column, column)`)
        let product = |a: NodeId, b: NodeId| -> BTreeSet<Word> {
            match (word(a), word(b)) {
                (Some((at, w)), _) if !reads_of(b).contains(&at) => BTreeSet::from([w]),
                (_, Some((at, w))) if !reads_of(a).contains(&at) => BTreeSet::from([w]),
                _ if reads_of(a).is_disjoint(&reads_of(b)) && !of(a).is_empty() => of(a),
                _ if reads_of(a).is_disjoint(&reads_of(b)) => of(b),
                _ => BTreeSet::new(),
            }
        };
        // a choice's terms: those of its ways made of the matrix, where they all sum the same words (and the ways made
        // of none of it sum none). The source's cloth programs blend the joints' matrices line by line and choose each
        // blended word, by a flag, against VC_World's before dotting the position with them.
        let chosen_terms = || -> BTreeSet<Word> {
            let mut matrix = ways.iter().filter(|&&w| !reads_of(w).is_empty()).map(|&w| of(w));
            match matrix.next() {
                Some(first) if matrix.all(|t| t == first) => first,
                _ => BTreeSet::new(),
            }
        };
        let sum: BTreeSet<Word> = match values.node(id) {
            Node::Op { op: Op::Select, .. } | Node::Merge { .. } => chosen_terms(),
            Node::Op { op, operands } => match (op, operands.as_slice()) {
                (Op::Binary(BinaryOperator::Multiply), &[a, b]) => product(a, b),
                (Op::Binary(BinaryOperator::Add | BinaryOperator::Subtract), &[a, b]) => {
                    of(a).into_iter().chain(of(b)).collect()
                }
                (Op::Unary(UnaryOperator::Negate), &[a]) => of(a),
                (Op::Math(MathFunction::Fma), &[a, b, c]) => product(a, b).into_iter().chain(of(c)).collect(),
                (Op::Math(MathFunction::Dot), all) if all.len() % 2 == 0 => {
                    let (x, y) = all.split_at(all.len() / 2);
                    x.iter().zip(y).flat_map(|(&a, &b)| product(a, b)).collect()
                }
                _ => BTreeSet::new(),
            },
            _ => BTreeSet::new(),
        };
        if sum.len() >= 2 {
            let mut lines = sum.iter().map(|(line, _)| line);
            let mut lanes = sum.iter().map(|(_, lane)| lane);
            let first_line = lines.next();
            let first_lane = lanes.next();
            if lines.all(|l| Some(l) == first_line) {
                along += 1;
            } else if lanes.all(|l| Some(l) == first_lane) {
                across += 1;
            }
        }
        terms.insert(id, sum);
    }
    match (along, across) {
        (0, 0) if !seen => Err(Unshown::NotRead),
        (0, 0) => Err(Unshown::NotApplied),
        (_, 0) => Ok(Majorness::Row),
        (0, _) => Ok(Majorness::Column),
        (along, across) => Err(Unshown::BothWays { along, across }),
    }
}

/// States the lines ([`ConstantBinding::lines`]) of each matrix role among `constants`, bindings over the resources of
/// `interface`, from the program's value graph and its own interface ([`ProgramGraph`]; a program lifted again may
/// number its resources otherwise, so a buffer is found by its D3D binding): [`lines`] of the matrix's words at their
/// places, or, where the program's results are made of none of them there, of the words of its buffer the program
/// reads by an index, when the program's layout shows the matrix is its buffer's one array (a palette; the indexed
/// reads of a buffer holding other arrays could be theirs). A matrix whose lines the program does not show keeps its
/// role with [`MatrixLines::Unshown`] and the kind [`lines`] gives; every matrix where the graph could not be built
/// (`graph` is why), or whose buffer is not found in it, with [`Unshown::Unexamined`]. Returns how many are unshown.
pub fn state_lines(
    graph: Result<&ProgramGraph, &str>,
    interface: &Interface,
    constants: &mut [ConstantBinding],
) -> usize {
    let mut unshown = 0;
    for c in constants.iter_mut().filter(|c| c.role.is_matrix()) {
        let unexamined = |why: String| Unshown::Unexamined { why };
        let found = graph
            .map_err(|why| unexamined(format!("the value graph: {why}")))
            .and_then(|graph| {
                let d3d = interface
                    .resource(c.resource)
                    .and_then(|r| r.provenance.d3d)
                    .ok_or_else(|| {
                        unexamined("its buffer has no D3D binding to find it in the program by".into())
                    })?;
                let (buffer, layout) = graph
                    .interface
                    .resources
                    .iter()
                    .find_map(|r| match &r.kind {
                        ResourceKind::ConstantBuffer { layout }
                            if r.provenance
                                .d3d
                                .is_some_and(|b| (b.space, b.register) == (d3d.space, d3d.register)) =>
                        {
                            Some((r.id, layout))
                        }
                        _ => None,
                    })
                    .ok_or_else(|| {
                        unexamined("the program has no constant buffer at its D3D binding".into())
                    })?;
                let at = Words::At(c.offset / 4..(c.offset + c.size).div_ceil(4));
                match lines(&graph.values, buffer, &at) {
                    Err(Unshown::NotRead) => {
                        let end = c.offset + c.size;
                        let mut arrays = layout
                            .constants
                            .iter()
                            .filter(|k| matches!(k.ty, ConstantType::Array { .. }));
                        let alone = arrays.clone().next().is_some()
                            && arrays.all(|k| {
                                k.offset >= c.offset && k.ty.size().is_some_and(|n| k.offset + n <= end)
                            });
                        if !alone {
                            // the buffer's indexed reads, if any, may be another array's
                            return Err(Unshown::NotRead);
                        }
                        lines(&graph.values, buffer, &Words::Indexed)
                    }
                    other => other,
                }
            });
        c.lines = Some(match found {
            Ok(Majorness::Row) => MatrixLines::Rows,
            Ok(Majorness::Column) => MatrixLines::Columns,
            Err(why) => {
                unshown += 1;
                MatrixLines::Unshown(why)
            }
        });
    }
    unshown
}

/// The nodes the program's results are made of ([`Values::roots`]), each after its operands; an operand that is the
/// node's own loop-carried value (a cycle through [`Node::Loop`]) is not waited for.
fn post_order(values: &Values) -> Vec<NodeId> {
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    let mut open = BTreeSet::new();
    for root in values.roots() {
        let mut stack = vec![(root, false)];
        while let Some((id, expanded)) = stack.pop() {
            if expanded {
                open.remove(&id);
                if done.insert(id) {
                    order.push(id);
                }
                continue;
            }
            if done.contains(&id) || !open.insert(id) {
                continue;
            }
            stack.push((id, true));
            for o in values.operands(id) {
                if !done.contains(&o) && !open.contains(&o) {
                    stack.push((o, false));
                }
            }
        }
    }
    order
}
