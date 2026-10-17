//! Evaluate a pixel shader over a UV grid: the generic part of a material bake. The caller states everything the
//! program reads (constant layout and values, a texture per sampled slot, a value per used input) and which output
//! registers it wants; the evaluator has no game names and no policy, and refuses what it is not given.

use crate::{Error, PixelShader, Reflection, Sample, Type};
use std::collections::BTreeMap;
use std::sync::Arc;

fn error(at: usize, detail: impl Into<String>) -> Error {
    Error {
        bytecode: "evaluate",
        at,
        detail: detail.into(),
    }
}

/// Where a named constant lives, in 32-bit words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Constant-buffer bind slot (`cb<n>`).
    pub buffer: u32,
    /// Word offset in the buffer.
    pub offset: u32,
    /// Words the field occupies, padding inside arrays and matrices included.
    pub words: u32,
}

/// The constant buffers a program reads and where each named constant lives. Built from the bytecode's reflection
/// ([`Layout::of`]) or by the caller from a game's own records when the bytecode carries none.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Bind slot and size in words of each buffer the program reads.
    pub buffers: Vec<(u32, u32)>,
    pub fields: BTreeMap<String, Field>,
}
impl Layout {
    /// The program's buffers, and every reflected field by name: struct members are flattened to their own names, as
    /// D3D reflection names them. A name that occurs twice or a field off a word boundary is an error.
    pub fn of(shader: &dyn PixelShader) -> Result<Self, Error> {
        let mut layout = Self {
            buffers: shader
                .constant_buffers()
                .iter()
                .map(|&(slot, rows)| {
                    rows.checked_mul(4)
                        .map(|words| (slot, words))
                        .ok_or_else(|| error(0, "constant buffer size overflow"))
                })
                .collect::<Result<_, _>>()?,
            fields: BTreeMap::new(),
        };
        layout.add_reflected(shader.reflection())?;
        Ok(layout)
    }
    fn add_reflected(&mut self, reflection: &Reflection) -> Result<(), Error> {
        for buffer in &reflection.buffers {
            let slot = reflection
                .resources
                .iter()
                .find(|r| r.kind == 0 && r.name == buffer.name)
                .ok_or_else(|| error(0, format!("buffer {} has no binding", buffer.name)))?
                .slot;
            for v in &buffer.variables {
                if v.ty.members.is_empty() {
                    self.insert(slot, &v.name, v.offset, v.size.div_ceil(4))?;
                } else {
                    self.members(slot, v.offset, &v.ty)?;
                }
            }
        }
        Ok(())
    }
    fn members(&mut self, slot: u32, offset: u32, ty: &Type) -> Result<(), Error> {
        for m in &ty.members {
            let at = offset
                .checked_add(m.offset)
                .ok_or_else(|| error(0, "member offset overflow"))?;
            if m.ty.members.is_empty() {
                self.insert(slot, &m.name, at, words(&m.ty)?)?;
            } else {
                self.members(slot, at, &m.ty)?;
            }
        }
        Ok(())
    }
    fn insert(&mut self, buffer: u32, name: &str, offset: u32, words: u32) -> Result<(), Error> {
        if !offset.is_multiple_of(4) {
            return Err(error(
                offset as usize,
                format!("field {name} is not word-aligned"),
            ));
        }
        let field = Field {
            buffer,
            offset: offset / 4,
            words,
        };
        if self.fields.insert(name.to_owned(), field).is_some() {
            return Err(error(offset as usize, format!("field name {name} occurs twice")));
        }
        Ok(())
    }
}

/// Words of a member type under D3D constant packing ([`Type::packed_words`]).
fn words(ty: &Type) -> Result<u32, Error> {
    ty.packed_words().ok_or_else(|| {
        error(
            0,
            format!("a constant of variable class {} has no packed size", ty.class),
        )
    })
}

/// Constant-buffer words of one layout, zero until set.
#[derive(Clone, Debug)]
pub struct Constants {
    fields: BTreeMap<String, Field>,
    buffers: Vec<(u32, Vec<[u32; 4]>)>,
    written: Vec<Vec<bool>>,
}
impl Constants {
    pub fn new(layout: &Layout) -> Self {
        Self {
            fields: layout.fields.clone(),
            buffers: layout
                .buffers
                .iter()
                .map(|&(slot, words)| (slot, vec![[0; 4]; words.div_ceil(4) as usize]))
                .collect(),
            written: layout
                .buffers
                .iter()
                .map(|&(_, words)| vec![false; words.div_ceil(4) as usize * 4])
                .collect(),
        }
    }
    /// Write a field's leading words. `Ok(false)` when the layout has no such field; more values than the field has
    /// words is an error. Words past the rows the program reads (or in a buffer it does not read) are not stored:
    /// the program cannot read them.
    pub fn set(&mut self, field: &str, values: &[f32]) -> Result<bool, Error> {
        let Some(&f) = self.fields.get(field) else {
            return Ok(false);
        };
        if values.len() > f.words as usize {
            return Err(error(
                0,
                format!("{} values for {field}, which has {} words", values.len(), f.words),
            ));
        }
        if let Some(b) = self.buffers.iter().position(|(slot, _)| *slot == f.buffer) {
            for (i, v) in values.iter().enumerate() {
                let word = f.offset as usize + i;
                if word < self.written[b].len() {
                    self.store(b, word, v.to_bits());
                }
            }
        }
        Ok(true)
    }
    /// Write raw words at a stated offset of a buffer the program reads (a source that states offsets, integer
    /// fields, or a caller's own field rules). Words outside the buffer are an error.
    pub fn set_words(&mut self, buffer: u32, word_offset: u32, words: &[u32]) -> Result<(), Error> {
        let b = self
            .buffers
            .iter()
            .position(|(slot, _)| *slot == buffer)
            .ok_or_else(|| error(0, format!("the program reads no cb{buffer}")))?;
        let end = (word_offset as usize)
            .checked_add(words.len())
            .filter(|&end| end <= self.written[b].len())
            .ok_or_else(|| error(word_offset as usize, format!("words past the end of cb{buffer}")))?;
        for (word, &bits) in (word_offset as usize..end).zip(words) {
            self.store(b, word, bits);
        }
        Ok(())
    }
    fn store(&mut self, b: usize, word: usize, bits: u32) {
        self.buffers[b].1[word / 4][word % 4] = bits;
        self.written[b][word] = true;
    }
    /// Words never written, padding included: what the caller leaves at zero.
    pub fn unset_words(&self) -> u64 {
        self.written.iter().flatten().filter(|w| !**w).count() as u64
    }
}

/// A texture the caller samples itself (its footprint, addressing, filtering and transfer are its own policy).
pub trait Image: Send + Sync + std::fmt::Debug {
    fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4], String>;
}
/// What a texture slot samples.
#[derive(Clone, Debug)]
pub enum Texture {
    Image(Arc<dyn Image>),
    Constant([f32; 4]),
}

/// One component of an input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lane {
    Bits(u32),
    /// The grid's U (times the period).
    U,
    /// The grid's V (times the period).
    V,
}
/// A used input's value. Lane k is the element's k-th component, wherever the signature places the element in its
/// register (`TEXCOORD1` packed into `.zw` receives lanes 0 and 1 there); lanes past its components are unused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Lanes([Lane; 4]),
    Constant([f32; 4]),
    /// U, V, 0, 0.
    GridUv,
}
impl Input {
    fn lanes(self) -> [Lane; 4] {
        match self {
            Self::Lanes(lanes) => lanes,
            Self::Constant(c) => c.map(|v| Lane::Bits(v.to_bits())),
            Self::GridUv => [Lane::U, Lane::V, Lane::Bits(0), Lane::Bits(0)],
        }
    }
}

/// Pixel (x, y) of a size x size grid is evaluated at U = (x + 0.5) / size * period[0], V likewise with y and
/// period[1], in f32.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub size: u32,
    pub period: [f32; 2],
}

#[derive(Debug)]
pub struct Evaluation {
    /// Per requested output: size * size values in row-major order (unrequested components zero), or None when the
    /// program does not write the requested components.
    pub targets: Vec<Option<Vec<[f32; 4]>>>,
    pub instructions: u64,
    pub samples: u64,
}

/// Evaluate `shader` once per grid pixel. `textures` are by bind slot (`t<n>`); a slot the program samples without
/// one is an error. `inputs` are by (semantic, index), compared case-insensitively; a used input without one is an
/// error. `targets` are (output register, required component mask).
pub fn evaluate(
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    grid: Grid,
    targets: &[(u32, u8)],
) -> Result<Evaluation, Error> {
    if grid.size == 0 || grid.size > 4096 || !grid.period.iter().all(|p| p.is_finite() && *p > 0.) {
        return Err(error(
            0,
            "grid size must be 1..=4096 and its period finite and positive",
        ));
    }
    if let Some(slot) = shader.textures().iter().find(|s| !textures.contains_key(s)) {
        return Err(error(0, format!("t{slot} is sampled but has no texture")));
    }
    if targets.iter().any(|&(_, mask)| mask == 0 || mask > 15) {
        return Err(error(0, "a target's component mask must be 1..=15"));
    }
    // Each used input element's lanes, placed at its register and components.
    let mut placed: Vec<(usize, usize, Lane)> = Vec::new();
    let mut registers = vec![[0u32; 4]; shader.input_count()];
    for element in shader.reflection().inputs.iter().filter(|e| e.used_mask != 0) {
        let key = inputs
            .iter()
            .find(|((name, index), _)| name.eq_ignore_ascii_case(&element.name) && *index == element.index)
            .map(|(_, input)| *input)
            .ok_or_else(|| {
                error(
                    0,
                    format!("no value for used input {}{}", element.name, element.index),
                )
            })?;
        let register = element.register as usize;
        if register >= registers.len() || element.mask == 0 || element.mask > 15 {
            return Err(error(
                0,
                format!(
                    "input {}{} is outside the program's registers",
                    element.name, element.index
                ),
            ));
        }
        let components = (0..4).filter(|c| element.mask & (1 << c) != 0);
        for (component, lane) in components.zip(key.lanes()) {
            match lane {
                Lane::Bits(bits) => registers[register][component] = bits,
                _ => placed.push((register, component, lane)),
            }
        }
    }
    let refs: Vec<(u32, &[[u32; 4]])> = constants
        .buffers
        .iter()
        .map(|(s, v)| (*s, v.as_slice()))
        .collect();
    let pixels = grid.size as usize * grid.size as usize;
    let mut written: Vec<Option<bool>> = vec![None; targets.len()];
    let mut values: Vec<Vec<[f32; 4]>> = targets.iter().map(|_| Vec::with_capacity(pixels)).collect();
    let (mut instructions, mut samples) = (0u64, 0u64);
    let mut sampler = |s: Sample| match textures.get(&s.texture) {
        Some(Texture::Image(image)) => image.sample(s.uv),
        Some(Texture::Constant(c)) => Ok(*c),
        None => Err(format!("t{} has no texture", s.texture)),
    };
    for y in 0..grid.size {
        for x in 0..grid.size {
            let u = (x as f32 + 0.5) / grid.size as f32 * grid.period[0];
            let v = (y as f32 + 0.5) / grid.size as f32 * grid.period[1];
            for &(register, lane, source) in &placed {
                registers[register][lane] = if source == Lane::U { u } else { v }.to_bits();
            }
            let pixel = (y * grid.size + x) as usize;
            let result = shader.execute(&registers, &refs, &mut sampler).map_err(|mut e| {
                e.detail = format!("pixel {pixel}: {}", e.detail);
                e
            })?;
            instructions += result.instructions as u64;
            samples += result.samples as u64;
            for (t, &(register, mask)) in targets.iter().enumerate() {
                let writes = result
                    .output_masks
                    .get(register as usize)
                    .is_some_and(|m| m & mask == mask);
                if *written[t].get_or_insert(writes) != writes {
                    return Err(error(
                        pixel,
                        format!("o{register} is written at some pixels only"),
                    ));
                }
                if writes {
                    let bits = result.outputs[register as usize];
                    values[t].push(std::array::from_fn(|c| {
                        if mask & (1 << c) != 0 {
                            f32::from_bits(bits[c])
                        } else {
                            0.
                        }
                    }));
                }
            }
        }
    }
    Ok(Evaluation {
        targets: written
            .into_iter()
            .zip(values)
            .map(|(w, v)| w.unwrap_or(false).then_some(v))
            .collect(),
        instructions,
        samples,
    })
}
