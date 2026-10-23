//! Structured control flow from a control flow graph, for lifters whose source has gotos (DXIL's basic blocks, any
//! SPIR-V-like graph): naga has `if`, `loop`, `switch`, `break`, `continue` and `return`, nothing else.
//!
//! [`structure`] is Ramsey's translation of a reducible graph by its dominator tree ("Beyond Relooper", ICFP 2022):
//! a node that several forward edges reach becomes a labelled block its predecessors branch out of, a loop header a
//! labelled loop its back edges continue. An irreducible graph (a back edge whose target does not dominate its source)
//! is refused by name; nothing is duplicated and no flag variable invented for it.
//!
//! [`lower`] writes the result as naga statements. naga's `break` leaves the innermost loop or switch only, so a
//! branch to a label further out is routed: it sets a jump variable and breaks, and each construct it leaves checks the
//! variable after it and carries on outward. That is the one place a variable is introduced, and only where a branch
//! really crosses a construct.

use std::collections::BTreeMap;

use naga::{
    BinaryOperator, Block, Expression, Handle, LocalVariable, Span, Statement, SwitchCase, SwitchValue,
};

use super::{Builder, Kind};

/// How a node of the graph ends. Targets are nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// Leaves the program (a return, an unreachable).
    Exit,
    Goto(usize),
    /// A two-way branch: `then`, `otherwise`.
    If(usize, usize),
    /// A multi-way branch: the targets in case order, the default last.
    Switch(Vec<usize>),
}

/// A control flow graph: node 0 is the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub terms: Vec<Term>,
}

/// A label: the node whose code follows the block it names (for a block), or the header of the loop (for a loop).
pub type Label = usize;

/// The structured form of a graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    /// The node's own statements (not its terminator).
    Node(usize),
    Seq(Vec<Code>),
    /// Code that a [`Code::Br`] to the label leaves for what follows.
    Block(Label, Box<Code>),
    /// Code that a [`Code::Continue`] to the label restarts; falling off its end leaves it.
    Loop(Label, Box<Code>),
    /// The two-way branch that ends the node: its `then` and `otherwise` code.
    If(usize, Box<Code>, Box<Code>),
    /// The multi-way branch that ends the node: the code of each target in case order, the default last.
    Switch(usize, Vec<Code>),
    Br(Label),
    Continue(Label),
    /// The values the edge carries (phi copies), before the branch it belongs to.
    Edge(usize, usize),
    /// The node leaves the program.
    Exit(usize),
}

/// Why a graph cannot be structured.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error(
        "irreducible control flow: the loop entered at node {entry} has a second entry through node {other}"
    )]
    Irreducible { entry: usize, other: usize },
}

fn successors(term: &Term) -> Vec<usize> {
    match term {
        Term::Exit => Vec::new(),
        Term::Goto(t) => vec![*t],
        Term::If(a, b) => vec![*a, *b],
        Term::Switch(ts) => ts.clone(),
    }
}

struct Analysis<'a> {
    cfg: &'a Cfg,
    /// Reverse postorder number of each reachable node.
    rpo: Vec<Option<usize>>,
    /// Immediate dominator of each reachable node (the entry's is itself).
    idom: Vec<usize>,
    /// Dominator tree children, in reverse postorder.
    children: Vec<Vec<usize>>,
    /// Forward in-edges, with multiplicity.
    forward_in: Vec<usize>,
    loop_header: Vec<bool>,
}

fn analyse(cfg: &Cfg) -> Result<Analysis<'_>, Error> {
    let n = cfg.terms.len();
    // depth-first postorder from the entry, iteratively
    let mut post = Vec::new();
    let mut seen = vec![false; n];
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some((node, next)) = stack.pop() {
        let succ = successors(&cfg.terms[node]);
        if next < succ.len() {
            stack.push((node, next + 1));
            let s = succ[next];
            if !seen[s] {
                seen[s] = true;
                stack.push((s, 0));
            }
        } else {
            post.push(node);
        }
    }
    let order: Vec<usize> = post.iter().rev().copied().collect();
    let mut rpo = vec![None; n];
    for (i, &node) in order.iter().enumerate() {
        rpo[node] = Some(i);
    }
    let rpo_of = |node: usize| rpo[node].expect("reachable");
    // predecessors of reachable nodes
    let mut preds = vec![Vec::new(); n];
    for &node in &order {
        for s in successors(&cfg.terms[node]) {
            preds[s].push(node);
        }
    }
    // Cooper, Harvey and Kennedy's iterative dominators
    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[0] = Some(0);
    let intersect = |idom: &[Option<usize>], mut a: usize, mut b: usize| {
        while a != b {
            while rpo_of(a) > rpo_of(b) {
                a = idom[a].expect("processed");
            }
            while rpo_of(b) > rpo_of(a) {
                b = idom[b].expect("processed");
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &node in order.iter().skip(1) {
            let mut new: Option<usize> = None;
            for &p in &preds[node] {
                if idom[p].is_none() {
                    continue;
                }
                new = Some(match new {
                    None => p,
                    Some(q) => intersect(&idom, p, q),
                });
            }
            if new != idom[node] {
                idom[node] = new;
                changed = true;
            }
        }
    }
    let idom: Vec<usize> = idom.into_iter().map(|d| d.unwrap_or(0)).collect();
    let dominates = |a: usize, mut b: usize| {
        loop {
            if a == b {
                return true;
            }
            if b == 0 {
                return false;
            }
            b = idom[b];
        }
    };
    let mut forward_in = vec![0; n];
    let mut loop_header = vec![false; n];
    for &node in &order {
        for s in successors(&cfg.terms[node]) {
            if rpo_of(s) > rpo_of(node) {
                forward_in[s] += 1;
            } else {
                // a back edge (or a self loop): its target must dominate its source
                if !dominates(s, node) {
                    return Err(Error::Irreducible {
                        entry: s,
                        other: node,
                    });
                }
                loop_header[s] = true;
            }
        }
    }
    let mut children = vec![Vec::new(); n];
    for &node in order.iter().skip(1) {
        children[idom[node]].push(node);
    }
    Ok(Analysis {
        cfg,
        rpo,
        idom,
        children,
        forward_in,
        loop_header,
    })
}

impl Analysis<'_> {
    fn is_merge(&self, node: usize) -> bool {
        self.forward_in[node] >= 2
    }

    fn rpo(&self, node: usize) -> usize {
        self.rpo[node].expect("reachable")
    }

    /// `x`'s code with the blocks of its merge-node children wrapped around it.
    fn do_tree(&self, x: usize, context: &mut Vec<Label>) -> Code {
        let mut ys: Vec<usize> = self.children[x]
            .iter()
            .copied()
            .filter(|&y| self.is_merge(y))
            .collect();
        ys.sort_by_key(|&y| self.rpo(y));
        let inner = self.node_within(x, &ys, context);
        if self.loop_header[x] {
            Code::Loop(x, Box::new(inner))
        } else {
            inner
        }
    }

    fn node_within(&self, x: usize, ys: &[usize], context: &mut Vec<Label>) -> Code {
        match ys.split_last() {
            None => self.translate(x, context),
            Some((&y, rest)) => {
                context.push(y);
                let inside = self.node_within(x, rest, context);
                context.pop();
                Code::Seq(vec![Code::Block(y, Box::new(inside)), self.do_tree(y, context)])
            }
        }
    }

    fn translate(&self, x: usize, context: &mut Vec<Label>) -> Code {
        let mut seq = vec![Code::Node(x)];
        match &self.cfg.terms[x] {
            Term::Exit => seq.push(Code::Exit(x)),
            Term::Goto(t) => seq.push(self.branch(x, *t, context)),
            Term::If(a, b) => seq.push(Code::If(
                x,
                Box::new(self.branch(x, *a, context)),
                Box::new(self.branch(x, *b, context)),
            )),
            Term::Switch(targets) => seq.push(Code::Switch(
                x,
                targets.iter().map(|&t| self.branch(x, t, context)).collect(),
            )),
        }
        Code::Seq(seq)
    }

    /// The edge `from -> to`: its copies, then a continue to a loop, a break to a block, or the target's own code.
    fn branch(&self, from: usize, to: usize, context: &mut Vec<Label>) -> Code {
        let edge = Code::Edge(from, to);
        if self.rpo(to) <= self.rpo(from) {
            Code::Seq(vec![edge, Code::Continue(to)])
        } else if self.is_merge(to) {
            debug_assert!(
                context.contains(&to),
                "a merge node's block encloses its predecessors"
            );
            Code::Seq(vec![edge, Code::Br(to)])
        } else {
            Code::Seq(vec![edge, self.do_tree(to, context)])
        }
    }
}

/// The structured form of `cfg`; every node reachable from the entry appears once (a merge node or loop header
/// appears as the code after its block, the rest at their only forward edge).
pub fn structure(cfg: &Cfg) -> Result<Code, Error> {
    let analysis = analyse(cfg)?;
    let _ = &analysis.idom;
    Ok(simplify(analysis.do_tree(0, &mut Vec::new())))
}

/// Drop the branches out of a block that leave it by falling off its end anyway: a `Br` at the end of its body, of an
/// arm of an `if` or `switch` that ends it, or of a nested block's body. A block nothing else leaves then needs no
/// construct of its own when lowered. Behaviour is unchanged.
fn simplify(code: Code) -> Code {
    fn tail(code: &mut Code, label: Label) {
        match code {
            Code::Br(l) if *l == label => *code = Code::Seq(Vec::new()),
            Code::Seq(items) => {
                if let Some(last) = items.last_mut() {
                    tail(last, label);
                }
            }
            Code::If(_, a, b) => {
                tail(a, label);
                tail(b, label);
            }
            Code::Switch(_, arms) => arms.iter_mut().for_each(|a| tail(a, label)),
            Code::Block(m, body) if *m != label => tail(body, label),
            _ => {}
        }
    }
    match code {
        Code::Seq(items) => Code::Seq(items.into_iter().map(simplify).collect()),
        Code::Block(label, body) => {
            let mut body = simplify(*body);
            tail(&mut body, label);
            Code::Block(label, Box::new(body))
        }
        Code::Loop(label, body) => Code::Loop(label, Box::new(simplify(*body))),
        Code::If(n, a, b) => Code::If(n, Box::new(simplify(*a)), Box::new(simplify(*b))),
        Code::Switch(n, arms) => Code::Switch(n, arms.into_iter().map(simplify).collect()),
        other => other,
    }
}

// ---------------------------------------------------------------- lowering

/// What the lowering asks of the lifter that owns the nodes' contents.
pub trait Emit {
    type Error;
    /// The node's own statements, into the builder's open block.
    fn node(&mut self, b: &mut Builder, node: usize) -> Result<(), Self::Error>;
    /// The copies of the edge `from -> to`.
    fn edge(&mut self, b: &mut Builder, from: usize, to: usize) -> Result<(), Self::Error>;
    /// The bool of the node's two-way branch.
    fn condition(&mut self, b: &mut Builder, node: usize) -> Result<Handle<Expression>, Self::Error>;
    /// The node's multi-way branch: the selector and the value of each target but the last (the default).
    fn selector(
        &mut self,
        b: &mut Builder,
        node: usize,
    ) -> Result<(Handle<Expression>, Vec<SwitchValue>), Self::Error>;
    /// The node leaves the program: its return.
    fn exit(&mut self, b: &mut Builder, node: usize) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Action {
    Break(Label),
    Continue(Label),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind_ {
    Block(Label),
    Loop(Label),
    Switch,
}

struct Entry {
    kind: Kind_,
    /// The actions routed out through this construct, to be carried on after it.
    pending: Vec<Action>,
}

struct Lowering<'e, E: Emit> {
    emit: &'e mut E,
    jump: Handle<LocalVariable>,
    ids: BTreeMap<Action, u32>,
    stack: Vec<Entry>,
    used: std::collections::BTreeSet<Label>,
}

fn uses(code: &Code, out: &mut std::collections::BTreeSet<Label>) {
    match code {
        Code::Seq(v) => v.iter().for_each(|c| uses(c, out)),
        Code::Block(_, c) | Code::Loop(_, c) => uses(c, out),
        Code::If(_, a, b) => {
            uses(a, out);
            uses(b, out);
        }
        Code::Switch(_, v) => v.iter().for_each(|c| uses(c, out)),
        Code::Br(l) => {
            out.insert(*l);
        }
        Code::Node(_) | Code::Continue(_) | Code::Edge(..) | Code::Exit(_) => {}
    }
}

impl<E: Emit> Lowering<'_, E> {
    fn id(&mut self, action: Action) -> u32 {
        let next = self.ids.len() as u32 + 1;
        *self.ids.entry(action).or_insert(next)
    }

    /// A construct's body, in a block of its own.
    fn block(&mut self, b: &mut Builder, code: &Code) -> Result<Block, E::Error> {
        b.blocks.push(Block::new());
        let result = self.code(b, code);
        let block = b.blocks.pop().expect("the block just pushed");
        result.map(|()| block)
    }

    /// Close the construct on top of the stack, and carry on what was routed through it.
    fn leave(&mut self, b: &mut Builder) -> Result<(), E::Error> {
        let entry = self.stack.pop().expect("an open construct");
        for action in entry.pending {
            let id = self.id(action);
            let jump = b.local_ptr(self.jump);
            let current = b.load(jump, Kind::U, 1);
            let want = b.lit_u(id);
            let hit = b.binary(BinaryOperator::Equal, current, want);
            b.blocks.push(Block::new());
            let jump = b.local_ptr(self.jump);
            let zero = b.lit_u(0);
            b.store(jump, zero);
            self.action(b, action);
            let accept = b.blocks.pop().expect("the arm just pushed");
            b.push(Statement::If {
                condition: hit.expr,
                accept,
                reject: Block::new(),
            });
        }
        Ok(())
    }

    fn action(&mut self, b: &mut Builder, action: Action) {
        let destination = self
            .stack
            .iter()
            .rposition(|e| match (action, e.kind) {
                (Action::Break(l), Kind_::Block(m)) => l == m,
                (Action::Continue(l), Kind_::Loop(m)) => l == m,
                _ => false,
            })
            .expect("a label's construct encloses its branches");
        let top = self.stack.len() - 1;
        let direct = match action {
            Action::Break(_) => destination == top,
            // `continue` leaves any switches and restarts the innermost loop-like construct
            Action::Continue(_) => self.stack[destination + 1..]
                .iter()
                .all(|e| e.kind == Kind_::Switch),
        };
        if direct {
            b.push(match action {
                Action::Break(_) => Statement::Break,
                Action::Continue(_) => Statement::Continue,
            });
        } else {
            let id = self.id(action);
            let jump = b.local_ptr(self.jump);
            let value = b.lit_u(id);
            b.store(jump, value);
            b.push(Statement::Break);
            let pending = &mut self.stack[top].pending;
            if !pending.contains(&action) {
                pending.push(action);
            }
        }
    }

    fn code(&mut self, b: &mut Builder, code: &Code) -> Result<(), E::Error> {
        match code {
            Code::Node(n) => self.emit.node(b, *n)?,
            Code::Seq(items) => {
                for item in items {
                    self.code(b, item)?;
                }
            }
            Code::Edge(from, to) => self.emit.edge(b, *from, *to)?,
            Code::Exit(n) => self.emit.exit(b, *n)?,
            Code::Br(l) => self.action(b, Action::Break(*l)),
            Code::Continue(l) => self.action(b, Action::Continue(*l)),
            Code::Block(label, body) => {
                if self.used.contains(label) {
                    self.stack.push(Entry {
                        kind: Kind_::Block(*label),
                        pending: Vec::new(),
                    });
                    let mut inner = self.block(b, body)?;
                    inner.push(Statement::Break, Span::UNDEFINED);
                    b.push(Statement::Loop {
                        body: inner,
                        continuing: Block::new(),
                        break_if: None,
                    });
                    self.leave(b)?;
                } else {
                    self.code(b, body)?;
                }
            }
            Code::Loop(label, body) => {
                self.stack.push(Entry {
                    kind: Kind_::Loop(*label),
                    pending: Vec::new(),
                });
                let mut inner = self.block(b, body)?;
                inner.push(Statement::Break, Span::UNDEFINED);
                b.push(Statement::Loop {
                    body: inner,
                    continuing: Block::new(),
                    break_if: None,
                });
                self.leave(b)?;
            }
            Code::If(node, then, otherwise) => {
                let condition = self.emit.condition(b, *node)?;
                let accept = self.block(b, then)?;
                let reject = self.block(b, otherwise)?;
                b.push(Statement::If {
                    condition,
                    accept,
                    reject,
                });
            }
            Code::Switch(node, targets) => {
                let (selector, values) = self.emit.selector(b, *node)?;
                self.stack.push(Entry {
                    kind: Kind_::Switch,
                    pending: Vec::new(),
                });
                let mut cases = Vec::new();
                for (i, target) in targets.iter().enumerate() {
                    let body = self.block(b, target)?;
                    cases.push(SwitchCase {
                        value: values.get(i).copied().unwrap_or(SwitchValue::Default),
                        body,
                        fall_through: false,
                    });
                }
                b.push(Statement::Switch { selector, cases });
                self.leave(b)?;
            }
        }
        Ok(())
    }
}

/// Write `code` into the builder's open block through `emit`. `jump` is a `u32` local (zero at the start) the
/// lowering uses to carry a branch across the constructs it leaves.
pub fn lower<E: Emit>(
    b: &mut Builder,
    code: &Code,
    emit: &mut E,
    jump: Handle<LocalVariable>,
) -> Result<(), E::Error> {
    let mut used = std::collections::BTreeSet::new();
    uses(code, &mut used);
    let mut lowering = Lowering {
        emit,
        jump,
        ids: BTreeMap::new(),
        stack: Vec::new(),
        used,
    };
    lowering.code(b, code)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// What a walk of a graph or of its structured form visits: nodes and edges in order.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Step {
        Node(usize),
        Edge(usize, usize),
    }

    /// A deterministic choice for the `visit`-th time node `n` decides, among `ways`.
    fn decide(seed: u64, n: usize, visit: usize, ways: usize) -> usize {
        let mut x = seed ^ ((n as u64) << 20) ^ ((visit as u64) << 40) ^ 0x9e37_79b9_7f4a_7c15;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        x ^= x >> 33;
        (x % ways as u64) as usize
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Flow {
        Next,
        Br(Label),
        Continue(Label),
        Exit,
    }

    /// The structured code run the way naga's constructs would: a block is left by its `Br`, a loop restarted by its
    /// `Continue` and left by falling off its end.
    fn run(code: &Code, seed: u64, visits: &mut [usize], trace: &mut Vec<Step>, limit: usize) -> Flow {
        if trace.len() >= limit {
            return Flow::Exit;
        }
        match code {
            Code::Node(n) => {
                trace.push(Step::Node(*n));
                Flow::Next
            }
            Code::Edge(a, b) => {
                trace.push(Step::Edge(*a, *b));
                Flow::Next
            }
            Code::Seq(items) => {
                for item in items {
                    match run(item, seed, visits, trace, limit) {
                        Flow::Next => {}
                        other => return other,
                    }
                }
                Flow::Next
            }
            Code::Block(l, body) => match run(body, seed, visits, trace, limit) {
                Flow::Br(m) if m == *l => Flow::Next,
                other => other,
            },
            Code::Loop(l, body) => loop {
                match run(body, seed, visits, trace, limit) {
                    Flow::Continue(m) if m == *l => {
                        if trace.len() >= limit {
                            return Flow::Exit;
                        }
                    }
                    other => return other,
                }
            },
            Code::If(n, a, b) => {
                let way = decide(seed, *n, visits[*n], 2);
                visits[*n] += 1;
                run([a, b][way], seed, visits, trace, limit)
            }
            Code::Switch(n, targets) => {
                let way = decide(seed, *n, visits[*n], targets.len());
                visits[*n] += 1;
                run(&targets[way], seed, visits, trace, limit)
            }
            Code::Br(l) => Flow::Br(*l),
            Code::Continue(l) => Flow::Continue(*l),
            Code::Exit(_) => Flow::Exit,
        }
    }

    /// The decisions of straight-line nodes (a `Goto`) are not counted as visits by either walk: only `If` and `Switch`
    /// consult `decide`, and both walks count a visit per consultation.
    fn same_behaviour(cfg: &Cfg, code: &Code) {
        for seed in 0..12u64 {
            let want = {
                // the reference counts a visit for every pass through a node, so count only deciding nodes
                let mut trace = Vec::new();
                let mut visits = vec![0usize; cfg.terms.len()];
                let mut node = 0;
                while trace.len() < 400 {
                    trace.push(Step::Node(node));
                    let next = match &cfg.terms[node] {
                        Term::Exit => break,
                        Term::Goto(t) => *t,
                        Term::If(a, b) => {
                            let w = decide(seed, node, visits[node], 2);
                            visits[node] += 1;
                            [*a, *b][w]
                        }
                        Term::Switch(ts) => {
                            let w = decide(seed, node, visits[node], ts.len());
                            visits[node] += 1;
                            ts[w]
                        }
                    };
                    trace.push(Step::Edge(node, next));
                    node = next;
                }
                trace
            };
            let mut got = Vec::new();
            run(code, seed, &mut vec![0; cfg.terms.len()], &mut got, 400);
            let n = want.len().min(got.len());
            assert_eq!(&want[..n], &got[..n], "seed {seed}: {cfg:?}\n{code:#?}");
            if want.len() < 400 {
                assert_eq!(want.len(), got.len(), "seed {seed}: {cfg:?}\n{code:#?}");
            }
        }
    }

    fn cfg(terms: Vec<Term>) -> Cfg {
        Cfg { terms }
    }

    #[test]
    fn the_shapes_a_shader_has_behave_as_their_graphs() {
        use Term::*;
        let shapes = vec![
            // a straight line, a diamond, a triangle
            cfg(vec![Goto(1), Exit]),
            cfg(vec![If(1, 2), Goto(3), Goto(3), Exit]),
            cfg(vec![If(1, 2), Goto(2), Exit]),
            // a && b ? X : Y: the else reached from two tests, then a join
            cfg(vec![If(1, 3), If(2, 3), Goto(4), Goto(4), Exit]),
            // an early exit in one arm, the other arm continuing
            cfg(vec![If(1, 2), Exit, Goto(3), Exit]),
            // a loop with the test at the top and an exit
            cfg(vec![Goto(1), If(2, 3), Goto(1), Exit]),
            // a loop with a continue, a break and a normal exit
            cfg(vec![
                Goto(1),
                If(2, 6),
                If(3, 4),
                Goto(1),
                If(5, 1),
                Goto(6),
                Exit,
            ]),
            // nested loops where the inner one breaks out of both
            cfg(vec![
                Goto(1),
                If(2, 7),
                Goto(3),
                If(4, 6),
                If(5, 7),
                Goto(3),
                Goto(1),
                Exit,
            ]),
            // a switch with two labels on one target and a default
            cfg(vec![Switch(vec![1, 2, 2, 3]), Goto(4), Goto(4), Goto(4), Exit]),
            // a switch in a loop whose cases continue, break the loop, and join
            cfg(vec![
                Goto(1),
                Switch(vec![2, 3, 4]),
                Goto(1),
                Goto(5),
                Goto(5),
                If(1, 6),
                Exit,
            ]),
            // a chain of ifs sharing an else
            cfg(vec![If(1, 4), If(2, 4), If(3, 4), Goto(5), Goto(5), Exit]),
            // a self loop
            cfg(vec![Goto(1), If(1, 2), Exit]),
        ];
        for cfg in &shapes {
            let code = structure(cfg).unwrap_or_else(|e| panic!("{e}: {cfg:?}"));
            same_behaviour(cfg, &code);
        }
    }

    #[test]
    fn irreducible_flow_is_refused_by_name() {
        use Term::*;
        // two ways into the loop {1, 2}: from 0 straight to 1 and to 2
        let g = cfg(vec![If(1, 2), Goto(2), Goto(1)]);
        let e = structure(&g).unwrap_err();
        assert!(matches!(e, Error::Irreducible { .. }), "{e}");
        assert!(e.to_string().contains("irreducible"));
    }

    /// Random graphs: every one the structurizer accepts behaves as the graph does.
    #[test]
    fn random_graphs_behave_as_themselves() {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let (mut accepted, mut refused) = (0, 0);
        for _ in 0..4000 {
            let n = 2 + next(8);
            let mut terms = Vec::new();
            for i in 0..n {
                terms.push(match next(10) {
                    0 | 1 => Term::Exit,
                    2..=4 => Term::Goto(next(n)),
                    5..=8 => Term::If(next(n), next(n)),
                    _ => Term::Switch((0..2 + next(3)).map(|_| next(n)).collect()),
                });
                let _ = i;
            }
            let g = Cfg { terms };
            match structure(&g) {
                Ok(code) => {
                    accepted += 1;
                    same_behaviour(&g, &code);
                }
                Err(_) => refused += 1,
            }
        }
        assert!(
            accepted > 500 && refused > 0,
            "{accepted} accepted, {refused} refused"
        );
    }
}
