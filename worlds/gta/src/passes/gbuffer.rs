//! What a G-buffer pixel program's output statements say about the lanes the G-buffer's roles name, read from the
//! program lifted by [`primase_dxil::lift`] (the stores into the entry point's output variables `o<n>`), not from
//! the program's name.
//!
//! The G-buffer the `PS_DeferredTextured*` programs write (the layout is checked against the programs by evaluating
//! them, `tests/gbuffer_layout.rs`):
//!
//! | target | lanes | carries |
//! |---|---|---|
//! | 0 | xyz | the diffuse colour (texture times the material's colour, darkened by wetness) |
//! | 0 | w | a byte of the draw's state, from the engine's constants |
//! | 1 | xyz | the world normal biased to 0 to 1 (`n * 0.5 + 0.5`) |
//! | 2 | xyz | the specular intensity, falloff and Fresnel terms |
//! | 3 | x, y | the vertex colour's red and green (ambient terms) |
//! | 3 | z | the emission (the constant 0 where the effect has none) |
//! | 4 | xy | the screen-space motion |
//!
//! A program need not write a lane the way its family usually does: one effect stores target 1 as `v / 256`, and a
//! program that writes no emission stores the constant 0 there. What a lane holds is the program's own, so the two
//! facts the roles depend on are read per program: [`GBufferWrites::normal_biased`] (target 1's x, y and z are each
//! stored as a value plus 0.5) and [`GBufferWrites::emission_written`] (target 3's z is stored as something other
//! than the constant 0).
use naga::{Expression, Function, Literal, Statement};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GBufferWrites {
    /// Target 1's x, y and z are each stored as `value + 0.5`.
    pub normal_biased: bool,
    /// Target 3's z is stored as something other than the constant 0.
    pub emission_written: bool,
}

/// The writes of a DXIL container's pixel program, or why they cannot be read.
pub fn writes(container: &[u8]) -> Result<GBufferWrites, String> {
    let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
    let function = &program
        .module
        .entry_points
        .first()
        .ok_or("a program without an entry point")?
        .function;
    let mut stores = Vec::new();
    collect(function, &function.body, &mut stores);
    // every store to a lane
    let lane = |target: usize, lane: u32| -> Vec<naga::Handle<Expression>> {
        stores
            .iter()
            .filter(|s| s.target == target && s.lane == lane)
            .map(|s| s.value)
            .collect()
    };
    let all_biased = |target: usize, lane_index: u32| {
        let values = lane(target, lane_index);
        !values.is_empty() && values.iter().all(|v| is_biased(function, *v))
    };
    let emission = lane(3, 2);
    Ok(GBufferWrites {
        normal_biased: (0..3).all(|l| all_biased(1, l)),
        emission_written: !emission.is_empty() && emission.iter().any(|v| !is_zero(function, *v)),
    })
}

/// One store of a value into a lane of an output variable.
struct Store {
    target: usize,
    lane: u32,
    value: naga::Handle<Expression>,
}

/// The stores into `o<n>.<lane>` in a block, in every branch of it.
fn collect(function: &Function, block: &naga::Block, out: &mut Vec<Store>) {
    for statement in block {
        match statement {
            Statement::Store { pointer, value } => {
                if let Some((target, lane)) = output_lane(function, *pointer) {
                    out.push(Store {
                        target,
                        lane,
                        value: *value,
                    });
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

/// The output target and lane a pointer expression names: lane `index` of the local variable `o<target>`.
fn output_lane(function: &Function, pointer: naga::Handle<Expression>) -> Option<(usize, u32)> {
    let Expression::AccessIndex { base, index } = function.expressions.try_get(pointer).ok()? else {
        return None;
    };
    let Expression::LocalVariable(variable) = function.expressions.try_get(*base).ok()? else {
        return None;
    };
    let name = function
        .local_variables
        .try_get(*variable)
        .ok()?
        .name
        .as_deref()?;
    let target = name.strip_prefix('o')?.parse().ok()?;
    Some((target, *index))
}

/// The float a stored word is: the value under the bitcast the lifter writes every output word with.
fn stored(function: &Function, value: naga::Handle<Expression>) -> Option<&Expression> {
    match function.expressions.try_get(value).ok()? {
        Expression::As {
            expr, convert: None, ..
        } => function.expressions.try_get(*expr).ok(),
        other => Some(other),
    }
}

/// `x + 0.5` (the literal either side).
fn is_biased(function: &Function, value: naga::Handle<Expression>) -> bool {
    let Some(Expression::Binary {
        op: naga::BinaryOperator::Add,
        left,
        right,
    }) = stored(function, value)
    else {
        return false;
    };
    [left, right].into_iter().any(|side| {
        matches!(
            function.expressions.try_get(*side),
            Ok(Expression::Literal(Literal::F32(half))) if half.to_bits() == 0.5f32.to_bits()
        )
    })
}

/// The constant 0.
fn is_zero(function: &Function, value: naga::Handle<Expression>) -> bool {
    match stored(function, value) {
        Some(Expression::Literal(Literal::F32(v))) => v.to_bits() == 0,
        Some(Expression::Literal(Literal::U32(v))) => *v == 0,
        _ => false,
    }
}
