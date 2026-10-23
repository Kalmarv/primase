#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The structurizer's lowering, end to end: random control flow graphs are structured, written as naga statements
//! (blocks and loops with routed branches), validated, and run by the executor. Every node adds to an accumulator, so
//! the output is a hash of the nodes and edges visited in order; each deciding node picks its way from a hash of its
//! visit count. The output must be the one a walk of the graph itself gives, including for graphs with loops, switches
//! and branches that leave several constructs at once.
use naga::{Expression, Handle, Statement, SwitchValue};
use primase_shader::build::structure::{self, Cfg, Emit, Term};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::exec::ProgramShader;
use primase_shader::{
    Conventions, D3dModes, D3dSignature, Interface, InterpMode, Label, PixelShader, Program, Provenance,
    ScalarType, ShaderFormat, Stage, SystemValue, Value, Varying, VaryingProvenance, entry_io,
};

const LIMIT: u32 = 64;

/// The way node `n` goes the `visit`-th time, of `ways`: a 32-bit hash both the shader and the walk compute.
fn way(seed: u32, n: usize, visit: u32, ways: u32) -> u32 {
    let mut x = seed.wrapping_mul(0x9e37_79b1)
        ^ (n as u32 + 1).wrapping_mul(0x85eb_ca6b)
        ^ (visit + 1).wrapping_mul(0xc2b2_ae35);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297a_2d39);
    x ^= x >> 15;
    x % ways
}

/// The accumulator a walk of the graph ends with.
fn walk(cfg: &Cfg, seed: u32) -> u32 {
    let (mut acc, mut steps, mut node) = (0u32, 0u32, 0usize);
    let mut visits = vec![0u32; cfg.terms.len()];
    loop {
        steps += 1;
        acc = acc.wrapping_mul(31).wrapping_add(node as u32 + 1);
        if steps >= LIMIT {
            return acc;
        }
        let mut decide = |ways: u32| {
            let w = way(seed, node, visits[node], ways);
            visits[node] += 1;
            w as usize
        };
        let next = match &cfg.terms[node] {
            Term::Exit => return acc,
            Term::Goto(t) => *t,
            Term::If(a, b) => [*a, *b][decide(2)],
            Term::Switch(ts) => ts[decide(ts.len() as u32)],
        };
        acc = acc.wrapping_mul(31).wrapping_add(200 + (node * 16 + next) as u32);
        node = next;
    }
}

struct Toy {
    seed: u32,
    acc: Handle<naga::LocalVariable>,
    steps: Handle<naga::LocalVariable>,
    visits: Vec<Handle<naga::LocalVariable>>,
    result: Handle<naga::Type>,
}

impl Toy {
    fn load(&self, b: &mut Builder, local: Handle<naga::LocalVariable>) -> Val {
        let p = b.local_ptr(local);
        b.load(p, Kind::U, 1)
    }
    fn set(&self, b: &mut Builder, local: Handle<naga::LocalVariable>, v: Val) {
        let p = b.local_ptr(local);
        b.store(p, v);
    }
    fn accumulate(&self, b: &mut Builder, word: u32) {
        use naga::BinaryOperator::{Add, Multiply};
        let acc = self.load(b, self.acc);
        let k = b.lit_u(31);
        let times = b.binary(Multiply, acc, k);
        let w = b.lit_u(word);
        let sum = b.binary(Add, times, w);
        self.set(b, self.acc, sum);
    }
    fn return_acc(&self, b: &mut Builder) {
        let acc = self.load(b, self.acc);
        let bits = b.bitcast(acc, Kind::F);
        let zero = b.lit_f(0.0);
        let colour = b.compose(&[bits, zero, zero, zero]);
        let out = b.app(Expression::Compose {
            ty: self.result,
            components: vec![colour.expr],
        });
        b.push(Statement::Return { value: Some(out) });
    }
    /// The hash of the node's visit, advancing the count.
    fn pick(&self, b: &mut Builder, node: usize, ways: u32) -> Val {
        use naga::BinaryOperator::{ExclusiveOr, Modulo, Multiply, ShiftRight};
        let visit = self.load(b, self.visits[node]);
        let one = b.lit_u(1);
        let next = b.binary(naga::BinaryOperator::Add, visit, one);
        self.set(b, self.visits[node], next);
        let seed = b.lit_u(self.seed);
        let k1 = b.lit_u(0x9e37_79b1);
        let a = b.binary(Multiply, seed, k1);
        let n1 = b.lit_u(node as u32 + 1);
        let k2 = b.lit_u(0x85eb_ca6b);
        let c = b.binary(Multiply, n1, k2);
        let v1 = b.binary(naga::BinaryOperator::Add, visit, one);
        let k3 = b.lit_u(0xc2b2_ae35);
        let d = b.binary(Multiply, v1, k3);
        let ac = b.binary(ExclusiveOr, a, c);
        let mut x = b.binary(ExclusiveOr, ac, d);
        for (shift, mul) in [(15, 0x2c1b_3c6d_u32), (12, 0x297a_2d39), (15, 1)] {
            let s = b.lit_u(shift);
            let shifted = b.binary(ShiftRight, x, s);
            x = b.binary(ExclusiveOr, x, shifted);
            if mul != 1 {
                let m = b.lit_u(mul);
                x = b.binary(Multiply, x, m);
            }
        }
        let w = b.lit_u(ways);
        b.binary(Modulo, x, w)
    }
}

impl Emit for Toy {
    type Error = std::convert::Infallible;
    fn node(&mut self, b: &mut Builder, node: usize) -> Result<(), Self::Error> {
        use naga::BinaryOperator::{Add, GreaterEqual};
        let steps = self.load(b, self.steps);
        let one = b.lit_u(1);
        let steps = b.binary(Add, steps, one);
        self.set(b, self.steps, steps);
        self.accumulate(b, node as u32 + 1);
        let limit = b.lit_u(LIMIT);
        let over = b.binary(GreaterEqual, steps, limit);
        b.blocks.push(naga::Block::new());
        self.return_acc(b);
        let accept = b.blocks.pop().unwrap();
        b.push(Statement::If {
            condition: over.expr,
            accept,
            reject: naga::Block::new(),
        });
        Ok(())
    }
    fn edge(&mut self, b: &mut Builder, from: usize, to: usize) -> Result<(), Self::Error> {
        self.accumulate(b, 200 + (from * 16 + to) as u32);
        Ok(())
    }
    fn condition(&mut self, b: &mut Builder, node: usize) -> Result<Handle<Expression>, Self::Error> {
        let w = self.pick(b, node, 2);
        let zero = b.lit_u(0);
        Ok(b.binary(naga::BinaryOperator::Equal, w, zero).expr)
    }
    fn selector(
        &mut self,
        b: &mut Builder,
        node: usize,
    ) -> Result<(Handle<Expression>, Vec<SwitchValue>), Self::Error> {
        // the number of ways is the node's: the toy knows it from its table
        let ways = TOY_WAYS.with(|w| w.borrow()[node]);
        let w = self.pick(b, node, ways);
        Ok((w.expr, (0..ways - 1).map(SwitchValue::U32).collect()))
    }
    fn exit(&mut self, b: &mut Builder, _node: usize) -> Result<(), Self::Error> {
        self.return_acc(b);
        Ok(())
    }
}

thread_local! {
    static TOY_WAYS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn program(cfg: &Cfg, seed: u32) -> Result<Program, structure::Error> {
    let code = structure::structure(cfg)?;
    TOY_WAYS.with(|w| {
        *w.borrow_mut() = cfg
            .terms
            .iter()
            .map(|t| match t {
                Term::Switch(ts) => ts.len() as u32,
                _ => 2,
            })
            .collect();
    });
    let interface = Interface {
        stage: Stage::Pixel,
        inputs: vec![],
        outputs: vec![Varying {
            label: Label {
                name: "SV_Target".into(),
                index: 0,
            },
            system: Some(SystemValue::Target(0)),
            value: Value {
                scalar: ScalarType::Float,
                width: 4,
            },
            interp: None,
            used: true,
            provenance: VaryingProvenance {
                d3d: Some(D3dSignature {
                    register: 0,
                    mask: 15,
                    read_write_mask: 0,
                }),
            },
        }],
        resources: vec![],
    };
    let _ = InterpMode::Flat;
    let mut b = Builder::new();
    let u1 = b.ty_of(Kind::U, 1);
    let acc = b.local("acc", u1);
    let steps = b.local("steps", u1);
    let jump = b.local("jump", u1);
    let visits: Vec<_> = (0..cfg.terms.len())
        .map(|i| b.local(&format!("v{i}"), u1))
        .collect();
    let io = entry_io(&interface).unwrap();
    let declared = b.declare_entry(&io).unwrap();
    let mut toy = Toy {
        seed,
        acc,
        steps,
        visits,
        result: declared.result.unwrap(),
    };
    structure::lower(&mut b, &code, &mut toy, jump).unwrap();
    let module = b.finish(
        "main",
        naga::ShaderStage::Fragment,
        declared.arguments,
        declared.result,
        false,
    );
    Ok(Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface,
        bindings: vec![],
        conventions: Conventions::D3D,
        modes: D3dModes::default(),
        provenance: Provenance {
            format: ShaderFormat::Dxbc,
            target: "toy".into(),
            lifter: "test".into(),
            lifter_version: 0,
            source_hash: None,
        },
        notes: vec![],
    })
}

fn run(program: Program) -> u32 {
    let shader = ProgramShader::new(program).unwrap();
    let execution = shader
        .execute(&[], &[], &mut |_: primase_shader::Sample| Ok([0.0; 4]))
        .unwrap();
    execution.outputs[0][0]
}

#[test]
fn lowered_graphs_run_as_their_walk() {
    use Term::*;
    let shapes = vec![
        Cfg {
            terms: vec![If(1, 3), If(2, 3), Goto(4), Goto(4), Exit],
        },
        Cfg {
            terms: vec![Goto(1), If(2, 6), If(3, 4), Goto(1), If(5, 1), Goto(6), Exit],
        },
        // nested loops where the inner one breaks out of both
        Cfg {
            terms: vec![
                Goto(1),
                If(2, 7),
                Goto(3),
                If(4, 6),
                If(5, 7),
                Goto(3),
                Goto(1),
                Exit,
            ],
        },
        // a switch in a loop: one case continues the loop, one leaves it, one joins
        Cfg {
            terms: vec![
                Goto(1),
                Switch(vec![2, 3, 4]),
                Goto(1),
                Goto(5),
                Goto(5),
                If(1, 6),
                Exit,
            ],
        },
        Cfg {
            terms: vec![Switch(vec![1, 2, 2, 3]), Goto(4), Goto(4), Goto(4), Exit],
        },
    ];
    for cfg in &shapes {
        for seed in 0..8 {
            let p = program(cfg, seed).unwrap();
            assert_eq!(run(p), walk(cfg, seed), "{cfg:?} seed {seed}");
        }
    }
}

#[test]
fn random_lowered_graphs_run_as_their_walk() {
    let mut state = 0x0fed_cba9_8765_4321u64;
    let mut next = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let (mut ran, mut routed) = (0, 0);
    for _ in 0..1500 {
        let n = 2 + next(8);
        let terms: Vec<Term> = (0..n)
            .map(|_| match next(10) {
                0 | 1 => Term::Exit,
                2..=4 => Term::Goto(next(n)),
                5..=8 => Term::If(next(n), next(n)),
                _ => Term::Switch((0..2 + next(3)).map(|_| next(n)).collect()),
            })
            .collect();
        let cfg = Cfg { terms };
        for seed in 0..4 {
            let Ok(p) = program(&cfg, seed) else { break };
            routed += usize::from(primase_shader::emit::wgsl(&p).unwrap().contains("jump"));
            assert_eq!(run(p), walk(&cfg, seed), "{cfg:?} seed {seed}");
            ran += 1;
        }
    }
    assert!(
        ran > 1000 && routed > 100,
        "{ran} runs, {routed} with a routed branch"
    );
}
