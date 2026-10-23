//! Atomic operations on a pointer to an atomic word, and the guard D3D's bounds rule needs around them.

use naga::{AtomicFunction, Expression, Handle, Scalar, Statement};

use super::{Builder, Kind, Val};

impl Builder {
    /// The kind a word of the atomic integer type `scalar` is read as.
    pub fn atomic_kind(scalar: Scalar) -> Kind {
        if scalar == Scalar::I32 { Kind::I } else { Kind::U }
    }

    /// `fun` applied to the atomic word `pointer` names with `value` (as the word's own type): the old value.
    pub fn atomic(
        &mut self,
        pointer: Handle<Expression>,
        fun: AtomicFunction,
        value: Val,
        scalar: Scalar,
    ) -> Val {
        let kind = Self::atomic_kind(scalar);
        let value = self.bitcast_to(value, kind);
        let ty = self.ty_of(kind, 1);
        let result = self.result_expr(Expression::AtomicResult {
            ty,
            comparison: false,
        });
        self.push(Statement::Atomic {
            pointer,
            fun,
            value: value.expr,
            result: Some(result),
        });
        Val {
            expr: result,
            kind,
            width: 1,
        }
    }

    /// A compare-and-swap on the atomic word `pointer` names: the old value and whether the swap happened.
    pub fn compare_exchange(
        &mut self,
        pointer: Handle<Expression>,
        compare: Val,
        value: Val,
        scalar: Scalar,
    ) -> (Val, Val) {
        let kind = Self::atomic_kind(scalar);
        let compare = self.bitcast_to(compare, kind);
        let value = self.bitcast_to(value, kind);
        let ty = self
            .module
            .generate_predeclared_type(naga::PredeclaredType::AtomicCompareExchangeWeakResult(scalar));
        let result = self.result_expr(Expression::AtomicResult { ty, comparison: true });
        self.push(Statement::Atomic {
            pointer,
            fun: AtomicFunction::Exchange {
                compare: Some(compare.expr),
            },
            value: value.expr,
            result: Some(result),
        });
        let old = Val {
            expr: self.app(Expression::AccessIndex {
                base: result,
                index: 0,
            }),
            kind,
            width: 1,
        };
        let exchanged = Val {
            expr: self.app(Expression::AccessIndex {
                base: result,
                index: 1,
            }),
            kind: Kind::B,
            width: 1,
        };
        (old, exchanged)
    }

    /// The pointer to word `word` of the buffer whose words `words` points to (a typed buffer's element is four words:
    /// its first).
    pub fn buffer_word(&mut self, words: Handle<Expression>, word: Val, typed: bool) -> Handle<Expression> {
        let element = self.app(Expression::Access {
            base: words,
            index: word.expr,
        });
        if typed {
            self.app(Expression::AccessIndex {
                base: element,
                index: 0,
            })
        } else {
            element
        }
    }

    /// An atomic operation `body` makes only where `inside` holds, its old value (of `kind`) zero where it does not:
    /// D3D's answer for an operation past the end of its buffer, which does nothing.
    pub fn atomic_inside(&mut self, inside: Val, kind: Kind, body: impl FnOnce(&mut Self) -> Val) -> Val {
        let ty = self.ty_of(kind, 1);
        let local = self.local("old", ty);
        let zero = self.lit_bits(kind, 0);
        let pointer = self.local_ptr(local);
        self.store(pointer, zero);
        self.when(inside, |b| {
            let old = body(b);
            let old = b.bitcast_to(old, kind);
            let pointer = b.local_ptr(local);
            b.store(pointer, old);
        });
        let pointer = self.local_ptr(local);
        self.load(pointer, kind, 1)
    }
}
