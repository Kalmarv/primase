use crate::program::derivative;
use crate::{Operand, Program, Register, Result, Selection, error};
use primase_shader::{Execution, Sample, Sampler};

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
        let mut s = State {
            temps: vec![Value::default(); self.temps],
            outputs: vec![Value::default(); self.outputs],
            inputs,
            derivatives,
            buffers,
        };
        let mut samples = 0;
        let (mut pc, mut executed) = (0, 0usize);
        while let Some(ins) = self.code.get(pc) {
            executed += 1;
            if executed > BUDGET {
                return Err(error(
                    ins.word,
                    "instruction budget exceeded (a loop that does not end?)",
                ));
            }
            if ins.opcode == 62 {
                break;
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
                8 => Some(if taken(&s)? { target + 1 } else { pc + 1 }),
                // break leaves after its loop's endloop
                2 => Some(self.jumps[target] + 1),
                3 => Some(if taken(&s)? {
                    self.jumps[target] + 1
                } else {
                    pc + 1
                }),
                // if: on to its first branch, or past its else (or endif)
                31 => Some(if taken(&s)? { pc + 1 } else { target + 1 }),
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
            } else if matches!(ins.opcode, 69 | 73) {
                let texture = a[2].indices[0].base;
                if self.volumes.contains(&texture) {
                    return Err(error(
                        ins.word,
                        format!("t{texture} is a 3D texture: sampling one is not supported"),
                    ));
                }
                let xy = s.source(&a[1], 3, ins.word)?;
                let gradients = if ins.opcode == 73 {
                    let (dx, dy) = (s.source(&a[4], 3, ins.word)?, s.source(&a[5], 3, ins.word)?);
                    Some([[float(dx[0]), float(dx[1])], [float(dy[0]), float(dy[1])]])
                } else {
                    None
                };
                let sampled = sampler
                    .sample(Sample {
                        texture,
                        sampler: a[3].indices[0].base,
                        uv: [float(xy[0]), float(xy[1])],
                        gradients,
                    })
                    .map_err(|e| error(ins.word, e))?;
                let Selection::Swizzle(swizzle) = a[2].selection else {
                    return Err(error(ins.word, "resource requires return swizzle"));
                };
                result = std::array::from_fn(|i| sampled[swizzle[i] as usize].to_bits());
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
        }
        Ok(Execution {
            outputs: s.outputs.iter().map(|v| v.bits).collect(),
            output_masks: s.outputs.iter().map(|v| v.mask).collect(),
            instructions: executed,
            samples,
        })
    }
}
