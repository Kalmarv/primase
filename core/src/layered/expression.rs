//! [`Expression`]: a weight channel as a small expression over sources, for what the named [`Channel`] forms do not
//! spell, and a layer's colour lane over the lanes of its own colour ([`Layer::color`](super::Layer::color)).
//!
//! An expression is a list of [`Term`]s, each a constant, a source or one f32 operation on earlier terms; the last is
//! the channel's value. Shared values are one term, so a value used twice is computed once.
//!
//! # Numeric rules
//!
//! Every term is an f32 and every operation is rounded to f32 on its own: nothing is fused (no multiply-add) or
//! reassociated, so a program making the same operations in the same order makes the same bits. These rules are the
//! named forms' too ([`Channel::evaluate`]):
//!
//! - `add`, `subtract`, `multiply`: IEEE 754 binary32, rounding to nearest even.
//! - `min`, `max`: a NaN operand gives the other operand (IEEE 754's minNum and maxNum).
//! - `saturate`: [`saturate`], NaN gives 0, anything else is clamped to 0..=1.
//! - Lerp, power and product are spelled in these operations: [`lerp`] is `a + (b - a) * t` (the difference, then
//!   the product, then the sum); [`power`](super::power) squares for a power of two and otherwise multiplies `v` in
//!   from the left; a product multiplies from the left.
//! - The two zeros are one value: a constant `-0` is `0`, and no operation here tells them apart.
//!
//! # One spelling
//!
//! An expression is stored in its canonical spelling ([`Expression::canonical`]): every term and source used, equal
//! terms and equal sources one, the operands of `add`, `multiply`, `min` and `max` (which give the same bits in either
//! order) in a stated order, and the terms in the order a walk from the value first reaches them. Two expressions that
//! make the same operations on the same sources are then the same expression. An expression that makes exactly a named
//! form's operations ([`Channel::expression`]) is that form ([`Channel::from_expression`]), so a consumer that maps
//! the named forms keeps receiving them. The named forms are not canonical among themselves: a product of `a` and `b`
//! is also the product of `b` and `a`.
use std::cmp::Ordering;
use std::collections::HashMap;

use serde::Serialize;

use super::{Channel, POWER_LIMIT, PRODUCT_LIMIT, Source};

/// The most terms an expression has, in its canonical spelling.
pub const TERM_LIMIT: usize = 64;

/// The most terms a spelling may have before it is made canonical (its operands index them as bytes).
const RAW_LIMIT: usize = u8::MAX as usize;

/// `saturate(x)`: 0 for NaN, else `x` clamped to 0..=1.
pub fn saturate(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

/// `lerp(a, b, t)`: `a + (b - a) * t`, in that order.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// One value of an [`Expression`]: a constant, a source, or one operation on earlier terms (by their index).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Term {
    /// A finite constant.
    Constant(f32),
    /// The value of [`Expression::sources`]' source at this index.
    Source(u8),
    Add([u8; 2]),
    /// The first minus the second.
    Subtract([u8; 2]),
    Multiply([u8; 2]),
    Min([u8; 2]),
    Max([u8; 2]),
    /// [`saturate`].
    Saturate(u8),
}

impl Term {
    /// The terms it reads, in its order.
    pub fn operands(&self) -> &[u8] {
        match self {
            Term::Constant(_) | Term::Source(_) => &[],
            Term::Add(o) | Term::Subtract(o) | Term::Multiply(o) | Term::Min(o) | Term::Max(o) => o,
            Term::Saturate(o) => std::slice::from_ref(o),
        }
    }

    /// Whether its operands give the same bits in either order.
    fn commutative(&self) -> bool {
        matches!(
            self,
            Term::Add(_) | Term::Multiply(_) | Term::Min(_) | Term::Max(_)
        )
    }

    /// Its kind's place in the canonical order.
    fn rank(&self) -> u8 {
        match self {
            Term::Constant(_) => 0,
            Term::Source(_) => 1,
            Term::Add(_) => 2,
            Term::Subtract(_) => 3,
            Term::Multiply(_) => 4,
            Term::Min(_) => 5,
            Term::Max(_) => 6,
            Term::Saturate(_) => 7,
        }
    }

    /// The same operation on other terms (`operands` as many as it reads).
    fn with_operands(&self, operands: &[u8]) -> Term {
        let two = || [operands[0], operands[1]];
        match self {
            Term::Constant(_) | Term::Source(_) => *self,
            Term::Add(_) => Term::Add(two()),
            Term::Subtract(_) => Term::Subtract(two()),
            Term::Multiply(_) => Term::Multiply(two()),
            Term::Min(_) => Term::Min(two()),
            Term::Max(_) => Term::Max(two()),
            Term::Saturate(_) => Term::Saturate(operands[0]),
        }
    }

    /// Whether it is the same term as `other` with commutative operands in either order.
    fn same(&self, other: &Term) -> bool {
        let sorted = |t: &Term| {
            let mut o = t.operands().to_vec();
            if t.commutative() {
                o.sort_unstable();
            }
            t.with_operands(&o)
        };
        match (self, other) {
            (Term::Constant(a), Term::Constant(b)) => a.to_bits() == b.to_bits(),
            _ => sorted(self) == sorted(other),
        }
    }
}

/// A value per pixel as f32 operations on its sources (see the module): a weight channel's over [`Source`]s, a layer's
/// colour lane over the lanes of its own colour ([`ColorLane`](super::ColorLane)).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Expression<S = Source> {
    /// The sources its [`Term::Source`] terms read, each once.
    pub sources: Vec<S>,
    /// Its terms, each after the terms it reads; the last is the value.
    pub terms: Vec<Term>,
}

impl<S: Clone + PartialEq + Serialize> Expression<S> {
    /// Its value from its sources' values (in [`Expression::sources`] order), under the module's numeric rules.
    pub fn evaluate(&self, values: &[f32]) -> f32 {
        let mut v = [0.0f32; TERM_LIMIT];
        for (i, term) in self.terms.iter().enumerate().take(TERM_LIMIT) {
            let at = |k: u8| v[usize::from(k)];
            v[i] = match *term {
                Term::Constant(x) => x,
                Term::Source(s) => values[usize::from(s)],
                Term::Add([a, b]) => at(a) + at(b),
                Term::Subtract([a, b]) => at(a) - at(b),
                Term::Multiply([a, b]) => at(a) * at(b),
                Term::Min([a, b]) => at(a).min(at(b)),
                Term::Max([a, b]) => at(a).max(at(b)),
                Term::Saturate(a) => saturate(at(a)),
            };
        }
        self.terms
            .len()
            .checked_sub(1)
            .and_then(|last| v.get(last))
            .copied()
            .unwrap_or(f32::NAN)
    }

    /// Its canonical spelling (see the module), or why it has none: a term reading itself or a later term, a source
    /// that is not there, a constant that is not finite, no terms, or more than [`TERM_LIMIT`] once canonical.
    pub fn canonical(&self) -> Result<Expression<S>, String> {
        let root = self
            .terms
            .len()
            .checked_sub(1)
            .ok_or("an expression without terms")?;
        canonical(&self.sources, &self.terms, root)
    }

    /// Its own checks (its sources' apart): it is its canonical spelling. That it is no named form is
    /// [`Channel::check`]'s.
    pub fn check(&self) -> Result<(), String> {
        let canonical = self.canonical()?;
        if canonical != *self {
            return Err(format!(
                "expression is not canonical: {:?} is spelled {:?}",
                self.terms, canonical.terms
            ));
        }
        Ok(())
    }
}

/// A term of an [`ExpressionBuilder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermId(usize);

/// Spells an expression term by term, in any order and with any repetition; [`ExpressionBuilder::expression`] makes
/// it canonical, and [`ExpressionBuilder::channel`] the named form it is.
#[derive(Clone, Debug)]
pub struct ExpressionBuilder<S = Source> {
    sources: Vec<S>,
    terms: Vec<Term>,
    /// Past [`RAW_LIMIT`] terms: the spelling is refused.
    overflow: bool,
}

impl<S> Default for ExpressionBuilder<S> {
    fn default() -> Self {
        ExpressionBuilder {
            sources: Vec::new(),
            terms: Vec::new(),
            overflow: false,
        }
    }
}

impl<S: Clone + PartialEq + Serialize> ExpressionBuilder<S> {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, t: Term) -> TermId {
        if self.terms.len() >= RAW_LIMIT {
            self.overflow = true;
            return TermId(0);
        }
        self.terms.push(t);
        TermId(self.terms.len() - 1)
    }

    fn index(&mut self, v: TermId) -> u8 {
        u8::try_from(v.0).unwrap_or_else(|_| {
            self.overflow = true;
            0
        })
    }

    pub fn source(&mut self, s: S) -> TermId {
        self.sources.push(s);
        let i = self.sources.len() - 1;
        let i = u8::try_from(i).unwrap_or_else(|_| {
            self.overflow = true;
            0
        });
        self.push(Term::Source(i))
    }

    pub fn constant(&mut self, x: f32) -> TermId {
        self.push(Term::Constant(x))
    }

    fn binary(&mut self, a: TermId, b: TermId, make: fn([u8; 2]) -> Term) -> TermId {
        let o = [self.index(a), self.index(b)];
        self.push(make(o))
    }

    pub fn add(&mut self, a: TermId, b: TermId) -> TermId {
        self.binary(a, b, Term::Add)
    }

    /// `a - b`.
    pub fn subtract(&mut self, a: TermId, b: TermId) -> TermId {
        self.binary(a, b, Term::Subtract)
    }

    pub fn multiply(&mut self, a: TermId, b: TermId) -> TermId {
        self.binary(a, b, Term::Multiply)
    }

    pub fn min(&mut self, a: TermId, b: TermId) -> TermId {
        self.binary(a, b, Term::Min)
    }

    pub fn max(&mut self, a: TermId, b: TermId) -> TermId {
        self.binary(a, b, Term::Max)
    }

    pub fn saturate(&mut self, a: TermId) -> TermId {
        let o = self.index(a);
        self.push(Term::Saturate(o))
    }

    /// [`lerp`]'s operations.
    pub fn lerp(&mut self, a: TermId, b: TermId, t: TermId) -> TermId {
        let d = self.subtract(b, a);
        let s = self.multiply(d, t);
        self.add(a, s)
    }

    /// [`power`](super::power)'s operations; an exponent of 0 is the constant 1.
    pub fn power(&mut self, v: TermId, exponent: u8) -> TermId {
        if exponent == 0 {
            return self.constant(1.0);
        }
        if exponent.is_power_of_two() {
            (0..exponent.trailing_zeros()).fold(v, |x, _| self.multiply(x, x))
        } else {
            (1..exponent).fold(v, |x, _| self.multiply(x, v))
        }
    }

    /// The canonical expression whose value is `root`.
    pub fn expression(&self, root: TermId) -> Result<Expression<S>, String> {
        if self.overflow {
            return Err(format!(
                "an expression spelled in more than {RAW_LIMIT} terms or sources"
            ));
        }
        if root.0 >= self.terms.len() {
            return Err(format!("term {} is not in the expression", root.0));
        }
        canonical(&self.sources, &self.terms, root.0)
    }
}

impl ExpressionBuilder {
    /// The channel whose value is `root`: the named form it is, else the canonical expression.
    pub fn channel(&self, root: TermId) -> Result<Channel, String> {
        self.expression(root).map(Channel::from_expression_canonical)
    }
}

/// That every operand reads an earlier term and every source term a source, and every constant is finite.
fn well_formed<S>(sources: &[S], terms: &[Term], root: usize) -> Result<(), String> {
    if terms.len() > RAW_LIMIT || sources.len() > RAW_LIMIT {
        return Err(format!(
            "an expression spelled in more than {RAW_LIMIT} terms or sources"
        ));
    }
    for (i, t) in terms.iter().enumerate().take(root + 1) {
        if let Some(&o) = t.operands().iter().find(|&&o| usize::from(o) >= i) {
            return Err(format!("term {i} reads term {o}, which is not before it"));
        }
        match *t {
            Term::Source(s) if usize::from(s) >= sources.len() => {
                return Err(format!("term {i} reads source {s} of {}", sources.len()));
            }
            Term::Constant(x) if !x.is_finite() => {
                return Err(format!("term {i}: constant {x} is not finite"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The canonical spelling of term `root` of `terms` over `sources` (see the module).
fn canonical<S: Clone + PartialEq + Serialize>(
    sources: &[S],
    terms: &[Term],
    root: usize,
) -> Result<Expression<S>, String> {
    well_formed(sources, terms, root)?;
    let mut needed = vec![false; root + 1];
    needed[root] = true;
    for i in (0..=root).rev() {
        if needed[i] {
            for &o in terms[i].operands() {
                needed[usize::from(o)] = true;
            }
        }
    }
    // equal terms (over equal classes, commutative operands in either order) are one class; equal sources one source
    let mut distinct: Vec<&S> = Vec::new();
    let mut classes: Vec<Term> = Vec::new();
    let mut class_of = vec![0u8; root + 1];
    for i in (0..=root).filter(|&i| needed[i]) {
        let operands: Vec<u8> = terms[i]
            .operands()
            .iter()
            .map(|&o| class_of[usize::from(o)])
            .collect();
        let term = match terms[i] {
            Term::Source(s) => {
                let s = &sources[usize::from(s)];
                let d = distinct.iter().position(|x| *x == s).unwrap_or_else(|| {
                    distinct.push(s);
                    distinct.len() - 1
                });
                Term::Source(d as u8)
            }
            // the two zeros are one value
            Term::Constant(x) => Term::Constant(if x == 0.0 { 0.0 } else { x }),
            t => t.with_operands(&operands),
        };
        let class = classes.iter().position(|c| c.same(&term)).unwrap_or_else(|| {
            classes.push(term);
            classes.len() - 1
        });
        class_of[i] = class as u8;
    }
    let keys = distinct
        .iter()
        .map(|s| serde_json::to_string(s).map_err(|e| format!("a source without a key: {e}")))
        .collect::<Result<Vec<String>, String>>()?;
    let mut order = Order {
        classes: &classes,
        keys: &keys,
        memo: HashMap::new(),
    };
    let mut emit = Emit {
        index: vec![None; classes.len()],
        source_index: vec![None; distinct.len()],
        sources: Vec::new(),
        terms: Vec::new(),
    };
    emit.class(&mut order, &distinct, usize::from(class_of[root]));
    if emit.terms.len() > TERM_LIMIT {
        return Err(format!(
            "an expression of {} terms, past {TERM_LIMIT}",
            emit.terms.len()
        ));
    }
    Ok(Expression {
        sources: emit.sources,
        terms: emit.terms,
    })
}

/// The canonical order of classes: by kind, then constants by value, sources by their JSON, operations by their
/// operands in their canonical order. Two classes are equal in it only when they are one class.
struct Order<'a> {
    classes: &'a [Term],
    keys: &'a [String],
    memo: HashMap<(usize, usize), Ordering>,
}

impl Order<'_> {
    fn operands(&mut self, c: usize) -> Vec<usize> {
        let t = self.classes[c];
        let mut o: Vec<usize> = t.operands().iter().map(|&o| usize::from(o)).collect();
        if t.commutative() && self.cmp(o[1], o[0]) == Ordering::Less {
            o.swap(0, 1);
        }
        o
    }

    fn cmp(&mut self, a: usize, b: usize) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        if let Some(&o) = self.memo.get(&(a, b)) {
            return o;
        }
        let (x, y) = (self.classes[a], self.classes[b]);
        let o = match (x, y) {
            _ if x.rank() != y.rank() => x.rank().cmp(&y.rank()),
            (Term::Constant(p), Term::Constant(q)) => p.total_cmp(&q),
            (Term::Source(p), Term::Source(q)) => self.keys[usize::from(p)].cmp(&self.keys[usize::from(q)]),
            _ => {
                let (p, q) = (self.operands(a), self.operands(b));
                p.iter()
                    .zip(&q)
                    .map(|(&i, &j)| self.cmp(i, j))
                    .find(|o| o.is_ne())
                    .unwrap_or(Ordering::Equal)
            }
        };
        self.memo.insert((a, b), o);
        o
    }
}

/// The canonical terms, emitted by a walk from the value: operands first, in their canonical order.
struct Emit<S> {
    index: Vec<Option<u8>>,
    source_index: Vec<Option<u8>>,
    sources: Vec<S>,
    terms: Vec<Term>,
}

impl<S: Clone> Emit<S> {
    fn class(&mut self, order: &mut Order, distinct: &[&S], c: usize) -> u8 {
        if let Some(i) = self.index[c] {
            return i;
        }
        let operands: Vec<u8> = order
            .operands(c)
            .into_iter()
            .map(|o| self.class(order, distinct, o))
            .collect();
        let term = match order.classes[c] {
            Term::Source(s) => {
                let s = usize::from(s);
                let i = *self.source_index[s].get_or_insert_with(|| {
                    self.sources.push(distinct[s].clone());
                    (self.sources.len() - 1) as u8
                });
                Term::Source(i)
            }
            t => t.with_operands(&operands),
        };
        self.terms.push(term);
        let i = (self.terms.len() - 1) as u8;
        self.index[c] = Some(i);
        i
    }
}

impl Channel {
    /// Its operations as an [`Expression`], in the canonical spelling: what it means under the numeric rules.
    pub fn expression(&self) -> Result<Expression, String> {
        self.check()?;
        let mut b = ExpressionBuilder::new();
        let root = match self {
            Channel::Source(s) => b.source(s.clone()),
            Channel::Lerp { a, b: to, t } => {
                let (a, to, t) = (b.source(a.clone()), b.source(to.clone()), b.source(t.clone()));
                b.lerp(a, to, t)
            }
            Channel::Threshold { v, m, k } => {
                // ((k0 + k1 v) + k2 m) + (k3 v) m, as [`Channel::evaluate`] writes it
                let (v, m) = (b.source(v.clone()), b.source(m.clone()));
                let [k0, k1, k2, k3] = k.map(|x| b.constant(x));
                let k1v = b.multiply(k1, v);
                let x = b.add(k0, k1v);
                let k2m = b.multiply(k2, m);
                let x = b.add(x, k2m);
                let k3v = b.multiply(k3, v);
                let k3vm = b.multiply(k3v, m);
                let x = b.add(x, k3vm);
                b.saturate(x)
            }
            Channel::Product { sources } => {
                let mut values = sources
                    .iter()
                    .map(|s| b.source(s.clone()))
                    .collect::<Vec<_>>()
                    .into_iter();
                let first = values.next().ok_or("a product of no sources")?;
                values.fold(first, |p, v| b.multiply(p, v))
            }
            Channel::Power { v, exponent } => {
                let v = b.source(v.clone());
                b.power(v, *exponent)
            }
            Channel::Expression(e) => return Ok(e.clone()),
        };
        b.expression(root)
    }

    /// The channel `e` spells: the named form whose operations it makes, else `e` in its canonical spelling.
    pub fn from_expression(e: &Expression) -> Result<Channel, String> {
        e.canonical().map(Self::from_expression_canonical)
    }

    /// [`Channel::from_expression`] of a canonical expression.
    fn from_expression_canonical(e: Expression) -> Channel {
        named(&e).unwrap_or(Channel::Expression(e))
    }
}

/// The named form whose canonical expression is `e`, trying source, power, product, lerp and threshold in that order,
/// each with its sources and constants taken from `e` in their order there.
fn named(e: &Expression) -> Option<Channel> {
    let constants: Vec<f32> = e
        .terms
        .iter()
        .filter_map(|t| match t {
            Term::Constant(x) => Some(*x),
            _ => None,
        })
        .collect();
    let s = &e.sources;
    let mut candidates: Vec<Channel> = Vec::new();
    match e.terms.last()? {
        Term::Source(_) => candidates.push(Channel::Source(s.first()?.clone())),
        Term::Multiply(_) if constants.is_empty() => {
            if s.len() == 1 {
                candidates.extend((2..=POWER_LIMIT).map(|exponent| Channel::Power {
                    v: s[0].clone(),
                    exponent,
                }));
            }
            if s.len() <= PRODUCT_LIMIT {
                for n in 2..=PRODUCT_LIMIT {
                    candidates.extend(sequences(s, n).map(|sources| Channel::Product { sources }));
                }
            }
        }
        Term::Add(_) if constants.is_empty() && s.len() <= 3 => {
            candidates.extend(sequences(s, 3).map(|mut abt| {
                let (t, b, a) = (abt.remove(2), abt.remove(1), abt.remove(0));
                Channel::Lerp { a, b, t }
            }));
        }
        Term::Saturate(_) if s.len() <= 2 && constants.len() <= 4 && e.terms.len() <= 14 => {
            let ks = (0..constants.len().pow(4)).map(|mut n| {
                std::array::from_fn(|_| {
                    let k = constants[n % constants.len()];
                    n /= constants.len();
                    k
                })
            });
            let ks: Vec<[f32; 4]> = ks.collect();
            for vm in sequences(s, 2) {
                candidates.extend(ks.iter().map(|&k| Channel::Threshold {
                    v: vm[0].clone(),
                    m: vm[1].clone(),
                    k,
                }));
            }
        }
        _ => {}
    }
    candidates
        .into_iter()
        .find(|c| c.expression().is_ok_and(|x| x == *e))
}

/// Every sequence of `n` of `sources` (repeats allowed) that uses each, in lexicographic order of their indices.
fn sequences(sources: &[Source], n: usize) -> impl Iterator<Item = Vec<Source>> + '_ {
    let k = sources.len();
    let count = if k == 0 { 0 } else { k.pow(n as u32) };
    (0..count).filter_map(move |mut x| {
        let mut picks = vec![0; n];
        for p in picks.iter_mut().rev() {
            *p = x % k;
            x /= k;
        }
        (0..k)
            .all(|s| picks.contains(&s))
            .then(|| picks.iter().map(|&p| sources[p].clone()).collect())
    })
}
