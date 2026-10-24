//! How a reading of a program from probe runs ([`ProgramProbe`]) is accepted: differences counted in units in the last
//! place against one bound, random samples from one seeded sequence, and each value read attributed to the material
//! words it reads by changing one word at a time. Shared by every derivation from a probed program, so each is held to
//! the same measure.
use crate::detail_probe::ProgramProbe;

/// How far a value read from a program may be from the equation's, in [`units`]: an equation's f32 order and the
/// G-buffer's encodings decoded back are each a few roundings; 64 covers them with room, and is far below any difference
/// of form (a wrong constant or term moves a lane by orders more).
pub const BOUND_UNITS: f32 = 64.;

/// Units of `f32::EPSILON` `a` is from `b`, of the larger magnitude (or of one, near zero).
pub fn units(a: f32, b: f32) -> f32 {
    (a - b).abs() / (f32::EPSILON * a.abs().max(b.abs()).max(1.))
}

/// Whether two values differ past [`BOUND_UNITS`]: by more than the roundings of a term that only rounds through them (a
/// lerp at either end, `w * (x - y) + y`, carries `y`'s rounding at `w = 1`).
pub fn moved(a: f32, b: f32) -> bool {
    units(a, b) > BOUND_UNITS
}

/// A seeded sequence of floats from 0 to 1 (xorshift32), the same for the same seed: a proof's random samples.
#[derive(Clone, Debug)]
pub struct Seeded(u32);

impl Seeded {
    /// The sequence of `seed` (made odd, so never the stuck zero state).
    pub fn new(seed: u32) -> Self {
        Seeded(seed | 1)
    }

    /// The next float, from 0 to 1.
    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Four floats: a texture sample's lanes.
    pub fn lanes(&mut self) -> [f32; 4] {
        [self.unit(), self.unit(), self.unit(), self.unit()]
    }
}

/// A material word a value read reads: its field, the word's place in the field and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct WordSource {
    pub field: String,
    pub word: u32,
    pub value: f32,
}

/// A material word changed by enough to move what reads it far past [`BOUND_UNITS`] (one part in 32), and to something
/// where it is zero.
pub fn perturbed(v: f32) -> f32 {
    if v == 0. { 1. / 32. } else { v * (1. + 1. / 32.) }
}

/// Each value `read` reads from `probe`'s program (named, in a fixed order), beside the material words it reads: each
/// word of the material ([`ProgramProbe::material`]) changed alone ([`perturbed`]) and the values read again, a value
/// moved past [`BOUND_UNITS`] reading the word. The words are restored; a reading that fails with a word changed is an
/// error naming the word.
pub fn word_sources(
    probe: &mut ProgramProbe,
    read: impl Fn(&ProgramProbe) -> Result<Vec<(String, f32)>, String>,
) -> Result<Vec<(String, f32, Vec<WordSource>)>, String> {
    let values = read(probe)?;
    let mut sources: Vec<Vec<WordSource>> = vec![Vec::new(); values.len()];
    for m in probe.material.clone() {
        for k in 0..m.words {
            let word = m.word + k;
            let [bits] = probe
                .words(m.buffer, word, 1)
                .ok_or_else(|| format!("{} word {k} is past its buffer", m.name))?[..]
            else {
                unreachable!("one word read")
            };
            let value = f32::from_bits(bits);
            probe
                .set_words(m.buffer, word, &[perturbed(value).to_bits()])
                .map_err(|e| e.to_string())?;
            let changed = read(probe);
            probe
                .set_words(m.buffer, word, &[bits])
                .map_err(|e| e.to_string())?;
            let changed = changed.map_err(|e| format!("{} word {k} changed: {e}", m.name))?;
            for (i, ((_, before), (_, after))) in values.iter().zip(changed).enumerate() {
                if moved(*before, after) {
                    sources[i].push(WordSource {
                        field: m.name.clone(),
                        word: k,
                        value,
                    });
                }
            }
        }
    }
    Ok(values
        .into_iter()
        .zip(sources)
        .map(|((name, value), sources)| (name, value, sources))
        .collect())
}
