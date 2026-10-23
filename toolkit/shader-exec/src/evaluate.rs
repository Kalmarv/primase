//! Evaluate a pixel shader over a UV grid: the generic part of a material bake. The caller states everything the
//! program reads (constant layout and values, a texture per sampled slot, a value per used input) and which output
//! registers it wants; the evaluator has no game names and no policy, and refuses what it is not given.

use crate::{Error, Load, PixelShader, Sample, Sampler};
use primase_shader::{ConstantType, D3dClass, Interface, Member, ResourceKind};
use std::collections::BTreeMap;
use std::sync::Arc;

use primase_core::parallel::Threads;

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

/// The constant buffers a program reads and where each named constant lives. Built from the program's interface
/// ([`Layout::of`]) or by the caller from a game's own records when the bytecode carries none.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Bind slot and size in words of each buffer the program reads.
    pub buffers: Vec<(u32, u32)>,
    pub fields: BTreeMap<String, Field>,
}
impl Layout {
    /// The program's buffers, and every constant its interface names, by name: a structure's members are flattened to
    /// their own names, as D3D reflection names them. A name that occurs twice or a constant off a word boundary is
    /// an error.
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
        layout.add_named(shader.interface())?;
        Ok(layout)
    }
    fn add_named(&mut self, interface: &Interface) -> Result<(), Error> {
        for resource in &interface.resources {
            let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
                continue;
            };
            let slot = resource
                .provenance
                .d3d
                .filter(|b| b.class == D3dClass::ConstantBuffer)
                .ok_or_else(|| {
                    error(
                        0,
                        format!(
                            "buffer {} has no constant-buffer register",
                            resource.provenance.name
                        ),
                    )
                })?
                .register;
            for c in &layout.constants {
                match c.ty.members() {
                    [] => self.insert(slot, &c.name, c.offset, words(&c.ty)?)?,
                    members => self.members(slot, c.offset, members)?,
                }
            }
        }
        Ok(())
    }
    fn members(&mut self, slot: u32, offset: u32, members: &[Member]) -> Result<(), Error> {
        for m in members {
            let at = offset
                .checked_add(m.offset)
                .ok_or_else(|| error(0, "member offset overflow"))?;
            match m.ty.members() {
                [] => self.insert(slot, &m.name, at, words(&m.ty)?)?,
                inner => self.members(slot, at, inner)?,
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

/// Words a constant of this type occupies in its buffer, padding inside arrays and matrices included.
fn words(ty: &ConstantType) -> Result<u32, Error> {
    ty.size()
        .map(|bytes| bytes.div_ceil(4))
        .ok_or_else(|| error(0, "a constant's layout overflows"))
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
    /// Every buffer the program reads, by slot, as its rows of four words (zero where nothing was written).
    pub fn buffers(&self) -> &[(u32, Vec<[u32; 4]>)] {
        &self.buffers
    }
    /// Words never written, padding included: what the caller leaves at zero.
    pub fn unset_words(&self) -> u64 {
        self.written.iter().flatten().filter(|w| !**w).count() as u64
    }
}

/// A texture the caller samples itself (its footprint, addressing, filtering and transfer are its own policy), by the
/// level of detail the program states ([`crate::Lod`]).
pub trait Image: Send + Sync + std::fmt::Debug {
    fn sample(&self, uv: [f32; 2], lod: crate::Lod) -> Result<[f32; 4], String>;
}
/// What a texture slot samples.
#[derive(Clone, Debug)]
pub enum Texture {
    Image(Arc<dyn Image>),
    /// One value everywhere: what a sample returns, and a load the value's bits (as a float view returns them).
    Constant([f32; 4]),
}

/// The textures by slot, answering samples and loads.
struct Bound<'t>(&'t BTreeMap<u32, Texture>);

impl Sampler for Bound<'_> {
    fn sample(&mut self, s: Sample) -> Result<[f32; 4], String> {
        match self.0.get(&s.texture) {
            Some(Texture::Image(_)) if s.layer.is_some() => {
                Err(format!("t{}: an array layer of an image", s.texture))
            }
            Some(Texture::Image(image)) => image.sample(s.uv, s.lod),
            Some(Texture::Constant(c)) => Ok(*c),
            None => Err(format!("t{} has no texture", s.texture)),
        }
    }
    fn load(&mut self, l: Load) -> Result<[u32; 4], String> {
        match self.0.get(&l.texture) {
            Some(Texture::Constant(c)) => Ok(c.map(f32::to_bits)),
            Some(Texture::Image(_)) => Err(format!("t{}: a load of an image", l.texture)),
            None => Err(format!("t{} has no texture", l.texture)),
        }
    }
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
/// period[1], in f32. The grid is the screen: an input lane taking U steps by period[0] / size along x (its d/dx) and
/// not along y; V likewise along y; other lanes are constant.
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

/// How [`evaluate_raw`] runs the program over the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invocation {
    /// One pixel at a time; a derivative is the grid's step for an input the caller places with [`Lane::U`] or
    /// [`Lane::V`] (zero for a constant input), and of nothing computed.
    Scalar,
    /// 2 x 2 quads, as a GPU runs them ([`PixelShader::execute_quad`]): a derivative is the difference inside the
    /// quad, of inputs and computed values alike, and an implicit-LOD sample's footprint the quad's gradients of its
    /// coordinates. A quad past the grid's last row or column runs its outside lanes as helpers (their coordinates
    /// continue past the edge), their values not kept.
    Quads,
}

/// Every requested output of every pixel, as the program wrote it.
#[derive(Debug)]
pub struct Raw {
    /// Per requested target, size * size pixels in row-major order: the output register's bits and the lanes the
    /// program wrote of it (within the target's mask).
    pub pixels: Vec<Vec<([u32; 4], u8)>>,
    pub instructions: u64,
    pub samples: u64,
}

/// Evaluate `shader` over the grid as `invocation` says, keeping each requested output's bits and written lanes per
/// pixel ([`Raw`]). `textures` are by bind slot (`t<n>`); a slot the program samples without one is an error.
/// `inputs` are by (semantic, index), compared case-insensitively; a used input without one is an error. `targets`
/// are (output register, component mask).
pub fn evaluate_raw(
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    grid: Grid,
    targets: &[(u32, u8)],
    invocation: Invocation,
) -> Result<Raw, Error> {
    evaluate_raw_with(
        Threads::ONE,
        shader,
        constants,
        textures,
        inputs,
        grid,
        targets,
        invocation,
    )
}

/// [`evaluate_raw`] on `threads` threads: the grid's rows (of quads, for [`Invocation::Quads`]) worked one per thread
/// at a time, each with its own sampler, the pixels and counts gathered in row order: the same [`Raw`] on any count.
/// The first failing row's error, as the serial run's.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_raw_with(
    threads: Threads,
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    grid: Grid,
    targets: &[(u32, u8)],
    invocation: Invocation,
) -> Result<Raw, Error> {
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
    for element in shader.interface().inputs.iter().filter(|e| e.used) {
        let (name, index) = (&element.label.name, element.label.index);
        let key = inputs
            .iter()
            .find(|((n, i), _)| n.eq_ignore_ascii_case(name) && *i == index)
            .map(|(_, input)| *input)
            .ok_or_else(|| error(0, format!("no value for used input {name}{index}")))?;
        let d3d = element
            .provenance
            .d3d
            .ok_or_else(|| error(0, format!("input {name}{index} has no D3D register")))?;
        let register = d3d.register as usize;
        if register >= registers.len() || d3d.mask == 0 || d3d.mask > 15 {
            return Err(error(
                0,
                format!("input {name}{index} is outside the program's registers"),
            ));
        }
        let components = (0..4).filter(|c| d3d.mask & (1 << c) != 0);
        for (component, lane) in components.zip(key.lanes()) {
            match lane {
                Lane::Bits(bits) => registers[register][component] = bits,
                _ => placed.push((register, component, lane)),
            }
        }
    }
    // the inputs' screen derivatives, the grid's steps (lanes not placed are constant)
    let mut derivatives = vec![[[0u32; 4]; 2]; shader.input_count()];
    for &(register, component, lane) in &placed {
        let (axis, step) = if lane == Lane::U {
            (0, grid.period[0] / grid.size as f32)
        } else {
            (1, grid.period[1] / grid.size as f32)
        };
        derivatives[register][axis][component] = step.to_bits();
    }
    let refs: Vec<(u32, &[[u32; 4]])> = constants
        .buffers
        .iter()
        .map(|(s, v)| (*s, v.as_slice()))
        .collect();
    let size = grid.size as usize;
    // the registers of pixel (x, y), which may lie past the grid (a quad's helper lanes)
    let at = |x: u32, y: u32| -> Vec<[u32; 4]> {
        let u = (x as f32 + 0.5) / grid.size as f32 * grid.period[0];
        let v = (y as f32 + 0.5) / grid.size as f32 * grid.period[1];
        let mut r = registers.clone();
        for &(register, lane, source) in &placed {
            r[register][lane] = if source == Lane::U { u } else { v }.to_bits();
        }
        r
    };
    // one row's (or row of quads') kept results: pixel index, then per target its bits and written lanes
    type Kept = Vec<(usize, Vec<([u32; 4], u8)>)>;
    let kept = |result: &crate::Execution| -> Vec<([u32; 4], u8)> {
        targets
            .iter()
            .map(|&(register, mask)| {
                let written = result.output_masks.get(register as usize).copied().unwrap_or(0) & mask;
                let bits = result.outputs.get(register as usize).copied().unwrap_or([0; 4]);
                (bits, written)
            })
            .collect()
    };
    let row = |y: u32| -> Result<(Kept, u64, u64), Error> {
        let mut sampler = Bound(textures);
        let (mut out, mut instructions, mut samples): (Kept, u64, u64) = (Vec::new(), 0, 0);
        let mut keep = |pixel: usize, result: &crate::Execution| {
            instructions += result.instructions as u64;
            samples += result.samples as u64;
            out.push((pixel, kept(result)));
        };
        match invocation {
            Invocation::Scalar => {
                for x in 0..grid.size {
                    let pixel = (y * grid.size + x) as usize;
                    let result = shader
                        .execute_with_derivatives(&at(x, y), &derivatives, &refs, &mut sampler)
                        .map_err(|mut e| {
                            e.detail = format!("pixel {pixel}: {}", e.detail);
                            e
                        })?;
                    keep(pixel, &result);
                }
            }
            Invocation::Quads => {
                for x in (0..grid.size).step_by(2) {
                    let lanes = [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)];
                    let inputs = lanes.map(|(lx, ly)| at(lx, ly));
                    let results = shader
                        .execute_quad(
                            [&inputs[0], &inputs[1], &inputs[2], &inputs[3]],
                            &refs,
                            &mut sampler,
                        )
                        .map_err(|mut e| {
                            e.detail = format!("quad at pixel ({x}, {y}): {}", e.detail);
                            e
                        })?;
                    for ((lx, ly), result) in lanes.into_iter().zip(&results) {
                        if lx < grid.size && ly < grid.size {
                            keep((ly * grid.size + lx) as usize, result);
                        }
                    }
                }
            }
        }
        Ok((out, instructions, samples))
    };
    let rows: Vec<u32> = match invocation {
        Invocation::Scalar => (0..grid.size).collect(),
        Invocation::Quads => (0..grid.size).step_by(2).collect(),
    };
    let done = primase_core::parallel::map(threads, &rows, |_, &y| row(y))
        .map_err(|p| error(0, format!("row {}: {}", rows[p.item], p.message)))?;
    let mut pixels: Vec<Vec<([u32; 4], u8)>> =
        targets.iter().map(|_| vec![([0; 4], 0); size * size]).collect();
    let (mut instructions, mut samples) = (0u64, 0u64);
    for r in done {
        let (kept, i, s) = r?;
        instructions += i;
        samples += s;
        for (pixel, values) in kept {
            for (t, v) in values.into_iter().enumerate() {
                pixels[t][pixel] = v;
            }
        }
    }
    Ok(Raw {
        pixels,
        instructions,
        samples,
    })
}

/// Evaluate `shader` once per grid pixel ([`Invocation::Scalar`]). `textures` are by bind slot (`t<n>`); a slot the
/// program samples without one is an error. `inputs` are by (semantic, index), compared case-insensitively; a used
/// input without one is an error. `targets` are (output register, required component mask); a target written at some
/// pixels only is an error.
pub fn evaluate(
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    grid: Grid,
    targets: &[(u32, u8)],
) -> Result<Evaluation, Error> {
    evaluate_with(Threads::ONE, shader, constants, textures, inputs, grid, targets)
}

/// [`evaluate`] on `threads` threads ([`evaluate_raw_with`], a row of the grid each at a time): the same evaluation on
/// any count.
pub fn evaluate_with(
    threads: Threads,
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    grid: Grid,
    targets: &[(u32, u8)],
) -> Result<Evaluation, Error> {
    let raw = evaluate_raw_with(
        threads,
        shader,
        constants,
        textures,
        inputs,
        grid,
        targets,
        Invocation::Scalar,
    )?;
    let mut out = Vec::with_capacity(targets.len());
    for (t, &(register, mask)) in targets.iter().enumerate() {
        let pixels = &raw.pixels[t];
        let writes = pixels.first().is_some_and(|&(_, m)| m == mask);
        if let Some(pixel) = pixels.iter().position(|&(_, m)| (m == mask) != writes) {
            return Err(error(
                pixel,
                format!("o{register} is written at some pixels only"),
            ));
        }
        out.push(writes.then(|| {
            pixels
                .iter()
                .map(|&(bits, _)| {
                    std::array::from_fn(|c| {
                        if mask & (1 << c) != 0 {
                            f32::from_bits(bits[c])
                        } else {
                            0.
                        }
                    })
                })
                .collect()
        }));
    }
    Ok(Evaluation {
        targets: out,
        instructions: raw.instructions,
        samples: raw.samples,
    })
}
