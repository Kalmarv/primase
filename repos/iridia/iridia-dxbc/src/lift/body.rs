//! The instructions: each DXBC instruction as typed operations, with D3D's results where they differ written out.

use iridia::build::{Kind, Val};
use naga::{
    BinaryOperator as Bo, Block, DerivativeAxis, DerivativeControl, Expression as E, MathFunction as Mf,
    Span, Statement, SwitchCase, SwitchValue, UnaryOperator as Uo,
};

use super::{Frame, FrameKind, Lift};
use crate::Result;
use dxbc_reader::decode::{Instruction, Register};

impl Lift<'_> {
    pub(super) fn body(&mut self) -> Result<()> {
        self.body_range(0..usize::MAX)
    }

    /// The instructions whose first word is in `words`: all of them, or one phase of a hull program.
    pub(super) fn body_range(&mut self, words: std::ops::Range<usize>) -> Result<()> {
        let decoded = self.decoded;
        for ins in decoded.instructions.iter().filter(|i| words.contains(&i.word)) {
            if self.returned {
                // unreachable: a `ret` at the top level ended the program
                break;
            }
            self.word = ins.word;
            let name = ins.name();
            self.instruction(ins, name).map_err(|mut e| {
                if !e.detail.starts_with(name) {
                    e.detail = format!("{name}: {}", e.detail);
                }
                e
            })?;
            self.b
                .check()
                .map_err(|detail| crate::error(ins.word, format!("{name}: {detail}")))?;
        }
        Ok(())
    }

    /// A source read once, as all four destination lanes, before an instruction with two destinations writes either
    /// (a destination may alias a source: `sincos r0.x, r1.x, r0.x`); `lanes` then takes a destination's own.
    fn read_whole(&mut self, op: &dxbc_reader::decode::Operand, kind: Kind) -> Result<Val> {
        self.read_lanes(op, &[0, 1, 2, 3], kind)
    }
    fn lanes(&mut self, whole: Val, destination: &dxbc_reader::decode::Operand) -> Val {
        let mask = self.dest_mask(destination);
        self.b.swizzle(whole, &super::resources::lanes_of(mask))
    }

    fn float2(&mut self, ins: &Instruction, f: impl Fn(&mut Self, Val, Val) -> Val) -> Result<()> {
        let mask = self.dest_mask(&ins.operands[0]);
        let a = self.read(&ins.operands[1], mask, Kind::F)?;
        let b = self.read(&ins.operands[2], mask, Kind::F)?;
        let r = f(self, a, b);
        self.write(&ins.operands[0], r, ins.saturate)
    }
    fn float1(&mut self, ins: &Instruction, f: impl Fn(&mut Self, Val) -> Val) -> Result<()> {
        let mask = self.dest_mask(&ins.operands[0]);
        let a = self.read(&ins.operands[1], mask, Kind::F)?;
        let r = f(self, a);
        self.write(&ins.operands[0], r, ins.saturate)
    }
    fn int2(&mut self, ins: &Instruction, kind: Kind, f: impl Fn(&mut Self, Val, Val) -> Val) -> Result<()> {
        let mask = self.dest_mask(&ins.operands[0]);
        let a = self.read(&ins.operands[1], mask, kind)?;
        let b = self.read(&ins.operands[2], mask, kind)?;
        let r = f(self, a, b);
        self.write(&ins.operands[0], r, false)
    }
    /// A comparison whose result is D3D's mask: all ones where it holds.
    fn compare(&mut self, ins: &Instruction, kind: Kind, op: Bo) -> Result<()> {
        let mask = self.dest_mask(&ins.operands[0]);
        let a = self.read(&ins.operands[1], mask, kind)?;
        let b = self.read(&ins.operands[2], mask, kind)?;
        let c = self.b.binary(op, a, b);
        let r = self.b.mask_of(c);
        self.write(&ins.operands[0], r, false)
    }
    /// A flow condition: the first operand's x lane tested as zero or nonzero.
    fn condition(&mut self, ins: &Instruction) -> Result<Val> {
        let v = self.read(&ins.operands[0], 1, Kind::U)?;
        let zero = self.b.lit_u(0);
        Ok(self
            .b
            .binary(if ins.nonzero { Bo::NotEqual } else { Bo::Equal }, v, zero))
    }

    fn instruction(&mut self, ins: &Instruction, name: &str) -> Result<()> {
        use Bo::*;
        let ops = &ins.operands;
        // the operands are read by position below: the count is the opcode's, checked once here
        match crate::arity::operands(name) {
            None => return self.refuse(format!("opcode {} is not lifted", ins.opcode)),
            Some(n) if ops.len() != n => {
                return self.refuse(format!("{} operands, where the instruction takes {n}", ops.len()));
            }
            Some(_) => {}
        }
        match name {
            "nop" => {}
            "mov" => {
                let mask = self.dest_mask(&ops[0]);
                // modifiers and saturation on a mov apply to float data
                let kind = if ops[1].modifier != 0 || ins.saturate {
                    Kind::F
                } else {
                    Kind::U
                };
                let a = self.read(&ops[1], mask, kind)?;
                self.write(&ops[0], a, ins.saturate)?;
            }
            "movc" => {
                let mask = self.dest_mask(&ops[0]);
                let c = self.read(&ops[1], mask, Kind::U)?;
                // a modifier on a movc data operand is a float negate or abs (the data is moved, not interpreted)
                let ka = if ops[2].modifier != 0 { Kind::F } else { Kind::U };
                let kb = if ops[3].modifier != 0 { Kind::F } else { Kind::U };
                let a = self.read(&ops[2], mask, ka)?;
                let b = self.read(&ops[3], mask, kb)?;
                let (a, b) = (self.b.bitcast(a, Kind::U), self.b.bitcast(b, Kind::U));
                let zero = self.b.lit_u(0);
                let cond = self.b.binary(NotEqual, c, zero);
                let r = self.b.select(cond, a, b);
                if ins.saturate {
                    // movc_sat: the moved data is saturated as a float
                    let f = self.b.bitcast(r, Kind::F);
                    self.write(&ops[0], f, true)?;
                } else {
                    self.write(&ops[0], r, false)?;
                }
            }
            "add" => self.float2(ins, |s, a, b| s.b.binary(Add, a, b))?,
            "mul" => self.float2(ins, |s, a, b| s.b.binary(Multiply, a, b))?,
            "div" => self.float2(ins, |s, a, b| s.b.binary(Divide, a, b))?,
            "min" => self.float2(ins, |s, a, b| s.b.d3d_min_max(Mf::Min, a, b))?,
            "max" => self.float2(ins, |s, a, b| s.b.d3d_min_max(Mf::Max, a, b))?,
            "mad" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::F)?;
                let b = self.read(&ops[2], mask, Kind::F)?;
                let c = self.read(&ops[3], mask, Kind::F)?;
                let p = self.b.binary(Multiply, a, b);
                let r = self.b.binary(Add, p, c);
                self.write(&ops[0], r, ins.saturate)?;
            }
            "dp2" | "dp3" | "dp4" => {
                let n = name.as_bytes()[2] - b'0';
                let lanes: Vec<u8> = (0..n).collect();
                let a = self.read_lanes(&ops[1], &lanes, Kind::F)?;
                let b = self.read_lanes(&ops[2], &lanes, Kind::F)?;
                let r = self.b.math(Mf::Dot, &[a, b]);
                self.write(&ops[0], r, ins.saturate)?;
            }
            "rcp" => self.float1(ins, |s, a| {
                let one = s.b.lit_f(1.0);
                s.b.binary(Divide, one, a)
            })?,
            "rsq" => self.float1(ins, |s, a| s.b.math(Mf::InverseSqrt, &[a]))?,
            "sqrt" => self.float1(ins, |s, a| s.b.math(Mf::Sqrt, &[a]))?,
            "exp" => self.float1(ins, |s, a| s.b.math(Mf::Exp2, &[a]))?,
            "log" => self.float1(ins, |s, a| s.b.math(Mf::Log2, &[a]))?,
            "frc" => self.float1(ins, |s, a| s.b.math(Mf::Fract, &[a]))?,
            "round_ne" => self.float1(ins, |s, a| s.b.math(Mf::Round, &[a]))?,
            "round_ni" => self.float1(ins, |s, a| s.b.math(Mf::Floor, &[a]))?,
            "round_pi" => self.float1(ins, |s, a| s.b.math(Mf::Ceil, &[a]))?,
            "round_z" => self.float1(ins, |s, a| s.b.math(Mf::Trunc, &[a]))?,
            "deriv_rtx" | "deriv_rty" | "deriv_rtx_coarse" | "deriv_rty_coarse" | "deriv_rtx_fine"
            | "deriv_rty_fine" => {
                let axis = if name.contains("rtx") {
                    DerivativeAxis::X
                } else {
                    DerivativeAxis::Y
                };
                let ctrl = if name.ends_with("coarse") {
                    DerivativeControl::Coarse
                } else if name.ends_with("fine") {
                    DerivativeControl::Fine
                } else {
                    DerivativeControl::None
                };
                self.float1(ins, |s, a| Val {
                    expr: s.b.app(E::Derivative {
                        axis,
                        ctrl,
                        expr: a.expr,
                    }),
                    kind: Kind::F,
                    width: a.width,
                })?
            }
            "sincos" => {
                let source = self.read_whole(&ops[2], Kind::F)?;
                if ops[0].register != Register::Null {
                    let a = self.lanes(source, &ops[0]);
                    let r = self.b.math(Mf::Sin, &[a]);
                    self.write(&ops[0], r, ins.saturate)?;
                }
                if ops[1].register != Register::Null {
                    let a = self.lanes(source, &ops[1]);
                    let r = self.b.math(Mf::Cos, &[a]);
                    self.write(&ops[1], r, ins.saturate)?;
                }
            }
            // swapc d0, d1, c, a, b: where c holds, d0 = b and d1 = a, else d0 = a and d1 = b; the sources read before
            // either destination is written
            "swapc" => {
                let c = self.read_whole(&ops[2], Kind::U)?;
                let a = self.read_whole(&ops[3], Kind::U)?;
                let b = self.read_whole(&ops[4], Kind::U)?;
                let zero = self.b.lit_u(0);
                let cond = self.b.binary(NotEqual, c, zero);
                let first = self.b.select(cond, b, a);
                let second = self.b.select(cond, a, b);
                for (destination, whole) in [(&ops[0], first), (&ops[1], second)] {
                    if destination.register != Register::Null {
                        let v = self.lanes(whole, destination);
                        self.write(destination, v, false)?;
                    }
                }
            }
            "eq" => self.compare(ins, Kind::F, Equal)?,
            "ne" => self.compare(ins, Kind::F, NotEqual)?,
            "lt" => self.compare(ins, Kind::F, Less)?,
            "ge" => self.compare(ins, Kind::F, GreaterEqual)?,
            "ieq" => self.compare(ins, Kind::U, Equal)?,
            "ine" => self.compare(ins, Kind::U, NotEqual)?,
            "ilt" => self.compare(ins, Kind::I, Less)?,
            "ige" => self.compare(ins, Kind::I, GreaterEqual)?,
            "ult" => self.compare(ins, Kind::U, Less)?,
            "uge" => self.compare(ins, Kind::U, GreaterEqual)?,
            "and" => self.int2(ins, Kind::U, |s, a, b| s.b.binary(And, a, b))?,
            "or" => self.int2(ins, Kind::U, |s, a, b| s.b.binary(InclusiveOr, a, b))?,
            "xor" => self.int2(ins, Kind::U, |s, a, b| s.b.binary(ExclusiveOr, a, b))?,
            "not" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::U)?;
                let r = self.b.unary(Uo::BitwiseNot, a);
                self.write(&ops[0], r, false)?;
            }
            "iadd" => self.int2(ins, Kind::U, |s, a, b| s.b.binary(Add, a, b))?,
            "ineg" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::I)?;
                let r = self.b.unary(Uo::Negate, a);
                self.write(&ops[0], r, false)?;
            }
            "imad" | "umad" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::U)?;
                let b = self.read(&ops[2], mask, Kind::U)?;
                let c = self.read(&ops[3], mask, Kind::U)?;
                let p = self.b.binary(Multiply, a, b);
                let r = self.b.binary(Add, p, c);
                self.write(&ops[0], r, false)?;
            }
            "imul" | "umul" => {
                let signed = name == "imul";
                let (a, b) = (
                    self.read_whole(&ops[2], Kind::U)?,
                    self.read_whole(&ops[3], Kind::U)?,
                );
                if ops[0].register != Register::Null {
                    let (a, b) = (self.lanes(a, &ops[0]), self.lanes(b, &ops[0]));
                    let mut high = self.b.mul_high_unsigned(a, b);
                    if signed {
                        // the signed high word is the unsigned one less each operand where the other is negative
                        let zero = self.b.lit_u(0);
                        let ai = self.b.bitcast(a, Kind::I);
                        let bi = self.b.bitcast(b, Kind::I);
                        let zi = self.b.lit_i(0);
                        let a_negative = self.b.binary(Less, ai, zi);
                        let b_negative = self.b.binary(Less, bi, zi);
                        let from_a = self.b.select(a_negative, b, zero);
                        let from_b = self.b.select(b_negative, a, zero);
                        high = self.b.binary(Subtract, high, from_a);
                        high = self.b.binary(Subtract, high, from_b);
                    }
                    if ops[1].register != Register::Null {
                        let (la, lb) = (self.lanes(a, &ops[1]), self.lanes(b, &ops[1]));
                        let low = self.b.binary(Multiply, la, lb);
                        self.write(&ops[0], high, false)?;
                        self.write(&ops[1], low, false)?;
                    } else {
                        self.write(&ops[0], high, false)?;
                    }
                } else if ops[1].register != Register::Null {
                    let (la, lb) = (self.lanes(a, &ops[1]), self.lanes(b, &ops[1]));
                    let low = self.b.binary(Multiply, la, lb);
                    self.write(&ops[1], low, false)?;
                }
            }
            "imin" | "imax" => {
                let f = if name == "imin" { Mf::Min } else { Mf::Max };
                self.int2(ins, Kind::I, move |s, a, b| s.b.math(f, &[a, b]))?
            }
            "umin" | "umax" => {
                let f = if name == "umin" { Mf::Min } else { Mf::Max };
                self.int2(ins, Kind::U, move |s, a, b| s.b.math(f, &[a, b]))?
            }
            "ishl" => self.int2(ins, Kind::U, |s, a, b| {
                let count = s.b.d3d_shift_count(b);
                s.b.binary(ShiftLeft, a, count)
            })?,
            "ushr" => self.int2(ins, Kind::U, |s, a, b| {
                let count = s.b.d3d_shift_count(b);
                s.b.binary(ShiftRight, a, count)
            })?,
            "ishr" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::I)?;
                let b = self.read(&ops[2], mask, Kind::U)?;
                let count = self.b.d3d_shift_count(b);
                let r = self.b.binary(ShiftRight, a, count);
                self.write(&ops[0], r, false)?;
            }
            "udiv" => {
                let (a, b) = (
                    self.read_whole(&ops[2], Kind::U)?,
                    self.read_whole(&ops[3], Kind::U)?,
                );
                let (quotient, remainder) = self.b.d3d_unsigned_divide(a, b);
                if ops[0].register != Register::Null {
                    let q = self.lanes(quotient, &ops[0]);
                    self.write(&ops[0], q, false)?;
                }
                if ops[1].register != Register::Null {
                    let r = self.lanes(remainder, &ops[1]);
                    self.write(&ops[1], r, false)?;
                }
            }
            "itof" | "utof" => {
                let kind = if name == "itof" { Kind::I } else { Kind::U };
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, kind)?;
                let r = self.b.convert(a, Kind::F);
                self.write(&ops[0], r, ins.saturate)?;
            }
            "ftoi" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::F)?;
                let r = self.b.d3d_float_to_int(a);
                self.write(&ops[0], r, false)?;
            }
            "ftou" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::F)?;
                let r = self.b.d3d_float_to_uint(a);
                self.write(&ops[0], r, false)?;
            }
            "countbits" | "bfrev" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::U)?;
                let f = if name == "countbits" {
                    Mf::CountOneBits
                } else {
                    Mf::ReverseBits
                };
                let r = self.b.math(f, &[a]);
                self.write(&ops[0], r, false)?;
            }
            "firstbit_hi" | "firstbit_shi" | "firstbit_lo" => self.first_bit(ins, name)?,
            "ubfe" | "ibfe" => self.extract_bits(ins, name == "ubfe")?,
            "bfi" => {
                let mask = self.dest_mask(&ops[0]);
                let width = self.read(&ops[1], mask, Kind::U)?;
                let offset = self.read(&ops[2], mask, Kind::U)?;
                let insert = self.read(&ops[3], mask, Kind::U)?;
                let base = self.read(&ops[4], mask, Kind::U)?;
                let width = self.b.d3d_shift_count(width);
                let offset = self.b.d3d_shift_count(offset);
                let one = self.b.lit_u(1);
                let bit = self.b.binary(ShiftLeft, one, width);
                let low = self.b.binary(Subtract, bit, one);
                let field = self.b.binary(ShiftLeft, low, offset);
                let outside = self.b.unary(Uo::BitwiseNot, field);
                let keep = self.b.binary(And, outside, base);
                let shifted = self.b.binary(ShiftLeft, insert, offset);
                let put = self.b.binary(And, shifted, field);
                let r = self.b.binary(InclusiveOr, keep, put);
                self.write(&ops[0], r, false)?;
            }
            "f32tof16" => {
                self.notes.insert(
                    "f32tof16 is written through pack2x16float: its rounding is the implementation's".into(),
                );
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::F)?;
                let mut parts = Vec::new();
                for i in 0..a.width {
                    let x = self.b.lane(a, i);
                    let z = self.b.lit_f(0.0);
                    let pair = self.b.compose(&[x, z]);
                    let packed = self.b.math(Mf::Pack2x16float, &[pair]);
                    let low = self.b.lit_u(0xffff);
                    parts.push(self.b.binary(And, packed, low));
                }
                let r = self.b.compose(&parts);
                self.write(&ops[0], r, false)?;
            }
            "f16tof32" => {
                let mask = self.dest_mask(&ops[0]);
                let a = self.read(&ops[1], mask, Kind::U)?;
                let mut parts = Vec::new();
                for i in 0..a.width {
                    let x = self.b.lane(a, i);
                    let low = self.b.lit_u(0xffff);
                    let x = self.b.binary(And, x, low);
                    let pair = self.b.math(Mf::Unpack2x16float, &[x]);
                    parts.push(self.b.lane(pair, 0));
                }
                let r = self.b.compose(&parts);
                self.write(&ops[0], r, false)?;
            }
            "uaddc" | "usubb" => {
                let subtract = name == "usubb";
                let (a, b) = (
                    self.read_whole(&ops[2], Kind::U)?,
                    self.read_whole(&ops[3], Kind::U)?,
                );
                let result = self.b.binary(if subtract { Subtract } else { Add }, a, b);
                // the carry, or the borrow
                let condition = if subtract {
                    self.b.binary(Less, a, b)
                } else {
                    self.b.binary(Less, result, a)
                };
                let one = self.b.lit_u(1);
                let zero = self.b.lit_u(0);
                let flag = self.b.select(condition, one, zero);
                if ops[0].register != Register::Null {
                    let r = self.lanes(result, &ops[0]);
                    self.write(&ops[0], r, false)?;
                }
                if ops[1].register != Register::Null {
                    let f = self.lanes(flag, &ops[1]);
                    self.write(&ops[1], f, false)?;
                }
            }
            // ---- flow
            "if" => {
                let c = self.condition(ins)?;
                self.frames.push(Frame {
                    kind: FrameKind::If,
                    condition: Some(c.expr),
                    accept: None,
                    selector: None,
                    cases: Vec::new(),
                    current: None,
                });
                self.b.blocks.push(Block::new());
            }
            "else" => {
                let accepted = self.b.blocks.pop().expect("an open block");
                let Some(frame) = self.frames.last_mut().filter(|f| f.kind == FrameKind::If) else {
                    return self.refuse("else without its if");
                };
                frame.accept = Some(accepted);
                self.b.blocks.push(Block::new());
            }
            "endif" => {
                let block = self.b.blocks.pop().expect("an open block");
                let Some(frame) = self.frames.pop().filter(|f| f.kind == FrameKind::If) else {
                    return self.refuse("endif without its if");
                };
                let (accept, reject) = match frame.accept {
                    Some(a) => (a, block),
                    None => (block, Block::new()),
                };
                self.b.push(Statement::If {
                    condition: frame.condition.expect("an if has a condition"),
                    accept,
                    reject,
                });
            }
            "loop" => {
                self.frames.push(Frame {
                    kind: FrameKind::Loop,
                    condition: None,
                    accept: None,
                    selector: None,
                    cases: Vec::new(),
                    current: None,
                });
                self.b.blocks.push(Block::new());
            }
            "endloop" => {
                let body = self.b.blocks.pop().expect("an open block");
                if self.frames.pop().is_none_or(|f| f.kind != FrameKind::Loop) {
                    return self.refuse("endloop without its loop");
                }
                self.b.push(Statement::Loop {
                    body,
                    continuing: Block::new(),
                    break_if: None,
                });
            }
            "break" => self.b.push(Statement::Break),
            "continue" => self.b.push(Statement::Continue),
            "breakc" | "continuec" => {
                let c = self.condition(ins)?;
                let mut accept = Block::new();
                accept.push(
                    if name == "breakc" {
                        Statement::Break
                    } else {
                        Statement::Continue
                    },
                    Span::UNDEFINED,
                );
                self.b.push(Statement::If {
                    condition: c.expr,
                    accept,
                    reject: Block::new(),
                });
            }
            "switch" => {
                let selector = self.read(&ops[0], 1, Kind::U)?;
                self.frames.push(Frame {
                    kind: FrameKind::Switch,
                    condition: None,
                    accept: None,
                    selector: Some(selector.expr),
                    cases: Vec::new(),
                    current: None,
                });
            }
            "case" | "default" => {
                self.close_case();
                let value = if name == "default" {
                    SwitchValue::Default
                } else {
                    SwitchValue::U32(ops[0].immediate[0])
                };
                let Some(frame) = self.frames.last_mut().filter(|f| f.kind == FrameKind::Switch) else {
                    return self.refuse("case outside a switch");
                };
                frame.current = Some(value);
                self.b.blocks.push(Block::new());
            }
            "endswitch" => {
                self.close_case();
                let Some(frame) = self.frames.pop().filter(|f| f.kind == FrameKind::Switch) else {
                    return self.refuse("endswitch without its switch");
                };
                let mut cases = frame.cases;
                match cases.iter().position(|c| c.value == SwitchValue::Default) {
                    Some(p) if p + 1 != cases.len() => {
                        return self.refuse("a switch whose default is not last");
                    }
                    None => cases.push(SwitchCase {
                        value: SwitchValue::Default,
                        body: Block::new(),
                        fall_through: false,
                    }),
                    _ => {}
                }
                cases.last_mut().expect("a default case").fall_through = false;
                self.b.push(Statement::Switch {
                    selector: frame.selector.expect("a switch has a selector"),
                    cases,
                });
            }
            "ret" if self.in_phase => {
                // the end of a hull phase: the function goes on with the next
                if !self.frames.is_empty() {
                    return self.refuse("a return inside the control flow of a hull phase");
                }
                self.returned = true;
            }
            "retc" if self.in_phase => return self.refuse("a conditional return in a hull phase"),
            "ret" => {
                if self.frames.is_empty() {
                    self.returned = true;
                }
                self.epilogue()?;
            }
            "retc" => {
                let c = self.condition(ins)?;
                let outer = std::mem::replace(&mut self.b.blocks, vec![Block::new()]);
                self.epilogue()?;
                let accept = self.b.blocks.pop().expect("a block");
                self.b.blocks = outer;
                self.b.push(Statement::If {
                    condition: c.expr,
                    accept,
                    reject: Block::new(),
                });
            }
            "discard" => {
                let c = self.condition(ins)?;
                let mut accept = Block::new();
                accept.push(Statement::Kill, Span::UNDEFINED);
                self.b.push(Statement::If {
                    condition: c.expr,
                    accept,
                    reject: Block::new(),
                });
            }
            // ---- resources
            "sample" | "sample_l" | "sample_b" | "sample_d" | "sample_c" | "sample_c_lz" | "gather4"
            | "gather4_c" => self.sample(ins)?,
            "ld" | "ld_ms" => self.load(ins)?,
            "resinfo" => self.resource_info(ins)?,
            "sample_info" => self.sample_info(ins)?,
            "ld_structured" => self.load_structured(ins)?,
            "ld_raw" => self.load_raw(ins)?,
            "ld_uav_typed" => {
                let slot = self.slot_of(ins, 2, &[Register::Uav])?;
                if self.resources.uav_buffers.contains_key(&slot) {
                    self.load_typed_buffer(ins)?
                } else {
                    self.load_view(ins)?
                }
            }
            "store_uav_typed" => {
                let slot = self.slot_of(ins, 0, &[Register::Uav])?;
                if self.resources.uav_buffers.contains_key(&slot) {
                    self.store_typed_buffer(ins)?
                } else {
                    self.store_view(ins)?
                }
            }
            "store_raw" => self.store_memory(ins, &[&ops[1]], &ops[2])?,
            "store_structured" => self.store_memory(ins, &[&ops[1], &ops[2]], &ops[3])?,
            "sync" => self.sync(ins),
            // a geometry program's vertices: only stream 0 is carried
            "emit" | "emit_stream" | "cut" | "cut_stream" | "emitthencut" | "emitthencut_stream" => {
                if let Some(stream) = ins.operands.first()
                    && stream.indices.first().is_some_and(|i| i.base != 0)
                {
                    return self.refuse("a vertex emitted to a stream other than 0");
                }
                if name.starts_with("emit") {
                    self.emit_vertex()?;
                }
                if name.starts_with("cut") || name.starts_with("emitthencut") {
                    self.cut_strip()?;
                }
            }
            "imm_atomic_alloc" | "imm_atomic_consume" => {
                return self.refuse("append and consume buffer counters are not lifted");
            }
            n if n.starts_with("atomic_") || n.starts_with("imm_atomic_") => self.atomic_instruction(ins)?,
            _ => return self.refuse("the instruction is not lifted"),
        }
        Ok(())
    }

    fn close_case(&mut self) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        let Some(value) = frame.current.take() else { return };
        let body = self.b.blocks.pop().expect("an open block");
        let ends_break = matches!(body.last(), Some(Statement::Break));
        let terminated = ends_break
            || matches!(
                body.last(),
                Some(Statement::Return { .. } | Statement::Continue | Statement::Kill)
            );
        let mut statements: Vec<(Statement, Span)> = body.span_into_iter().collect();
        if ends_break {
            statements.pop();
        }
        let mut block = Block::new();
        for (s, span) in statements {
            block.push(s, span);
        }
        self.frames
            .last_mut()
            .expect("the switch frame")
            .cases
            .push(SwitchCase {
                value,
                body: block,
                fall_through: !terminated,
            });
    }

    fn first_bit(&mut self, ins: &Instruction, name: &str) -> Result<()> {
        let ops = &ins.operands;
        let mask = self.dest_mask(&ops[0]);
        let a = self.read(&ops[1], mask, Kind::U)?;
        let zero = self.b.lit_u(0);
        let none = self.b.lit_u(u32::MAX);
        let r = match name {
            "firstbit_lo" => {
                let bit = self.b.math(Mf::FirstTrailingBit, &[a]);
                let is_zero = self.b.binary(Bo::Equal, a, zero);
                self.b.select(is_zero, none, bit)
            }
            "firstbit_hi" => {
                // counted from the most significant bit
                let leading = self.b.math(Mf::CountLeadingZeros, &[a]);
                let is_zero = self.b.binary(Bo::Equal, a, zero);
                self.b.select(is_zero, none, leading)
            }
            _ => {
                // signed: the first bit that differs from the sign bit
                let signed = self.b.bitcast(a, Kind::I);
                let zi = self.b.lit_i(0);
                let negative = self.b.binary(Bo::Less, signed, zi);
                let inverted = self.b.unary(Uo::BitwiseNot, a);
                let x = self.b.select(negative, inverted, a);
                let leading = self.b.math(Mf::CountLeadingZeros, &[x]);
                let is_zero = self.b.binary(Bo::Equal, x, zero);
                self.b.select(is_zero, none, leading)
            }
        };
        self.write(&ops[0], r, false)
    }

    /// `ubfe` and `ibfe`: extract `width` bits from bit `offset`, counts taken in the low five bits; where the field
    /// reaches past bit 31 the rest of the word is taken.
    fn extract_bits(&mut self, ins: &Instruction, unsigned: bool) -> Result<()> {
        use Bo::*;
        let ops = &ins.operands;
        let mask = self.dest_mask(&ops[0]);
        let width = self.read(&ops[1], mask, Kind::U)?;
        let offset = self.read(&ops[2], mask, Kind::U)?;
        let source = self.read(&ops[3], mask, Kind::U)?;
        let width = self.b.d3d_shift_count(width);
        let offset = self.b.d3d_shift_count(offset);
        let zero = self.b.lit_u(0);
        let c32 = self.b.lit_u(32);
        let sum = self.b.binary(Add, width, offset);
        let wide = self.b.binary(GreaterEqual, sum, c32);
        let empty = self.b.binary(Equal, width, zero);
        let r = if unsigned {
            let shifted = self.b.binary(ShiftRight, source, offset);
            let one = self.b.lit_u(1);
            let bit = self.b.binary(ShiftLeft, one, width);
            let field = self.b.binary(Subtract, bit, one);
            let narrow = self.b.binary(And, shifted, field);
            let r = self.b.select(wide, shifted, narrow);
            self.b.select(empty, zero, r)
        } else {
            let signed = self.b.bitcast(source, Kind::I);
            let shifted = self.b.binary(ShiftRight, signed, offset);
            // narrow: (source << (32 - width - offset)) >> (32 - width) arithmetically, with safe counts elsewhere
            let left = self.b.binary(Subtract, c32, sum);
            let safe_left = self.b.select(wide, zero, left);
            let right = self.b.binary(Subtract, c32, width);
            let safe_right = self.b.select(empty, zero, right);
            let moved = self.b.binary(ShiftLeft, source, safe_left);
            let moved = self.b.bitcast(moved, Kind::I);
            let narrow = self.b.binary(ShiftRight, moved, safe_right);
            let r = self.b.select(wide, shifted, narrow);
            let zi = self.b.lit_i(0);
            self.b.select(empty, zi, r)
        };
        self.write(&ops[0], r, false)
    }
}
