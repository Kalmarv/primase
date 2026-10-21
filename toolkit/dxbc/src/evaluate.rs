use crate::program::{Dimension, derivative, view_address};
use crate::{Operand, Program, Register, Result, Selection, error};
use primase_shader::{AtomicAdd, Execution, Load, Lod, Sample, Sampler, Store};

#[derive(Clone, Copy, Debug, Default)]
pub struct RegisterValue {
    pub bits: [u32; 4],
    pub mask: u8,
}
type Value = RegisterValue;
/// State immediately after an instruction. Unwritten lanes remain masked out.
#[derive(Debug)]
pub struct Trace<'a> {
    pub word: usize,
    pub temps: &'a [RegisterValue],
    pub outputs: &'a [RegisterValue],
}
struct State<'a> {
    temps: Vec<Value>,
    outputs: Vec<Value>,
    inputs: &'a [[u32; 4]],
    derivatives: &'a [[[u32; 4]; 2]],
    buffers: &'a [(u32, &'a [[u32; 4]])],
    immediates: &'a [[u32; 4]],
}
/// Instructions one evaluation may execute: a bound on loops whose exit depends on values.
const BUDGET: usize = 1 << 22;
/// Signed-zero-preserving flush-to-zero for floating instruction boundaries.
fn ftz(v: f32) -> f32 {
    if v.is_subnormal() {
        f32::from_bits(v.to_bits() & 0x8000_0000)
    } else {
        v
    }
}
fn float(v: u32) -> f32 {
    ftz(f32::from_bits(v))
}
fn bits(v: f32) -> u32 {
    ftz(v).to_bits()
}
fn saturation(v: u32) -> u32 {
    let f = float(v);
    if f.is_nan() {
        0
    } else {
        f.clamp(0.0, 1.0).to_bits()
    }
}
impl State<'_> {
    fn source(&self, op: &Operand, mask: u8, word: usize) -> Result<[u32; 4]> {
        let index = op.indices.first().map_or(0, |i| i.base as usize);
        let v = match op.register {
            Register::Immediate => Value {
                bits: op.immediate,
                mask: 15,
            },
            Register::Temp => self.temps[index],
            Register::Input => Value {
                bits: self.inputs[index],
                mask: 15,
            },
            Register::ImmediateBuffer => {
                let ind = &op.indices[0];
                let relative = match &ind.relative {
                    Some(r) => self.source(r, 1, word)?[0],
                    None => 0,
                };
                Value {
                    bits: *self
                        .immediates
                        .get(ind.base.wrapping_add(relative) as usize)
                        .ok_or_else(|| error(word, "immediate-constant-buffer address out of range"))?,
                    mask: 15,
                }
            }
            Register::Constant => {
                let ind = &op.indices[1];
                let relative = if let Some(r) = &ind.relative {
                    self.source(r, 1, word)?[0]
                } else {
                    0
                };
                let i = ind.base.wrapping_add(relative) as usize;
                let data = self
                    .buffers
                    .iter()
                    .find(|(slot, _)| *slot as usize == index)
                    .ok_or_else(|| error(word, "missing constant buffer"))?
                    .1;
                Value {
                    bits: *data
                        .get(i)
                        .ok_or_else(|| error(word, "constant-buffer address out of range"))?,
                    mask: 15,
                }
            }
            _ => return Err(error(word, "not a numeric source")),
        };
        let mut result = [0; 4];
        for (lane, output) in result.iter_mut().enumerate() {
            if mask & (1 << lane) == 0 {
                continue;
            }
            let from = match op.selection {
                Selection::Swizzle(s) => s[lane] as usize,
                Selection::Scalar(c) => c as usize,
                Selection::Mask(_) if op.register == Register::Immediate => lane,
                _ => return Err(error(word, "numeric source needs swizzle or scalar selection")),
            };
            if v.mask & (1 << from) == 0 {
                return Err(error(word, "read of unwritten temporary component"));
            }
            *output = match op.modifier {
                0 => v.bits[from],
                1 => v.bits[from] ^ 0x8000_0000,
                2 => v.bits[from] & 0x7fff_ffff,
                3 => v.bits[from] | 0x8000_0000,
                // an integer instruction's negate: two's complement
                4 => v.bits[from].wrapping_neg(),
                _ => unreachable!("checked modifier"),
            };
        }
        Ok(result)
    }
    fn write(&mut self, op: &Operand, value: [u32; 4]) {
        if op.register == Register::Null {
            return;
        }
        let i = op.indices[0].base as usize;
        let target = if op.register == Register::Temp {
            &mut self.temps[i]
        } else {
            &mut self.outputs[i]
        };
        let Selection::Mask(mask) = op.selection else {
            unreachable!("checked destination")
        };
        for (lane, v) in value.into_iter().enumerate() {
            if mask & (1 << lane) != 0 {
                target.bits[lane] = v;
            }
        }
        target.mask |= mask;
    }
}
/// One invocation: its registers and where it is.
struct Lane<'a> {
    s: State<'a>,
    pc: usize,
    executed: usize,
    samples: usize,
    discarded: bool,
}

impl Lane<'_> {
    fn finish(self) -> Execution {
        Execution {
            outputs: self.s.outputs.iter().map(|v| v.bits).collect(),
            output_masks: self.s.outputs.iter().map(|v| v.mask).collect(),
            instructions: self.executed,
            samples: self.samples,
            discarded: self.discarded,
        }
    }
}

/// Where a lane's run stopped.
enum Stop {
    /// At `ret` (or, run alone, at a discard).
    Done,
    /// At an instruction that needs its quad's other lanes (a derivative, an implicit-LOD sample), not executed.
    Quad,
}

/// Whether a quad's lanes must reach `ins` together: a screen derivative, or a sample whose level of detail is the
/// implicit one (`sample`, `sample_b`), its footprint the quad's.
fn needs_quad(ins: &crate::Instruction) -> bool {
    derivative(ins.opcode) || matches!(ins.opcode, 69 | 74)
}

impl Program {
    /// Evaluate a prevalidated program. Missing inputs/buffers and unwritten
    /// temporaries are errors; no partial output is returned. Sampling callbacks
    /// may already have run when a *runtime* binding error occurs. A program reading
    /// derivatives needs [`Program::execute_with_derivatives`].
    pub fn execute<S: Sampler + ?Sized>(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut S,
    ) -> Result<Execution> {
        self.execute_traced(inputs, &[], buffers, sampler, &mut |_| {})
    }

    /// Evaluate with the inputs' screen derivatives: per input register, its d/dx then d/dy lanes (f32 bits), which
    /// `deriv_rtx`/`deriv_rty` (and their coarse and fine forms) read. A derivative of an input not given is an error.
    pub fn execute_with_derivatives<S: Sampler + ?Sized>(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut S,
    ) -> Result<Execution> {
        self.execute_traced(inputs, derivatives, buffers, sampler, &mut |_| {})
    }

    /// Evaluate with an observational callback after each executed non-ret instruction.
    /// The callback cannot mutate registers; no per-instruction snapshot allocation
    /// is needed unless the observer chooses to retain a copy.
    pub fn execute_traced<S: Sampler + ?Sized>(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut S,
        trace: &mut impl FnMut(Trace<'_>),
    ) -> Result<Execution> {
        self.check(inputs, buffers)?;
        let mut lane = self.lane(inputs, derivatives, buffers);
        self.run_lane(&mut lane, sampler, trace, false)?;
        Ok(lane.finish())
    }

    /// The inputs and constant buffers an evaluation needs, given.
    fn check(&self, inputs: &[[u32; 4]], buffers: &[(u32, &[[u32; 4]])]) -> Result<()> {
        if inputs.len() < self.inputs {
            return Err(error(0, "missing input registers"));
        }
        for &(slot, size) in &self.constant_buffers {
            if buffers.iter().filter(|(s, _)| *s == slot).count() != 1
                || buffers
                    .iter()
                    .find(|(s, _)| *s == slot)
                    .is_none_or(|(_, v)| v.len() != size as usize)
            {
                return Err(error(0, format!("cb{slot} requires exactly {size} vectors")));
            }
        }
        Ok(())
    }

    fn lane<'a>(
        &'a self,
        inputs: &'a [[u32; 4]],
        derivatives: &'a [[[u32; 4]; 2]],
        buffers: &'a [(u32, &'a [[u32; 4]])],
    ) -> Lane<'a> {
        Lane {
            s: State {
                temps: vec![Value::default(); self.temps],
                outputs: vec![Value::default(); self.outputs],
                inputs,
                derivatives,
                buffers,
                immediates: &self.immediates,
            },
            pc: 0,
            executed: 0,
            samples: 0,
            discarded: false,
        }
    }

    /// Run `lane` to its end; in a quad (`quad`), to the next instruction that needs the quad ([`needs_quad`]),
    /// which it leaves unexecuted, and past a discard (the lane goes on as a helper, its outputs marked discarded).
    fn run_lane<S: Sampler + ?Sized>(
        &self,
        lane: &mut Lane<'_>,
        sampler: &mut S,
        trace: &mut impl FnMut(Trace<'_>),
        quad: bool,
    ) -> Result<Stop> {
        let s = &mut lane.s;
        let (mut pc, mut executed, mut samples, mut discarded) =
            (lane.pc, lane.executed, lane.samples, lane.discarded);
        let stop = loop {
            let Some(ins) = self.code.get(pc) else {
                break Stop::Done;
            };
            if quad && needs_quad(ins) {
                break Stop::Quad;
            }
            executed += 1;
            if executed > BUDGET {
                return Err(error(
                    ins.word,
                    "instruction budget exceeded (a loop that does not end?)",
                ));
            }
            if ins.opcode == 62 {
                break Stop::Done;
            }
            let a = &ins.operands;
            // structured flow: `jumps` holds each one's matching instruction
            let target = self.jumps[pc];
            let taken = |s: &State<'_>| -> Result<bool> {
                Ok((s.source(&a[0], 1, ins.word)?[0] != 0) == ins.nonzero)
            };
            let next = match ins.opcode {
                // loop and endif mark places; else, reached in order, ends its if's first branch
                48 | 21 => Some(pc + 1),
                18 => Some(target + 1),
                // endloop goes back to its loop's first instruction; continue likewise
                22 | 7 => Some(target + 1),
                8 => Some(if taken(s)? { target + 1 } else { pc + 1 }),
                // break leaves after its loop's endloop
                2 => Some(self.jumps[target] + 1),
                3 => Some(if taken(s)? { self.jumps[target] + 1 } else { pc + 1 }),
                // if: on to its first branch, or past its else (or endif)
                31 => Some(if taken(s)? { pc + 1 } else { target + 1 }),
                // discard: the invocation ends (in a quad it goes on as a helper, for its neighbours' derivatives)
                13 if taken(s)? => {
                    discarded = true;
                    if !quad {
                        break Stop::Done;
                    }
                    Some(pc + 1)
                }
                13 => Some(pc + 1),
                _ => None,
            };
            if let Some(next) = next {
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                pc = next;
                continue;
            }
            pc += 1;
            let mask = match a[0].selection {
                Selection::Mask(m) => m,
                _ => 0,
            };
            if ins.opcode == 77 {
                // sincos: sine to the first destination, cosine to the second (either may be null)
                let mask2 = match a[1].selection {
                    Selection::Mask(m) => m,
                    _ => 0,
                };
                let x = s.source(&a[2], mask | mask2, ins.word)?;
                let f = |g: fn(f64) -> f64| -> [u32; 4] {
                    let r = x.map(|x| bits(g(f64::from(float(x))) as f32));
                    if ins.saturate { r.map(saturation) } else { r }
                };
                s.write(&a[0], f(f64::sin));
                s.write(&a[1], f(f64::cos));
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                continue;
            }
            if ins.opcode == 78 {
                // udiv: the unsigned quotient to the first destination, the remainder to the second (either may be
                // null); a divisor of 0 gives 0xFFFFFFFF to both, as Direct3D defines it
                let mask2 = match a[1].selection {
                    Selection::Mask(m) => m,
                    _ => 0,
                };
                let x = s.source(&a[2], mask | mask2, ins.word)?;
                let y = s.source(&a[3], mask | mask2, ins.word)?;
                let quotient: [u32; 4] = std::array::from_fn(|i| x[i].checked_div(y[i]).unwrap_or(u32::MAX));
                let remainder: [u32; 4] = std::array::from_fn(|i| x[i].checked_rem(y[i]).unwrap_or(u32::MAX));
                s.write(&a[0], quotient);
                s.write(&a[1], remainder);
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                continue;
            }
            if ins.opcode == 38 {
                let mask2 = match a[1].selection {
                    Selection::Mask(m) => m,
                    _ => 0,
                };
                let x = s.source(&a[2], mask | mask2, ins.word)?;
                let y = s.source(&a[3], mask | mask2, ins.word)?;
                let product: [i64; 4] = std::array::from_fn(|i| (x[i] as i32 as i64) * (y[i] as i32 as i64));
                s.write(&a[0], product.map(|v| (v >> 32) as u32));
                s.write(&a[1], product.map(|v| v as u32));
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                continue;
            }
            let mut result = [0; 4];
            if derivative(ins.opcode) {
                // a scalar evaluation has the derivatives the caller states for its inputs, of nothing computed: a
                // program reaching a derivative of anything else is refused when it does (one that doesn't, runs)
                if a[1].register != Register::Input || a[1].modifier != 0 {
                    return Err(error(
                        ins.word,
                        "a derivative of a computed value (a scalar evaluation has those of its inputs only)",
                    ));
                }
                // d/dx for deriv_rtx (and its coarse and fine forms), d/dy for deriv_rty
                let axis = usize::from(matches!(ins.opcode, 12 | 124 | 125));
                let index = a[1].indices[0].base as usize;
                let lanes = s
                    .derivatives
                    .get(index)
                    .ok_or_else(|| error(ins.word, format!("no derivatives given for v{index}")))?[axis];
                let Selection::Swizzle(swizzle) = a[1].selection else {
                    return Err(error(ins.word, "a derivative's source needs a swizzle"));
                };
                result = std::array::from_fn(|i| lanes[swizzle[i] as usize]);
            } else if matches!(ins.opcode, 69 | 72 | 73 | 74) {
                result = self.sample_texture(s, ins, sampler, None)?;
                samples += 1;
            } else if ins.opcode == 45 {
                // ld: the texel at the address's x, y (and z for an array or a 3D texture), its mip the w; a
                // buffer's element at x alone
                let texture = a[2].indices[0].base;
                let lanes = match self.resource(texture).map(|r| r.dimension) {
                    Some(Dimension::Buffer) => 0b0001,
                    Some(Dimension::Texture2d) => 0b1011,
                    _ => 0b1111,
                };
                let address = s.source(&a[1], lanes, ins.word)?;
                let at = |k: usize| {
                    if lanes & (1 << k) != 0 {
                        address[k] as i32
                    } else {
                        0
                    }
                };
                let loaded = sampler
                    .load(Load {
                        texture,
                        texel: [at(0), at(1), at(2)],
                        mip: at(3),
                    })
                    .map_err(|e| error(ins.word, e))?;
                let Selection::Swizzle(swizzle) = a[2].selection else {
                    return Err(error(ins.word, "resource requires return swizzle"));
                };
                result = std::array::from_fn(|i| loaded[swizzle[i] as usize]);
                samples += 1;
            } else if ins.opcode == 164 {
                // store_uav_typed: a side effect, the caller's; nothing written here
                let view = a[0].indices[0].base;
                let lanes = self.view(view).map_or(0b111, |v| view_address(v.dimension));
                let address = s.source(&a[1], lanes, ins.word)?;
                let value = s.source(&a[2], mask, ins.word)?;
                sampler
                    .store(Store {
                        view,
                        address: std::array::from_fn(|k| if lanes & (1 << k) != 0 { address[k] } else { 0 }),
                        value,
                        mask,
                    })
                    .map_err(|e| error(ins.word, e))?;
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                continue;
            } else if ins.opcode == 168 {
                // store_structured: the words from the element's byte offset, the caller's
                sampler
                    .store(Store {
                        view: a[0].indices[0].base,
                        address: [
                            s.source(&a[1], 1, ins.word)?[0],
                            s.source(&a[2], 1, ins.word)?[0],
                            0,
                        ],
                        value: s.source(&a[3], mask, ins.word)?,
                        mask,
                    })
                    .map_err(|e| error(ins.word, e))?;
                trace(Trace {
                    word: ins.word,
                    temps: &s.temps,
                    outputs: &s.outputs,
                });
                continue;
            } else if ins.opcode == 180 {
                // imm_atomic_iadd: the add the caller's, the word before it the result
                let view = a[1].indices[0].base;
                let lanes = self.view(view).map_or(0b111, |v| view_address(v.dimension));
                let address = s.source(&a[2], lanes, ins.word)?;
                let before = sampler
                    .atomic_add(AtomicAdd {
                        view,
                        address: std::array::from_fn(|k| if lanes & (1 << k) != 0 { address[k] } else { 0 }),
                        value: s.source(&a[3], 1, ins.word)?[0],
                    })
                    .map_err(|e| error(ins.word, e))?;
                result = [before; 4];
            } else if ins.opcode == 167 {
                // ld_structured: the words from the byte offset of the element, picked by the resource's swizzle
                let slot = a[3].indices[0].base;
                let element = s.source(&a[1], 1, ins.word)?[0];
                let offset = s.source(&a[2], 1, ins.word)?[0];
                let Selection::Swizzle(swizzle) = a[3].selection else {
                    return Err(error(ins.word, "resource requires return swizzle"));
                };
                for i in 0..4 {
                    if mask & (1 << i) != 0 {
                        let at = offset.wrapping_add(4 * u32::from(swizzle[i]));
                        result[i] = sampler
                            .structured(slot, element, at)
                            .map_err(|e| error(ins.word, e))?;
                    }
                }
                samples += 1;
            } else {
                let needed = match ins.opcode {
                    15 => 3,
                    16 => 7,
                    17 => 15,
                    _ => mask,
                };
                let x = s.source(&a[1], needed, ins.word)?;
                let y = if a.len() > 2 {
                    s.source(&a[2], needed, ins.word)?
                } else {
                    [0; 4]
                };
                let z = if a.len() > 3 {
                    s.source(&a[3], needed, ins.word)?
                } else {
                    [0; 4]
                };
                let w = if a.len() > 4 {
                    s.source(&a[4], needed, ins.word)?
                } else {
                    [0; 4]
                };
                if (15..=17).contains(&ins.opcode) {
                    let n = (ins.opcode - 13) as usize;
                    let mut dot = ftz(float(x[0]) * float(y[0]));
                    for i in 1..n {
                        dot = ftz(dot + ftz(float(x[i]) * float(y[i])));
                    }
                    result = [dot.to_bits(); 4];
                } else {
                    for i in 0..4 {
                        if mask & (1 << i) == 0 {
                            continue;
                        }
                        let (xf, yf, zf) = (float(x[i]), float(y[i]), float(z[i]));
                        let flag = |b: bool| if b { u32::MAX } else { 0 };
                        let (xi, yi) = (x[i] as i32, y[i] as i32);
                        result[i] = match ins.opcode {
                            0 => bits(xf + yf),
                            1 => x[i] & y[i],
                            14 => bits(xf / yf),
                            24 => flag(xf == yf),
                            25 => bits((xf as f64).exp2() as f32),
                            26 => bits(xf - xf.floor()),
                            // toward zero; NaN 0 and saturating, as D3D's ftoi
                            27 => xf as i32 as u32,
                            28 => xf as u32,
                            29 => flag(xf >= yf),
                            30 => x[i].wrapping_add(y[i]),
                            32 => flag(x[i] == y[i]),
                            33 => flag(xi >= yi),
                            34 => flag(xi < yi),
                            35 => x[i].wrapping_mul(y[i]).wrapping_add(z[i]),
                            36 => xi.max(yi) as u32,
                            37 => xi.min(yi) as u32,
                            39 => flag(x[i] != y[i]),
                            40 => xi.wrapping_neg() as u32,
                            41 => x[i] << (y[i] & 31),
                            42 => (xi >> (y[i] & 31)) as u32,
                            43 => bits(xi as f32),
                            47 => bits((xf as f64).log2() as f32),
                            49 => flag(xf < yf),
                            50 => bits(ftz(xf * yf) + zf),
                            51 => bits(xf.min(yf)),
                            52 => bits(xf.max(yf)),
                            54 => x[i],
                            55 => {
                                if x[i] != 0 {
                                    y[i]
                                } else {
                                    z[i]
                                }
                            }
                            56 => bits(xf * yf),
                            57 => {
                                if xf != yf {
                                    u32::MAX
                                } else {
                                    0
                                }
                            }
                            59 => !x[i],
                            60 => x[i] | y[i],
                            64 => bits(xf.round_ties_even()),
                            65 => bits(xf.floor()),
                            66 => bits(xf.ceil()),
                            67 => bits(xf.trunc()),
                            68 => bits((1.0 / (xf as f64).sqrt()) as f32),
                            75 => bits((xf as f64).sqrt() as f32),
                            79 => flag(x[i] < y[i]),
                            80 => flag(x[i] >= y[i]),
                            83 => x[i].max(y[i]),
                            84 => x[i].min(y[i]),
                            85 => x[i] >> (y[i] & 31),
                            86 => bits(x[i] as f32),
                            87 => x[i] ^ y[i],
                            129 => bits(1.0 / xf),
                            // ubfe: (x & 31) bits of z from bit (y & 31)
                            138 => {
                                let (width, offset) = (x[i] & 31, y[i] & 31);
                                if width == 0 {
                                    0
                                } else if width + offset < 32 {
                                    (z[i] << (32 - width - offset)) >> (32 - width)
                                } else {
                                    z[i] >> offset
                                }
                            }
                            // bfi: z's low (x & 31) bits put at bit (y & 31) of w
                            140 => {
                                let (width, offset) = (x[i] & 31, y[i] & 31);
                                let mask = ((1u32 << width).wrapping_sub(1)) << offset;
                                ((z[i] << offset) & mask) | (w[i] & !mask)
                            }
                            _ => unreachable!("prevalidated opcode"),
                        };
                    }
                }
            }
            if ins.saturate {
                result = result.map(saturation);
            }
            s.write(&a[0], result);
            trace(Trace {
                word: ins.word,
                temps: &s.temps,
                outputs: &s.outputs,
            });
        };
        (lane.pc, lane.executed, lane.samples, lane.discarded) = (pc, executed, samples, discarded);
        Ok(stop)
    }

    /// A sample instruction's texel (`sample`, `sample_l`, `sample_d`, `sample_b`), through its return swizzle;
    /// an implicit level of detail from `footprint` (the quad's gradients of the coordinates) where given.
    fn sample_texture<S: Sampler + ?Sized>(
        &self,
        s: &State<'_>,
        ins: &crate::Instruction,
        sampler: &mut S,
        footprint: Option<[[f32; 2]; 2]>,
    ) -> Result<[u32; 4]> {
        let a = &ins.operands;
        let texture = a[2].indices[0].base;
        let dimension = self.resource(texture).map(|r| r.dimension);
        let unsupported = match dimension {
            Some(Dimension::Texture3d) => Some("a 3D texture"),
            Some(Dimension::TextureCube) => Some("a cube"),
            Some(Dimension::TextureCubeArray) => Some("a cube array"),
            _ => None,
        };
        if let Some(shape) = unsupported {
            return Err(error(
                ins.word,
                format!("t{texture} is {shape}: sampling one is not supported"),
            ));
        }
        let array = dimension == Some(Dimension::Texture2dArray);
        let xyz = s.source(&a[1], if array { 7 } else { 3 }, ins.word)?;
        let lod = match (ins.opcode, footprint) {
            (73, _) => {
                let (dx, dy) = (s.source(&a[4], 3, ins.word)?, s.source(&a[5], 3, ins.word)?);
                Lod::Gradients([[float(dx[0]), float(dx[1])], [float(dy[0]), float(dy[1])]])
            }
            (72, _) => Lod::Level(float(s.source(&a[4], 1, ins.word)?[0])),
            (74, Some(g)) => Lod::GradientsBias(g, float(s.source(&a[4], 1, ins.word)?[0])),
            (74, None) => Lod::Bias(float(s.source(&a[4], 1, ins.word)?[0])),
            (_, Some(g)) => Lod::Gradients(g),
            _ => Lod::Implicit,
        };
        let sampled = sampler
            .sample(Sample {
                texture,
                sampler: a[3].indices[0].base,
                uv: [float(xyz[0]), float(xyz[1])],
                layer: array.then(|| float(xyz[2])),
                lod,
            })
            .map_err(|e| error(ins.word, e))?;
        let Selection::Swizzle(swizzle) = a[2].selection else {
            return Err(error(ins.word, "resource requires return swizzle"));
        };
        Ok(std::array::from_fn(|i| sampled[swizzle[i] as usize].to_bits()))
    }

    /// Evaluate a 2 x 2 quad of invocations together, as a GPU does: lanes 0 and 1 the top row (left, right), 2 and 3
    /// the bottom. A screen derivative is a difference inside the quad: coarse (and `deriv_rtx`, `deriv_rty`) the
    /// top-left lane's row or column for all four, fine each lane's own row or column. An implicit-LOD sample's
    /// footprint is the quad's coarse gradients of its coordinates ([`Lod::Gradients`], [`Lod::GradientsBias`]). A
    /// discarded lane goes on as a helper, its execution marked discarded. The lanes must reach each such
    /// instruction together: where they diverge (one ends, or they stand at different instructions) the quad is
    /// refused.
    pub fn execute_quad<S: Sampler + ?Sized>(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut S,
    ) -> Result<[Execution; 4]> {
        for i in inputs {
            self.check(i, buffers)?;
        }
        let mut lanes = inputs.map(|i| self.lane(i, &[], buffers));
        let mut unobserved = |_: Trace<'_>| {};
        loop {
            let mut stops = Vec::with_capacity(4);
            for lane in &mut lanes {
                stops.push(self.run_lane(lane, sampler, &mut unobserved, true)?);
            }
            if stops.iter().all(|s| matches!(s, Stop::Done)) {
                break;
            }
            let pc = lanes[0].pc;
            let word = self.code.get(pc).map_or(0, |i| i.word);
            if !stops.iter().all(|s| matches!(s, Stop::Quad)) || lanes.iter().any(|l| l.pc != pc) {
                return Err(error(
                    word,
                    "a quad's lanes diverge at a derivative or an implicit-LOD sample",
                ));
            }
            let ins = &self.code[pc];
            let a = &ins.operands;
            let mask = match a[0].selection {
                Selection::Mask(m) => m,
                _ => 0,
            };
            let mut results = [[0u32; 4]; 4];
            if derivative(ins.opcode) {
                let mut v = [[0u32; 4]; 4];
                for (k, lane) in lanes.iter().enumerate() {
                    v[k] = lane.s.source(&a[1], mask, word)?;
                }
                let x = !matches!(ins.opcode, 12 | 124 | 125);
                let fine = matches!(ins.opcode, 123 | 125);
                for (k, r) in results.iter_mut().enumerate() {
                    let (from, to) = match (x, fine) {
                        (true, false) => (0, 1),
                        (false, false) => (0, 2),
                        (true, true) if k < 2 => (0, 1),
                        (true, true) => (2, 3),
                        (false, true) if k % 2 == 0 => (0, 2),
                        (false, true) => (1, 3),
                    };
                    *r = std::array::from_fn(|i| bits(float(v[to][i]) - float(v[from][i])));
                }
            } else {
                let mut uv = [[0f32; 2]; 4];
                for (k, lane) in lanes.iter().enumerate() {
                    let c = lane.s.source(&a[1], 3, word)?;
                    uv[k] = [float(c[0]), float(c[1])];
                }
                let footprint = [
                    [uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]],
                    [uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]],
                ];
                for (k, lane) in lanes.iter_mut().enumerate() {
                    results[k] = self.sample_texture(&lane.s, ins, sampler, Some(footprint))?;
                    lane.samples += 1;
                }
            }
            for (lane, r) in lanes.iter_mut().zip(results) {
                lane.s
                    .write(&a[0], if ins.saturate { r.map(saturation) } else { r });
                lane.pc += 1;
                lane.executed += 1;
            }
        }
        Ok(lanes.map(Lane::finish))
    }
}
