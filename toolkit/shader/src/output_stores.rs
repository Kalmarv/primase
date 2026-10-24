//! What a program's entry point stores into its outputs, lane by lane, read from its lifted module: for each output the
//! interface declares, the local variable the entry point's result takes it from, and the stores into that variable's
//! lanes, in every branch. [`OutputStores`] gives a colour target's lanes (`SV_Target<n>`) and the constant rows an
//! output is made of, and tests how a stored value is made (`x * 0.5 + 0.5`, `floor(x) / 256`, a square root, the
//! constant 0), seeing through the bit casts the lifters write every output word with and the loads of a local variable
//! assigned once. Nothing here is read from a program's name or a local's name; what the lanes of a game's G-buffer
//! are is the game's reading (`primase-gta`'s `gbuffer`).
use std::collections::{BTreeMap, BTreeSet};

use naga::{BinaryOperator, Expression, Function, Handle, Literal, MathFunction, Statement};

use crate::{Interface, Label, Program, SystemValue};

/// What a program's entry point stores into its outputs' lanes, and tests of how a stored value is made.
#[derive(Debug)]
pub struct OutputStores<'a> {
    program: &'a Program,
    function: &'a Function,
    /// For each of the interface's outputs, the local variable the entry point's result takes it from.
    variables: Vec<Option<Handle<naga::LocalVariable>>>,
    /// Every store into a lane of a local variable, as (variable, lane, stored value).
    stores: Vec<(Handle<naga::LocalVariable>, u32, Handle<Expression>)>,
    reading: Reading<'a>,
}

impl<'a> OutputStores<'a> {
    /// The stores of `program`'s entry point, or why they cannot be read: an error where the entry point's result is
    /// not composed of its outputs from local variables (the lifted program is not shaped as this reading expects).
    pub fn of(program: &'a Program) -> Result<Self, String> {
        let function = &program
            .module
            .entry_points
            .first()
            .ok_or("a program without an entry point")?
            .function;
        let variables = output_variables(function, &program.interface)?;
        let mut stores = Vec::new();
        collect(function, &function.body, &mut stores);
        Ok(Self {
            program,
            function,
            variables,
            stores,
            reading: Reading::new(function),
        })
    }

    /// The values stored into lane `lane` of colour target `target` (`SV_Target<target>`), in every branch; empty where
    /// the program declares no such target or stores nothing there.
    pub fn lane(&self, target: u32, lane: u32) -> Vec<Handle<Expression>> {
        let Some(at) = self
            .program
            .interface
            .outputs
            .iter()
            .position(|v| v.system == Some(SystemValue::Target(target)))
        else {
            return Vec::new();
        };
        let Some(Some(variable)) = self.variables.get(at) else {
            return Vec::new();
        };
        self.stores
            .iter()
            .filter(|(v, l, _)| v == variable && *l == lane)
            .map(|(_, _, value)| *value)
            .collect()
    }

    /// `x * 0.5 + 0.5` (the literals on either side, `x` a product with a factor 0.5 or the `fma` of it).
    pub fn is_biased(&self, value: Handle<Expression>) -> bool {
        self.reading.is_biased(value)
    }

    /// `floor(x) * (1/256)`.
    pub fn is_floored_byte(&self, value: Handle<Expression>) -> bool {
        self.reading.is_floored_byte(value)
    }

    /// The square root of something.
    pub fn is_sqrt(&self, value: Handle<Expression>) -> bool {
        self.reading.is_sqrt(value)
    }

    /// The constant 0.
    pub fn is_zero(&self, value: Handle<Expression>) -> bool {
        self.reading.is_zero(value)
    }

    /// The constant-buffer rows (by buffer binding, then row) the output labelled `output` is computed from: every
    /// `cb[row]` the stores into the output's variable reach. For checking what an interpolant depends on.
    pub fn constant_rows(&self, output: &Label) -> Result<BTreeMap<u32, BTreeSet<u32>>, String> {
        let at = self
            .program
            .interface
            .outputs
            .iter()
            .position(|v| &v.label == output)
            .ok_or("the program has no such output")?;
        let Some(Some(variable)) = self.variables.get(at) else {
            return Err("the output's variable is not found".into());
        };
        let mut rows: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut work: Vec<Handle<Expression>> = self
            .stores
            .iter()
            .filter(|(v, _, _)| v == variable)
            .map(|(_, _, value)| *value)
            .collect();
        while let Some(handle) = work.pop() {
            if !seen.insert(handle.index()) {
                continue;
            }
            let Ok(e) = self.function.expressions.try_get(handle) else {
                continue;
            };
            if let Expression::AccessIndex { base, index } = e
                && let Ok(Expression::GlobalVariable(g)) = self.function.expressions.try_get(*base)
                && let Some(binding) = self
                    .program
                    .module
                    .global_variables
                    .try_get(*g)
                    .ok()
                    .and_then(|g| g.binding)
            {
                rows.entry(binding.binding).or_default().insert(*index);
            }
            work.extend(operands(e));
        }
        Ok(rows)
    }
}

/// The stores of a block, in every branch of it, as (local variable, lane, stored value).
fn collect(
    function: &Function,
    block: &naga::Block,
    out: &mut Vec<(Handle<naga::LocalVariable>, u32, Handle<Expression>)>,
) {
    for statement in block {
        match statement {
            Statement::Store { pointer, value } => {
                if let Some((variable, lane)) = output_lane(function, *pointer) {
                    out.push((variable, lane, *value));
                }
            }
            Statement::Block(inner) => collect(function, inner, out),
            Statement::If { accept, reject, .. } => {
                collect(function, accept, out);
                collect(function, reject, out);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    collect(function, &case.body, out);
                }
            }
            Statement::Loop { body, continuing, .. } => {
                collect(function, body, out);
                collect(function, continuing, out);
            }
            _ => {}
        }
    }
}

/// The local variable and lane a store's pointer names: lane `index` of a local variable.
fn output_lane(
    function: &Function,
    pointer: Handle<Expression>,
) -> Option<(Handle<naga::LocalVariable>, u32)> {
    let Expression::AccessIndex { base, index } = function.expressions.try_get(pointer).ok()? else {
        return None;
    };
    let Expression::LocalVariable(variable) = function.expressions.try_get(*base).ok()? else {
        return None;
    };
    Some((*variable, *index))
}

/// The local variable a result member's expression reads: through bit casts, swizzles and the load.
fn source_variable(function: &Function, member: Handle<Expression>) -> Option<Handle<naga::LocalVariable>> {
    let mut at = member;
    for _ in 0..16 {
        match function.expressions.try_get(at).ok()? {
            Expression::As { expr, .. } => at = *expr,
            Expression::Swizzle { vector, .. } => at = *vector,
            Expression::Load { pointer } => at = *pointer,
            Expression::LocalVariable(variable) => return Some(*variable),
            _ => return None,
        }
    }
    None
}

/// For each of the interface's outputs, the local variable the entry point's returned structure takes it from.
fn output_variables(
    function: &Function,
    interface: &Interface,
) -> Result<Vec<Option<Handle<naga::LocalVariable>>>, String> {
    /// Every returned value, in every branch.
    fn returns(block: &naga::Block, out: &mut Vec<Handle<Expression>>) {
        for s in block {
            match s {
                Statement::Return { value: Some(v) } => out.push(*v),
                Statement::Block(inner) => returns(inner, out),
                Statement::If { accept, reject, .. } => {
                    returns(accept, out);
                    returns(reject, out);
                }
                Statement::Switch { cases, .. } => cases.iter().for_each(|c| returns(&c.body, out)),
                Statement::Loop { body, continuing, .. } => {
                    returns(body, out);
                    returns(continuing, out);
                }
                _ => {}
            }
        }
    }
    let mut values = Vec::new();
    returns(&function.body, &mut values);
    // each return composes the outputs from the same variables
    let mut result: Option<Vec<Option<Handle<naga::LocalVariable>>>> = None;
    for value in values {
        let Expression::Compose { components, .. } =
            function.expressions.try_get(value).map_err(|e| e.to_string())?
        else {
            return Err("the entry point's result is not composed of its outputs".into());
        };
        if components.len() != interface.outputs.len() {
            return Err(format!(
                "the entry point returns {} members for {} outputs",
                components.len(),
                interface.outputs.len()
            ));
        }
        let here: Vec<_> = components.iter().map(|c| source_variable(function, *c)).collect();
        match &result {
            None => result = Some(here),
            Some(first) if *first == here => {}
            Some(_) => {
                return Err("the entry point's returns take their outputs from different variables".into());
            }
        }
    }
    result.ok_or_else(|| "the entry point returns no value".to_owned())
}

/// A function and what is stored into its local variables as a whole (`v = value`), so that the load of a variable
/// assigned once reads as the value assigned: the programs that compute a G-buffer lane before a conditional discard
/// keep it in a variable.
#[derive(Debug)]
struct Reading<'a> {
    function: &'a Function,
    assigned: BTreeMap<usize, Vec<Handle<Expression>>>,
}

impl<'a> Reading<'a> {
    fn new(function: &'a Function) -> Self {
        fn whole(
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
                    Statement::Block(inner) => whole(function, inner, out),
                    Statement::If { accept, reject, .. } => {
                        whole(function, accept, out);
                        whole(function, reject, out);
                    }
                    Statement::Switch { cases, .. } => {
                        cases.iter().for_each(|c| whole(function, &c.body, out))
                    }
                    Statement::Loop { body, continuing, .. } => {
                        whole(function, body, out);
                        whole(function, continuing, out);
                    }
                    _ => {}
                }
            }
        }
        let mut assigned = BTreeMap::new();
        whole(function, &function.body, &mut assigned);
        Self { function, assigned }
    }

    /// The expression a handle stands for: through the bit casts the lifter writes every output word with and the
    /// loads of a local variable assigned once.
    fn resolve(&self, value: Handle<Expression>) -> Handle<Expression> {
        let mut at = value;
        for _ in 0..16 {
            match self.function.expressions.try_get(at) {
                Ok(Expression::As {
                    expr, convert: None, ..
                }) => at = *expr,
                Ok(Expression::Load { pointer }) => match self.function.expressions.try_get(*pointer) {
                    Ok(Expression::LocalVariable(v)) => {
                        match self.assigned.get(&v.index()).map(Vec::as_slice) {
                            Some([only]) => at = *only,
                            _ => return at,
                        }
                    }
                    _ => return at,
                },
                _ => return at,
            }
        }
        at
    }

    fn expression(&self, value: Handle<Expression>) -> Option<&Expression> {
        self.function.expressions.try_get(self.resolve(value)).ok()
    }

    fn is_literal(&self, value: Handle<Expression>, x: f32) -> bool {
        matches!(
            self.expression(value),
            Some(Expression::Literal(Literal::F32(v))) if v.to_bits() == x.to_bits()
        )
    }

    /// The factors of a product (its `Multiply` tree's leaves).
    fn factors(&self, value: Handle<Expression>, out: &mut Vec<Handle<Expression>>) {
        match self.expression(value) {
            Some(Expression::Binary {
                op: BinaryOperator::Multiply,
                left,
                right,
            }) => {
                self.factors(*left, out);
                self.factors(*right, out);
            }
            _ => out.push(value),
        }
    }

    /// `x * 0.5 + 0.5` (the literals on either side, `x` a product with a factor 0.5 or the `fma` of it).
    fn is_biased(&self, value: Handle<Expression>) -> bool {
        let half_factor = |product: Handle<Expression>| {
            let mut leaves = Vec::new();
            self.factors(product, &mut leaves);
            leaves.iter().any(|f| self.is_literal(*f, 0.5))
        };
        match self.expression(value) {
            Some(Expression::Binary {
                op: BinaryOperator::Add,
                left,
                right,
            }) => [(left, right), (right, left)]
                .into_iter()
                .any(|(bias, product)| self.is_literal(*bias, 0.5) && half_factor(*product)),
            Some(Expression::Math {
                fun: MathFunction::Fma,
                arg,
                arg1: Some(arg1),
                arg2: Some(arg2),
                ..
            }) => {
                self.is_literal(*arg2, 0.5)
                    && (self.is_literal(*arg, 0.5) || self.is_literal(*arg1, 0.5) || half_factor(*arg))
            }
            _ => false,
        }
    }

    /// `floor(x) * (1/256)`.
    fn is_floored_byte(&self, value: Handle<Expression>) -> bool {
        let Some(Expression::Binary {
            op: BinaryOperator::Multiply,
            left,
            right,
        }) = self.expression(value)
        else {
            return false;
        };
        [(left, right), (right, left)].into_iter().any(|(scale, floor)| {
            self.is_literal(*scale, 1.0 / 256.0)
                && matches!(
                    self.expression(*floor),
                    Some(Expression::Math {
                        fun: MathFunction::Floor,
                        ..
                    })
                )
        })
    }

    /// The square root of something.
    fn is_sqrt(&self, value: Handle<Expression>) -> bool {
        matches!(
            self.expression(value),
            Some(Expression::Math {
                fun: MathFunction::Sqrt,
                ..
            })
        )
    }

    /// The constant 0.
    fn is_zero(&self, value: Handle<Expression>) -> bool {
        match self.expression(value) {
            Some(Expression::Literal(Literal::F32(v))) => v.to_bits() == 0,
            Some(Expression::Literal(Literal::U32(v))) => *v == 0,
            _ => false,
        }
    }
}

/// The expressions an expression is computed from.
fn operands(e: &Expression) -> Vec<Handle<Expression>> {
    match e {
        Expression::Binary { left, right, .. } => vec![*left, *right],
        Expression::Unary { expr, .. } | Expression::As { expr, .. } => vec![*expr],
        Expression::Select {
            condition,
            accept,
            reject,
        } => vec![*condition, *accept, *reject],
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => [Some(*arg), *arg1, *arg2, *arg3].into_iter().flatten().collect(),
        Expression::Swizzle { vector, .. } => vec![*vector],
        Expression::AccessIndex { base, .. } => vec![*base],
        Expression::Access { base, index } => vec![*base, *index],
        Expression::Splat { value, .. } => vec![*value],
        Expression::Compose { components, .. } => components.clone(),
        Expression::Load { pointer } => vec![*pointer],
        _ => Vec::new(),
    }
}
