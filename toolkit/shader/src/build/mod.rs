//! A typed builder over naga's IR for lifters. Every value it hands out carries its scalar kind and width, so a lifter
//! never asks naga's typifier and a bitcast or splat is explicit where it is needed. Nothing here knows a bytecode;
//! [`behaviour`] holds D3D's results spelled out as operations.

mod atomic;
pub mod behaviour;
mod entry;
mod mesh;
mod operations;
pub mod structure;

pub use entry::{EntryDecl, FunctionDecl};
pub use mesh::MeshOutput;

use naga::{
    BinaryOperator, Block, Expression, Function, GlobalVariable, Handle, Literal, LocalVariable,
    MathFunction, Module, Scalar, ScalarKind, Span, Statement, SwizzleComponent, Type, TypeInner,
    UnaryOperator, VectorSize,
};

/// The scalar kind of a typed value: unsigned, signed, float or bool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    U,
    I,
    F,
    B,
}
impl Kind {
    pub fn scalar(self) -> Scalar {
        match self {
            Kind::U => Scalar::U32,
            Kind::I => Scalar::I32,
            Kind::F => Scalar::F32,
            Kind::B => Scalar::BOOL,
        }
    }
    pub fn kind(self) -> ScalarKind {
        self.scalar().kind
    }
}

/// An expression with its kind and width (1 for a scalar, 2 to 4 for a vector).
#[derive(Clone, Copy, Debug)]
pub struct Val {
    pub expr: Handle<Expression>,
    pub kind: Kind,
    pub width: u8,
}

pub fn vector_size(width: u8) -> VectorSize {
    match width {
        2 => VectorSize::Bi,
        3 => VectorSize::Tri,
        4 => VectorSize::Quad,
        _ => panic!("vector width {width} (a lifter bug: widths are 2 to 4)"),
    }
}

pub fn component(lane: u8) -> SwizzleComponent {
    match lane {
        0 => SwizzleComponent::X,
        1 => SwizzleComponent::Y,
        2 => SwizzleComponent::Z,
        _ => SwizzleComponent::W,
    }
}

/// A module and the entry function being built, with a stack of the blocks open for structured control flow.
///
/// What a lifter reads decides the shapes of the values it hands the builder, so an operation asked of values that cannot
/// do it (a vector of 2 lanes added to one of 3) is not a bug of the builder and does not panic: it records the first such
/// fault and returns a value of the shape asked for. The contract is the lifter's: it calls [`Builder::check`] where it
/// can name what it was reading (after each instruction or statement) and before it finishes the module, and turns a fault
/// into a refusal. `finish`, `finish_mesh`, `finish_functions` and `end_function` panic on a fault nobody checked, which a
/// lifter that follows the contract cannot reach from its input; that panic, and the one on control flow left open, are
/// for a lifter's own bugs.
#[derive(Debug)]
pub struct Builder {
    pub module: Module,
    pub function: Function,
    /// The blocks being filled, innermost last; statements go to the last.
    pub blocks: Vec<Block>,
    /// A compute entry point's thread group size (set before [`Builder::finish`]; 0 for the other stages).
    pub workgroup_size: [u32; 3],
    /// The first operation a lifter asked for that the values it was given cannot be (a vector of 2 lanes added to one
    /// of 3, a lane that is not there): the lifter's input is at fault, not the builder, so the operation hands back a
    /// value of the shape it was asked for and the fault waits here until the lifter asks ([`Builder::check`]).
    error: Option<String>,
}
impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
impl Builder {
    pub fn new() -> Self {
        Self {
            module: Module::default(),
            function: Function::default(),
            blocks: vec![Block::new()],
            workgroup_size: [0; 3],
            error: None,
        }
    }

    /// Record that an operation was asked of values that cannot do it; the first is kept.
    fn fail(&mut self, detail: impl Into<String>) {
        self.error.get_or_insert_with(|| detail.into());
    }

    /// The first fault of the operations since the last check, which a lifter turns into a refusal of the program at the
    /// place it is reading. A builder with a fault unchecked cannot be finished: that is the lifter's bug.
    pub fn check(&mut self) -> Result<(), String> {
        self.error.take().map_or(Ok(()), Err)
    }

    fn assert_checked(&self) {
        assert!(
            self.error.is_none(),
            "a lifter finished a builder without checking it: {}",
            self.error.as_deref().unwrap_or_default()
        );
    }

    pub fn ty(&mut self, inner: TypeInner) -> Handle<Type> {
        self.module
            .types
            .insert(Type { name: None, inner }, Span::UNDEFINED)
    }
    pub fn ty_of(&mut self, kind: Kind, width: u8) -> Handle<Type> {
        if width == 1 {
            self.ty(TypeInner::Scalar(kind.scalar()))
        } else {
            self.ty(TypeInner::Vector {
                size: vector_size(width),
                scalar: kind.scalar(),
            })
        }
    }

    /// Append an expression to the function; one that is not a pre-emitted kind (a literal, an argument, a pointer)
    /// is emitted in the current block.
    pub fn app(&mut self, e: Expression) -> Handle<Expression> {
        let pre = e.needs_pre_emit();
        let start = self.function.expressions.len();
        let h = self.function.expressions.append(e, Span::UNDEFINED);
        if !pre {
            let range = self.function.expressions.range_from(start);
            self.push(Statement::Emit(range));
        }
        h
    }
    /// The type of an element of a typed buffer the program reaches with atomic operations: four atomic words.
    pub fn atomic_quad(&mut self, scalar: naga::Scalar) -> Handle<Type> {
        let atomic = self.ty(TypeInner::Atomic(scalar));
        let members = (0..4u32)
            .map(|k| naga::StructMember {
                name: Some(format!("w{k}")),
                ty: atomic,
                binding: None,
                offset: 4 * k,
            })
            .collect();
        self.ty(TypeInner::Struct { members, span: 16 })
    }
    /// A result expression (an atomic's old value): made before the statement that fills it, covered by no emit.
    pub fn result_expr(&mut self, e: Expression) -> Handle<Expression> {
        self.function.expressions.append(e, Span::UNDEFINED)
    }
    /// `if condition { then }`: what `then` appends goes in the accept block.
    pub fn when(&mut self, condition: Val, then: impl FnOnce(&mut Self)) {
        self.blocks.push(Block::new());
        then(self);
        let accept = self.blocks.pop().expect("the block just pushed");
        self.push(Statement::If {
            condition: condition.expr,
            accept,
            reject: Block::new(),
        });
    }
    /// Open `for i in 0..count { .. }` as a loop (not unrolled): what is appended until [`Self::end_repeat`] is its
    /// body, and the value returned is `i`, read at the top of each pass.
    pub fn begin_repeat(&mut self, count: u32) -> (Val, Handle<LocalVariable>) {
        let ty = self.ty_of(Kind::U, 1);
        let counter = self.local("i", ty);
        let ptr = self.local_ptr(counter);
        let zero = self.lit_u(0);
        self.store(ptr, zero);
        self.blocks.push(Block::new());
        let i = self.load(ptr, Kind::U, 1);
        let limit = self.lit_u(count);
        let done = self.binary(BinaryOperator::GreaterEqual, i, limit);
        self.when(done, |b| b.push(Statement::Break));
        (i, counter)
    }
    /// Close the loop [`Self::begin_repeat`] opened: count the pass and go round.
    pub fn end_repeat(&mut self, counter: Handle<LocalVariable>) {
        let ptr = self.local_ptr(counter);
        let again = self.load(ptr, Kind::U, 1);
        let one = self.lit_u(1);
        let next = self.binary(BinaryOperator::Add, again, one);
        self.store(ptr, next);
        let inner = self.blocks.pop().expect("the loop body opened by begin_repeat");
        self.push(Statement::Loop {
            body: inner,
            continuing: Block::new(),
            break_if: None,
        });
    }
    pub fn push(&mut self, s: Statement) {
        self.blocks
            .last_mut()
            .expect("a block is always open")
            .push(s, Span::UNDEFINED);
    }
    pub fn local(&mut self, name: &str, ty: Handle<Type>) -> Handle<LocalVariable> {
        self.function.local_variables.append(
            LocalVariable {
                name: Some(name.to_owned()),
                ty,
                init: None,
            },
            Span::UNDEFINED,
        )
    }
    pub fn local_ptr(&mut self, l: Handle<LocalVariable>) -> Handle<Expression> {
        self.app(Expression::LocalVariable(l))
    }
    pub fn global_ptr(&mut self, g: Handle<GlobalVariable>) -> Handle<Expression> {
        self.app(Expression::GlobalVariable(g))
    }
    pub fn load(&mut self, ptr: Handle<Expression>, kind: Kind, width: u8) -> Val {
        Val {
            expr: self.app(Expression::Load { pointer: ptr }),
            kind,
            width,
        }
    }
    pub fn store(&mut self, ptr: Handle<Expression>, v: Val) {
        self.push(Statement::Store {
            pointer: ptr,
            value: v.expr,
        });
    }
    pub fn global(&mut self, g: GlobalVariable) -> Handle<GlobalVariable> {
        self.module.global_variables.append(g, Span::UNDEFINED)
    }
    /// An expression of the module's constant arena (an initialiser).
    pub fn global_expr(&mut self, e: Expression) -> Handle<Expression> {
        self.module.global_expressions.append(e, Span::UNDEFINED)
    }

    // literals
    pub fn lit_u(&mut self, x: u32) -> Val {
        self.lit(Literal::U32(x), Kind::U)
    }
    pub fn lit_i(&mut self, x: i32) -> Val {
        self.lit(Literal::I32(x), Kind::I)
    }
    /// A float literal. naga holds only finite ones, so a NaN or an infinity is the bits of one, reinterpreted.
    pub fn lit_f(&mut self, x: f32) -> Val {
        if x.is_finite() {
            return self.lit(Literal::F32(x), Kind::F);
        }
        let bits = self.lit_u(x.to_bits());
        self.bitcast(bits, Kind::F)
    }
    pub fn lit_b(&mut self, x: bool) -> Val {
        self.lit(Literal::Bool(x), Kind::B)
    }
    fn lit(&mut self, l: Literal, kind: Kind) -> Val {
        Val {
            expr: self.app(Expression::Literal(l)),
            kind,
            width: 1,
        }
    }
    /// The literal of `kind` whose 32 bits are `bits` (a bool is whether they are nonzero).
    pub fn lit_bits(&mut self, kind: Kind, bits: u32) -> Val {
        match kind {
            Kind::U => self.lit_u(bits),
            Kind::I => self.lit_i(bits as i32),
            Kind::F => self.lit_f(f32::from_bits(bits)),
            Kind::B => self.lit_b(bits != 0),
        }
    }

    pub fn splat(&mut self, v: Val, width: u8) -> Val {
        if v.width == width {
            return v;
        }
        if v.width != 1 || !(2..=4).contains(&width) {
            self.fail(format!("a value of {} lanes where {width} are needed", v.width));
            return v;
        }
        Val {
            expr: self.app(Expression::Splat {
                size: vector_size(width),
                value: v.expr,
            }),
            kind: v.kind,
            width,
        }
    }
    /// A vector (or the scalar itself, for one part) from scalars of one kind.
    pub fn compose(&mut self, parts: &[Val]) -> Val {
        if let [only] = parts {
            return *only;
        }
        let Some(first) = parts.first() else {
            self.fail("a vector of no lanes");
            return self.lit_u(0);
        };
        if parts.len() > 4 || parts.iter().any(|p| p.width != 1 || p.kind != first.kind) {
            self.fail(format!(
                "a vector of {} parts that are not scalars of one kind",
                parts.len()
            ));
            return *first;
        }
        let kind = first.kind;
        let width = parts.len() as u8;
        let ty = self.ty_of(kind, width);
        Val {
            expr: self.app(Expression::Compose {
                ty,
                components: parts.iter().map(|p| p.expr).collect(),
            }),
            kind,
            width,
        }
    }
    /// Lane `i` of a vector.
    pub fn lane(&mut self, v: Val, i: u8) -> Val {
        if i >= v.width {
            self.fail(format!("lane {i} of a value of {} lanes", v.width));
            return v;
        }
        if v.width == 1 {
            return v;
        }
        Val {
            expr: self.app(Expression::AccessIndex {
                base: v.expr,
                index: u32::from(i),
            }),
            kind: v.kind,
            width: 1,
        }
    }
    /// Lanes `lanes` of a vector, in that order, repeats allowed; a scalar is splat.
    pub fn swizzle(&mut self, v: Val, lanes: &[u8]) -> Val {
        if let [one] = lanes {
            return self.lane(v, *one);
        }
        if lanes.is_empty() || lanes.len() > 4 || lanes.iter().any(|l| *l >= v.width) {
            self.fail(format!("lanes {lanes:?} of a value of {} lanes", v.width));
            return v;
        }
        if v.width == 1 {
            return self.splat(v, lanes.len() as u8);
        }
        let mut pattern = [SwizzleComponent::X; 4];
        for (slot, lane) in pattern.iter_mut().zip(lanes) {
            *slot = component(*lane);
        }
        Val {
            expr: self.app(Expression::Swizzle {
                size: vector_size(lanes.len() as u8),
                vector: v.expr,
                pattern,
            }),
            kind: v.kind,
            width: lanes.len() as u8,
        }
    }
    /// Reinterpret the bits as another kind (u32, i32 or f32), width unchanged.
    pub fn bitcast(&mut self, v: Val, kind: Kind) -> Val {
        if v.kind == kind {
            return v;
        }
        Val {
            expr: self.app(Expression::As {
                expr: v.expr,
                kind: kind.kind(),
                convert: None,
            }),
            kind,
            width: v.width,
        }
    }
    /// The value as `kind`: a bitcast between u32, i32 and f32; a bool becomes a word of all ones or zero (D3D's
    /// true and false) and a word a bool by being nonzero.
    pub fn bitcast_to(&mut self, v: Val, kind: Kind) -> Val {
        match (v.kind, kind) {
            (a, b) if a == b => v,
            (Kind::B, _) => {
                let ones = self.lit_u(u32::MAX);
                let zero = self.lit_u(0);
                let word = self.select(v, ones, zero);
                self.bitcast(word, kind)
            }
            (_, Kind::B) => {
                let word = self.bitcast(v, Kind::U);
                let zero = self.lit_u(0);
                self.binary(BinaryOperator::NotEqual, word, zero)
            }
            _ => self.bitcast(v, kind),
        }
    }
    /// Convert the value to another kind (`itof`, `utof`, float to integer) with naga's own semantics; D3D's differ
    /// for float to integer and are [`behaviour`].
    pub fn convert(&mut self, v: Val, kind: Kind) -> Val {
        Val {
            expr: self.app(Expression::As {
                expr: v.expr,
                kind: kind.kind(),
                convert: Some(4),
            }),
            kind,
            width: v.width,
        }
    }
    pub fn unary(&mut self, op: UnaryOperator, v: Val) -> Val {
        Val {
            expr: self.app(Expression::Unary { op, expr: v.expr }),
            kind: v.kind,
            width: v.width,
        }
    }
    /// A binary operation; a scalar operand is splat to the other's width, a comparison gives a bool.
    pub fn binary(&mut self, op: BinaryOperator, a: Val, b: Val) -> Val {
        let width = a.width.max(b.width);
        let (a, b) = (self.splat(a, width), self.splat(b, width));
        let kind = match op {
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => Kind::B,
            _ => a.kind,
        };
        Val {
            expr: self.app(Expression::Binary {
                op,
                left: a.expr,
                right: b.expr,
            }),
            kind,
            width,
        }
    }
    pub fn math(&mut self, fun: MathFunction, args: &[Val]) -> Val {
        let Some(width) = args.iter().map(|a| a.width).max() else {
            self.fail("a math function of no arguments");
            return self.lit_u(0);
        };
        let args: Vec<Val> = args.iter().map(|a| self.splat(*a, width)).collect();
        let kind = args[0].kind;
        let arg = |i: usize| args.get(i).map(|a| a.expr);
        let (width, kind) = match fun {
            MathFunction::Dot => (1, kind),
            MathFunction::Pack2x16float => (1, Kind::U),
            MathFunction::Unpack2x16float => (2, Kind::F),
            _ => (width, kind),
        };
        Val {
            expr: self.app(Expression::Math {
                fun,
                arg: args[0].expr,
                arg1: arg(1),
                arg2: arg(2),
                arg3: arg(3),
            }),
            kind,
            width,
        }
    }
    /// `select(reject, accept, cond)`: a scalar condition is splat to the operands' width.
    pub fn select(&mut self, cond: Val, accept: Val, reject: Val) -> Val {
        let width = accept.width.max(reject.width).max(cond.width);
        let (accept, reject) = (self.splat(accept, width), self.splat(reject, width));
        let cond = self.splat(cond, width);
        Val {
            expr: self.app(Expression::Select {
                condition: cond.expr,
                accept: accept.expr,
                reject: reject.expr,
            }),
            kind: accept.kind,
            width,
        }
    }
    /// A comparison result as D3D's mask: all ones where true, else zero.
    pub fn mask_of(&mut self, cond: Val) -> Val {
        let ones = self.lit_u(u32::MAX);
        let zero = self.lit_u(0);
        self.select(cond, ones, zero)
    }

    /// Finish the module: the function built so far becomes the one entry point `name` of `stage`, with these
    /// arguments (each bound or a structure with bound members) and result structure. All control flow must be
    /// closed.
    /// Close the function being built as the module's function `name` with these arguments and result, and start an
    /// empty one: a program of a stage with no entry point is several of them (see [`crate::stage_functions`]).
    pub fn end_function(&mut self, name: &str, decl: FunctionDecl) -> Handle<naga::Function> {
        assert_eq!(self.blocks.len(), 1, "unclosed control flow (a lifter bug)");
        self.assert_checked();
        let mut function = std::mem::take(&mut self.function);
        function.name = Some(name.to_owned());
        function.arguments = decl.arguments;
        function.result = decl.result.map(|ty| naga::FunctionResult { ty, binding: None });
        function.body = std::mem::replace(&mut self.blocks[0], Block::new());
        self.module.functions.append(function, Span::UNDEFINED)
    }

    /// Declare a function with no body, the module's name for an operation the pipeline does (see [`crate::rt`]); one
    /// that returns a value returns `false`.
    pub fn declare_operation(
        &mut self,
        name: &str,
        arguments: Vec<naga::FunctionArgument>,
        returns_bool: bool,
    ) -> Handle<naga::Function> {
        let mut function = Function {
            name: Some(name.to_owned()),
            arguments,
            ..Function::default()
        };
        if returns_bool {
            let ty = self.ty_of(Kind::B, 1);
            function.result = Some(naga::FunctionResult { ty, binding: None });
            let value = function
                .expressions
                .append(Expression::Literal(naga::Literal::Bool(false)), Span::UNDEFINED);
            function
                .body
                .push(Statement::Return { value: Some(value) }, Span::UNDEFINED);
        }
        self.module.functions.append(function, Span::UNDEFINED)
    }

    /// Finish a module that has functions and no entry point.
    pub fn finish_functions(self) -> Module {
        assert_eq!(self.blocks.len(), 1, "unclosed control flow (a lifter bug)");
        self.assert_checked();
        self.module
    }

    pub fn finish(
        mut self,
        name: &str,
        stage: naga::ShaderStage,
        arguments: Vec<naga::FunctionArgument>,
        result: Option<Handle<Type>>,
        early_depth_test: bool,
    ) -> Module {
        assert_eq!(self.blocks.len(), 1, "unclosed control flow (a lifter bug)");
        self.assert_checked();
        let mut function = self.function;
        function.name = Some(name.to_owned());
        function.arguments = arguments;
        function.result = result.map(|ty| naga::FunctionResult { ty, binding: None });
        function.body = self.blocks.pop().expect("one block");
        self.module.entry_points.push(naga::EntryPoint {
            name: name.to_owned(),
            stage,
            early_depth_test: early_depth_test.then_some(naga::EarlyDepthTest::Force),
            workgroup_size: self.workgroup_size,
            workgroup_size_overrides: None,
            function,
            mesh_info: None,
            task_payload: None,
            incoming_ray_payload: None,
        });
        self.module
    }
}
