//! A property of an effect file as a core value ([`primase_core::particles::Value`]): its curve, its loop and its random
//! variation. The file's four keyframed functions become core's four curve kinds; the Hermite function, which core has no
//! kind for (its shape is the game's own and not a cubic in the keys), is sampled into a linear curve. Everything lowered
//! with a loss or an approximation is counted in [`Notes`].

use fromsoft_formats::fxr::{Function, ModifierKind, Property};
use primase_core::particles::{Argument, Curve, Input, Interpolation, Key, Value, Variation};
use std::collections::BTreeMap;

/// What a lowering did that its source or core did not say, by reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// Things the core description cannot carry, with how many times.
    pub lost: BTreeMap<String, u32>,
    /// Things carried approximately (a Hermite sampled into a line), with how many times.
    pub approximated: BTreeMap<String, u32>,
    /// What was made, by kind (`draw of action 603`), so that placed and lost add up to what the file holds.
    pub placed: BTreeMap<String, u32>,
}

impl Notes {
    /// Counts one loss.
    pub fn lose(&mut self, reason: impl Into<String>) {
        *self.lost.entry(reason.into()).or_default() += 1;
    }

    /// Counts `n` losses of one reason.
    pub fn lose_n(&mut self, reason: impl Into<String>, n: u32) {
        if n > 0 {
            *self.lost.entry(reason.into()).or_default() += n;
        }
    }

    /// Counts one thing made.
    pub fn place(&mut self, what: impl Into<String>) {
        *self.placed.entry(what.into()).or_default() += 1;
    }

    /// Counts one approximation.
    pub fn approximate(&mut self, reason: impl Into<String>) {
        *self.approximated.entry(reason.into()).or_default() += 1;
    }

    /// Adds another's counts.
    pub fn absorb(&mut self, other: &Notes) {
        for (k, n) in &other.lost {
            *self.lost.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &other.approximated {
            *self.approximated.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &other.placed {
            *self.placed.entry(k.clone()).or_default() += n;
        }
    }
}

/// The external values a system's properties depend on, by the file's index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    /// The system's inputs, in order of first use.
    pub list: Vec<Input>,
    index: BTreeMap<i32, u32>,
}

impl Inputs {
    /// The input for external value `n`, made on first use (named after the file's index; unset it is 0).
    pub fn input(&mut self, n: i32) -> u32 {
        if let Some(&i) = self.index.get(&n) {
            return i;
        }
        let i = u32::try_from(self.list.len()).unwrap_or(u32::MAX);
        self.list.push(Input { name: format!("external value {n}"), default: 0.0 });
        self.index.insert(n, i);
        i
    }
}

/// How finely a Hermite segment is cut, at most; the cut is refined until the line is within `HERMITE_TOLERANCE` of the
/// function (as a fraction of the segment's change) at the middle of every piece.
const HERMITE_MAX_PIECES: usize = 64;
const HERMITE_TOLERANCE: f32 = 2e-3;

fn pad(v: &[f32]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (slot, x) in out.iter_mut().zip(v) {
        *slot = *x;
    }
    out
}

fn key(at: f32, value: &[f32]) -> Key {
    Key { at, value: pad(value), handle_in: [0.0; 4], handle_out: [0.0; 4] }
}

/// The base curve of a property.
pub fn curve(p: &Property, notes: &mut Notes) -> Curve {
    let width = u8::try_from(p.components()).unwrap_or(4).clamp(1, 4);
    if let Some(c) = p.constant() {
        return Curve::constant(&c);
    }
    let Some(k) = p.keyframes() else { return Curve::constant(&vec![0.0; usize::from(width)]) };
    let mut keys: Vec<Key> = k.positions.iter().zip(&k.values).map(|(&at, v)| key(at, v)).collect();
    let interpolation = match p.function {
        Function::Stepped => Interpolation::Step,
        Function::Linear => Interpolation::Linear,
        Function::Bezier => {
            if let Some(t) = &k.tangents {
                for (i, key) in keys.iter_mut().enumerate() {
                    key.handle_out = pad(&t.first.get(i).map(|v| v.iter().map(|x| x / 3.0).collect::<Vec<_>>()).unwrap_or_default());
                    key.handle_in = pad(&t.second.get(i).map(|v| v.iter().map(|x| x / 3.0).collect::<Vec<_>>()).unwrap_or_default());
                }
            }
            Interpolation::Bezier
        }
        Function::Hermite => {
            keys = sample_hermite(p, &k.positions, width, notes);
            Interpolation::Linear
        }
        _ => {
            notes.lose("a property function core has no curve for");
            Interpolation::Linear
        }
    };
    // a looping function whose argument is cut at its last key
    Curve { width, interpolation, looped: p.looped, keys }
}

/// A Hermite property as linear keys: each segment cut into as many pieces as it takes for the line to stay within the
/// tolerance of the function at the middle of every piece.
fn sample_hermite(p: &Property, positions: &[f32], width: u8, notes: &mut Notes) -> Vec<Key> {
    let eval = |x: f32| p.value_at(x);
    let mut out: Vec<Key> = Vec::new();
    for pair in positions.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let v0 = eval(a);
        let v1 = eval(b);
        let range = v0
            .iter()
            .zip(&v1)
            .map(|(x, y)| (y - x).abs())
            .fold(0.0f32, f32::max)
            .max(1e-6);
        let mut pieces = 4usize;
        while pieces < HERMITE_MAX_PIECES {
            let step = (b - a) / pieces as f32;
            let worst = (0..pieces)
                .map(|i| {
                    let (x0, x1) = (a + step * i as f32, a + step * (i + 1) as f32);
                    let mid = eval((x0 + x1) * 0.5);
                    let line: Vec<f32> = eval(x0).iter().zip(eval(x1)).map(|(p, q)| (p + q) * 0.5).collect();
                    mid.iter().zip(&line).map(|(m, l)| (m - l).abs()).fold(0.0f32, f32::max)
                })
                .fold(0.0f32, f32::max);
            if worst <= HERMITE_TOLERANCE * range {
                break;
            }
            pieces *= 2;
        }
        let step = (b - a) / pieces as f32;
        if out.is_empty() {
            out.push(key(a, &v0));
        }
        for i in 1..=pieces {
            let x = if i == pieces { b } else { a + step * i as f32 };
            out.push(key(x, &eval(x)));
        }
    }
    if out.is_empty() {
        out.push(key(positions.first().copied().unwrap_or(0.0), &eval(positions.first().copied().unwrap_or(0.0))));
    }
    notes.approximate("a Hermite property sampled into a linear curve");
    let _ = width;
    out
}

/// The modifiers of a property as core variations.
fn variations(p: &Property, inputs: &mut Inputs, notes: &mut Notes) -> Vec<Variation> {
    let mut out = Vec::new();
    for m in &p.modifiers {
        let kind = match m.kind() {
            Ok(k) => k,
            Err(_) => {
                notes.lose("a property modifier that does not read");
                continue;
            }
        };
        let stream = |seeds: &[i32]| seeds.first().map_or(0, |&s| s as u32);
        match kind {
            ModifierKind::RandomDelta { seeds, max } => out.push(Variation::Delta { max: pad(&max), stream: stream(&seeds) }),
            ModifierKind::RandomRange { seeds, min, max } => {
                out.push(Variation::Range { min: pad(&min), max: pad(&max), stream: stream(&seeds) });
            }
            ModifierKind::RandomFraction { seeds, max } => {
                out.push(Variation::Fraction { max: pad(&max), stream: stream(&seeds) });
            }
            ModifierKind::ExternalValue1 { external_value, factor } | ModifierKind::ExternalValue2 { external_value, factor } => {
                let input = inputs.input(external_value);
                out.push(Variation::Scaled { input, factor: curve(&factor, notes) });
            }
        }
    }
    out
}

/// A property as a core value that is a function of `argument`.
pub fn value(p: &Property, argument: Argument, inputs: &mut Inputs, notes: &mut Notes) -> Value {
    let variation = variations(p, inputs, notes);
    Value { argument, curve: curve(p, notes), variation }
}

/// The first component of a property read at argument 0: what a scalar parameter that is not a function of time stands
/// for (a constant, or the first value of a function).
pub fn number(p: &Property) -> f32 {
    p.value_at(0.0).first().copied().filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// The first component of a value at argument 0.
pub fn number_of(v: &Value) -> f32 {
    v.curve.value_at(0.0)[0]
}

/// A texture, model or animation number a property names: a positive whole number.
pub fn id(p: &Property) -> Option<i32> {
    let n = number(p);
    (n >= 1.0 && n < 2_147_483_000.0).then(|| n.round() as i32)
}

/// A whole number of an integer-valued property (a count, a limit): the value at argument 0 rounded; `None` for a negative
/// one (the files' "no limit" is -1).
pub fn count(p: &Property) -> Option<u32> {
    let n = number(p);
    (n >= 0.0 && n < 4_294_967_000.0).then(|| n.round() as u32)
}

/// `v` with every key value and handle multiplied by `k` (a unit change, a sign), and its variations kept meaning the same
/// change of the scaled quantity.
pub fn scaled(mut v: Value, k: f32) -> Value {
    for key in &mut v.curve.keys {
        for c in 0..4 {
            key.value[c] *= k;
            key.handle_in[c] *= k;
            key.handle_out[c] *= k;
        }
    }
    for var in &mut v.variation {
        match var {
            // a delta is a magnitude and a fraction is relative to the base: neither changes with the sign
            Variation::Delta { max, .. } => {
                for m in max.iter_mut() {
                    *m *= k.abs();
                }
            }
            Variation::Fraction { .. } | Variation::Scaled { .. } => {}
            Variation::Range { min, max, .. } => {
                let (lo, hi) = (min.map(|x| x * k), max.map(|x| x * k));
                for c in 0..4 {
                    min[c] = lo[c].min(hi[c]);
                    max[c] = lo[c].max(hi[c]);
                }
            }
        }
    }
    v
}

/// `-v`.
pub fn negated(v: Value) -> Value {
    scaled(v, -1.0)
}

/// `v + c` for every component of every key.
pub fn shifted(mut v: Value, c: f32) -> Value {
    let width = usize::from(v.curve.width.clamp(1, 4));
    for key in &mut v.curve.keys {
        for slot in key.value.iter_mut().take(width) {
            *slot += c;
        }
    }
    v
}

/// The value of a fixed `v` (a constant without variation): its first key's components.
fn fixed(v: &Value) -> Option<[f32; 4]> {
    v.is_fixed().then(|| v.curve.value_at(0.0))
}

/// `a` and `b` combined component by component with `f`, where either is a constant exactly (a key-wise change), else sampled
/// into a linear curve at the keys of both and four pieces between them, counted. `a`'s variations are kept, `b`'s are dropped
/// (counted) when `b` varies.
fn combine(a: Value, b: Value, f: fn(f32, f32) -> f32, what: &str, notes: &mut Notes) -> Value {
    if let Some(k) = fixed(&b) {
        let mut out = a;
        for key in &mut out.curve.keys {
            for c in 0..4 {
                key.value[c] = f(key.value[c], k[c.min(usize::from(b.curve.width.clamp(1, 4)) - 1)]);
            }
        }
        // handles of a combined bezier are only right for a product by a constant; counted otherwise
        if out.curve.interpolation == Interpolation::Bezier {
            notes.approximate(format!("a {what} of a Bezier property and a constant, handles kept"));
        }
        return out;
    }
    if let Some(k) = fixed(&a) {
        let mut out = b;
        for key in &mut out.curve.keys {
            for c in 0..4 {
                key.value[c] = f(k[c.min(usize::from(a.curve.width.clamp(1, 4)) - 1)], key.value[c]);
            }
        }
        if out.curve.interpolation == Interpolation::Bezier {
            notes.approximate(format!("a {what} of a Bezier property and a constant, handles kept"));
        }
        return out;
    }
    if a.argument != b.argument {
        notes.approximate(format!("a {what} of properties read at different arguments, taken at the first's"));
    }
    let mut times: Vec<f32> = a.curve.keys.iter().chain(&b.curve.keys).map(|k| k.at).collect();
    times.sort_by(f32::total_cmp);
    times.dedup();
    let width = a.curve.width.max(b.curve.width).clamp(1, 4);
    let sample = |t: f32| -> [f32; 4] {
        let (x, y) = (a.curve.value_at(t), b.curve.value_at(t));
        let mut out = [0.0; 4];
        for (c, slot) in out.iter_mut().enumerate().take(usize::from(width)) {
            *slot = f(x[c.min(usize::from(a.curve.width.clamp(1, 4)) - 1)], y[c.min(usize::from(b.curve.width.clamp(1, 4)) - 1)]);
        }
        out
    };
    let mut keys = Vec::new();
    for (i, &t0) in times.iter().enumerate() {
        keys.push(Key { at: t0, value: sample(t0), handle_in: [0.0; 4], handle_out: [0.0; 4] });
        if let Some(&t1) = times.get(i + 1) {
            for q in 1..4 {
                let t = t0 + (t1 - t0) * q as f32 / 4.0;
                keys.push(Key { at: t, value: sample(t), handle_in: [0.0; 4], handle_out: [0.0; 4] });
            }
        }
    }
    if !b.variation.is_empty() {
        notes.approximate(format!("a {what} of two varying properties, the second's random variation dropped"));
    }
    notes.approximate(format!("a {what} of two varying properties sampled into a linear curve"));
    Value {
        argument: a.argument,
        curve: Curve { width, interpolation: Interpolation::Linear, looped: false, keys },
        variation: a.variation,
    }
}

/// `a * b`.
pub fn product(a: Value, b: Value, notes: &mut Notes) -> Value {
    if a.curve.width == 1 && b.curve.width == 1 {
        if let Some(k) = fixed(&b) {
            return scaled(a, k[0]);
        }
        if let Some(k) = fixed(&a) {
            return scaled(b, k[0]);
        }
    }
    combine(a, b, |x, y| x * y, "product", notes)
}

/// `a + b`.
pub fn sum(a: Value, b: Value, notes: &mut Notes) -> Value {
    combine(a, b, |x, y| x + y, "sum", notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::fxr::ValueKind;

    fn keyed(function: Function, positions: &[f32], values: &[f32], tangents: Option<(&[f32], &[f32])>) -> Property {
        let n = positions.len();
        let mut fields = vec![n as u32];
        let lo = values.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        fields.extend([lo.to_bits(), hi.to_bits()]);
        fields.extend(positions.iter().map(|p| p.to_bits()));
        fields.extend(values.iter().map(|v| v.to_bits()));
        if let Some((a, b)) = tangents {
            fields.extend(a.iter().chain(b).map(|v| v.to_bits()));
        }
        Property { kind: ValueKind::Scalar, function, looped: false, type_b: 0, fields, modifiers: Vec::new() }
    }

    #[test]
    fn linear_and_stepped_keys_carry_over() {
        let mut notes = Notes::default();
        let c = curve(&keyed(Function::Linear, &[0.0, 2.0], &[1.0, 5.0], None), &mut notes);
        assert_eq!(c.interpolation, Interpolation::Linear);
        assert_eq!(c.value_at(1.0)[0], 3.0);
        let s = curve(&keyed(Function::Stepped, &[0.0, 2.0], &[1.0, 5.0], None), &mut notes);
        assert_eq!(s.interpolation, Interpolation::Step);
        assert_eq!(s.value_at(1.9)[0], 1.0);
        assert!(notes.lost.is_empty() && notes.approximated.is_empty());
    }

    #[test]
    fn a_bezier_matches_the_files_function() {
        let mut notes = Notes::default();
        let p = keyed(Function::Bezier, &[0.0, 1.0], &[0.0, 1.0], Some((&[0.3, 0.0], &[0.0, 0.6])));
        let c = curve(&p, &mut notes);
        for x in [0.0, 0.2, 0.5, 0.9, 1.0] {
            assert!((c.value_at(x)[0] - p.value_at(x)[0]).abs() < 1e-5, "{x}");
        }
    }

    #[test]
    fn a_hermite_is_sampled_within_its_tolerance_and_counted() {
        let mut notes = Notes::default();
        let q = std::f32::consts::FRAC_PI_4;
        for (s1, s2) in [(q, q), (0.0, 0.0), (1.2, 0.1), (0.1, 1.5)] {
            let p = keyed(Function::Hermite, &[0.0, 2.0, 3.0], &[0.0, 10.0, 4.0], Some((&[s1, s1, 0.0], &[s2, s2, 0.0])));
            let c = curve(&p, &mut notes);
            assert_eq!(c.interpolation, Interpolation::Linear);
            for i in 0..=60 {
                let x = i as f32 * 0.05;
                let (got, want) = (c.value_at(x)[0], p.value_at(x)[0]);
                assert!((got - want).abs() <= 0.02 * 10.0, "{s1} {s2} at {x}: {got} against {want}");
            }
        }
        assert_eq!(notes.approximated.values().sum::<u32>(), 4);
    }

    #[test]
    fn modifiers_become_variations_and_external_values_inputs() {
        use fromsoft_formats::fxr::Modifier;
        let mut p = Property::constant_of(&[10.0]);
        p.modifiers.push(Modifier { type_a: 0xD050, type_b: 0, fields: vec![7, 0.5f32.to_bits()], properties: Vec::new() });
        p.modifiers.push(Modifier {
            type_a: 0xE060,
            type_b: 0,
            fields: vec![9],
            properties: vec![Property::constant_of(&[2.0])],
        });
        let mut inputs = Inputs::default();
        let mut notes = Notes::default();
        let v = value(&p, Argument::Age, &mut inputs, &mut notes);
        assert_eq!(v.argument, Argument::Age);
        assert!(matches!(v.variation[0], Variation::Delta { stream: 7, .. }));
        assert!(matches!(v.variation[1], Variation::Scaled { input: 0, .. }));
        assert_eq!(inputs.list.len(), 1);
        assert_eq!(inputs.list[0].name, "external value 9");
    }
}
