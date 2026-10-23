//! The `dx.op` intrinsics: signature reads and writes, math, bit operations, handles and the resource operations.

use naga::{
    BinaryOperator as Bo, DerivativeAxis, DerivativeControl, Expression as E, MathFunction as Mf, Statement,
};
use primase_shader::SystemValue;
use primase_shader::build::{Kind, Val};

use super::setup::{const_int, dx_opcode};
use super::{Lift, Lv};
use crate::Result;
use crate::module::{Instruction, ValueId};

impl Lift<'_> {
    pub(crate) fn call(&mut self, inst: &Instruction, function: ValueId, args: &[ValueId]) -> Result<()> {
        let Some(code) = dx_opcode(self.m, self.f, function, args) else {
            let name = self.m.name(function).unwrap_or("a function");
            // the compiler's lifetime markers of locals say nothing the program does
            if name.starts_with("llvm.lifetime.") {
                return Ok(());
            }
            return self.refuse(format!("a call of {name}"));
        };
        let result = inst.result;
        macro_rules! scalar {
            ($v:expr) => {
                self.define_result(result, Lv::Scalar($v))
            };
        }
        match code {
            4 => self.load_input(inst, args),
            5 => self.store_output(args),
            6 | 12..=20 | 22 | 24..=29 | 21 | 23 => {
                let x = self.float(args[1])?;
                let f = match code {
                    6 => Mf::Abs,
                    12 => Mf::Cos,
                    13 => Mf::Sin,
                    14 => Mf::Tan,
                    15 => Mf::Acos,
                    16 => Mf::Asin,
                    17 => Mf::Atan,
                    18 => Mf::Cosh,
                    19 => Mf::Sinh,
                    20 => Mf::Tanh,
                    21 => Mf::Exp2,
                    22 => Mf::Fract,
                    23 => Mf::Log2,
                    24 => Mf::Sqrt,
                    25 => Mf::InverseSqrt,
                    26 => Mf::Round,
                    27 => Mf::Floor,
                    28 => Mf::Ceil,
                    _ => Mf::Trunc,
                };
                let r = self.b.math(f, &[x]);
                scalar!(r)
            }
            7 => {
                let x = self.float(args[1])?;
                let r = self.b.d3d_saturate(x);
                scalar!(r)
            }
            8..=11 => {
                let x = self.float(args[1])?;
                let r = match code {
                    8 => self.b.is_nan(x),
                    _ => {
                        let bits = self.b.bitcast(x, Kind::U);
                        let magnitude_mask = self.b.lit_u(0x7fff_ffff);
                        let m = self.b.binary(Bo::And, bits, magnitude_mask);
                        let infinity = self.b.lit_u(0x7f80_0000);
                        match code {
                            9 => self.b.binary(Bo::Equal, m, infinity),
                            10 => self.b.binary(Bo::Less, m, infinity),
                            _ => {
                                let smallest = self.b.lit_u(0x0080_0000);
                                let big = self.b.binary(Bo::GreaterEqual, m, smallest);
                                let finite = self.b.binary(Bo::Less, m, infinity);
                                self.b.binary(Bo::LogicalAnd, big, finite)
                            }
                        }
                    }
                };
                scalar!(r)
            }
            31..=33 if self.is_wide(args[1]) => {
                let a = self.wide(args[1])?;
                let r = self.bits_wide(code, a)?;
                scalar!(r)
            }
            30 | 31 => {
                let x = self.uint(args[1])?;
                let r = self.b.math(
                    if code == 30 {
                        Mf::ReverseBits
                    } else {
                        Mf::CountOneBits
                    },
                    &[x],
                );
                scalar!(r)
            }
            32..=34 => {
                let a = self.uint(args[1])?;
                let zero = self.b.lit_u(0);
                let none = self.b.lit_u(u32::MAX);
                let r = match code {
                    32 => {
                        let bit = self.b.math(Mf::FirstTrailingBit, &[a]);
                        let is_zero = self.b.binary(Bo::Equal, a, zero);
                        self.b.select(is_zero, none, bit)
                    }
                    33 => {
                        let leading = self.b.math(Mf::CountLeadingZeros, &[a]);
                        let is_zero = self.b.binary(Bo::Equal, a, zero);
                        self.b.select(is_zero, none, leading)
                    }
                    _ => {
                        let signed = self.b.bitcast(a, Kind::I);
                        let zi = self.b.lit_i(0);
                        let negative = self.b.binary(Bo::Less, signed, zi);
                        let inverted = self.b.unary(naga::UnaryOperator::BitwiseNot, a);
                        let x = self.b.select(negative, inverted, a);
                        let leading = self.b.math(Mf::CountLeadingZeros, &[x]);
                        let is_zero = self.b.binary(Bo::Equal, x, zero);
                        self.b.select(is_zero, none, leading)
                    }
                };
                scalar!(r)
            }
            35 | 36 => {
                let (a, b) = (self.float(args[1])?, self.float(args[2])?);
                let r = self
                    .b
                    .d3d_min_max(if code == 35 { Mf::Max } else { Mf::Min }, a, b);
                scalar!(r)
            }
            37..=40 => {
                let (a, b) = (self.uint(args[1])?, self.uint(args[2])?);
                let signed = code <= 38;
                let f = if code % 2 == 1 { Mf::Max } else { Mf::Min };
                let r = if signed {
                    let (a, b) = (self.b.bitcast(a, Kind::I), self.b.bitcast(b, Kind::I));
                    let r = self.b.math(f, &[a, b]);
                    self.b.bitcast(r, Kind::U)
                } else {
                    self.b.math(f, &[a, b])
                };
                scalar!(r)
            }
            41 | 42 => {
                // {high, low} of the 64-bit product
                let (a, b) = (self.uint(args[1])?, self.uint(args[2])?);
                let low = self.b.binary(Bo::Multiply, a, b);
                let mut high = self.b.mul_high_unsigned(a, b);
                if code == 41 {
                    let zero = self.b.lit_u(0);
                    let (ai, bi) = (self.b.bitcast(a, Kind::I), self.b.bitcast(b, Kind::I));
                    let zi = self.b.lit_i(0);
                    let a_negative = self.b.binary(Bo::Less, ai, zi);
                    let b_negative = self.b.binary(Bo::Less, bi, zi);
                    let from_a = self.b.select(a_negative, b, zero);
                    let from_b = self.b.select(b_negative, a, zero);
                    high = self.b.binary(Bo::Subtract, high, from_a);
                    high = self.b.binary(Bo::Subtract, high, from_b);
                }
                self.define_result(result, Lv::Agg(vec![Lv::Scalar(high), Lv::Scalar(low)]))
            }
            43 => {
                let (a, b) = (self.uint(args[1])?, self.uint(args[2])?);
                let (q, r) = self.b.d3d_unsigned_divide(a, b);
                self.define_result(result, Lv::Agg(vec![Lv::Scalar(q), Lv::Scalar(r)]))
            }
            44 | 45 => {
                let (a, b) = (self.uint(args[1])?, self.uint(args[2])?);
                let subtract = code == 45;
                let value = self.b.binary(if subtract { Bo::Subtract } else { Bo::Add }, a, b);
                let condition = if subtract {
                    self.b.binary(Bo::Less, a, b)
                } else {
                    self.b.binary(Bo::Less, value, a)
                };
                let (one, zero) = (self.b.lit_u(1), self.b.lit_u(0));
                let flag = self.b.select(condition, one, zero);
                self.define_result(result, Lv::Agg(vec![Lv::Scalar(value), Lv::Scalar(flag)]))
            }
            46 => {
                // separately rounded multiply and add, as the DXBC lift's mad
                let (a, b, c) = (self.float(args[1])?, self.float(args[2])?, self.float(args[3])?);
                let p = self.b.binary(Bo::Multiply, a, b);
                let r = self.b.binary(Bo::Add, p, c);
                scalar!(r)
            }
            47 => {
                let (a, b, c) = (self.float(args[1])?, self.float(args[2])?, self.float(args[3])?);
                let r = self.b.math(Mf::Fma, &[a, b, c]);
                scalar!(r)
            }
            48 | 49 => {
                let (a, b, c) = (self.uint(args[1])?, self.uint(args[2])?, self.uint(args[3])?);
                let p = self.b.binary(Bo::Multiply, a, b);
                let r = self.b.binary(Bo::Add, p, c);
                scalar!(r)
            }
            54..=56 => {
                let n = (code - 52) as usize;
                let mut a = Vec::new();
                let mut b = Vec::new();
                for k in 0..n {
                    a.push(self.float(args[1 + k])?);
                    b.push(self.float(args[1 + n + k])?);
                }
                let (a, b) = (self.b.compose(&a), self.b.compose(&b));
                let r = self.b.math(Mf::Dot, &[a, b]);
                scalar!(r)
            }
            57 => self.create_handle(inst, args),
            216 => self.annotate_handle(inst, args),
            217 => self.create_handle_from_binding(inst, args),
            218 => self.create_handle_from_heap(inst, args),
            59 => self.cbuffer_load(inst, args),
            60..=65 | 73 | 74 => self.sample(inst, code, args),
            66 => self.texture_load(inst, args),
            67 => self.texture_store(args),
            68 | 139 => self.buffer_load(inst, args),
            72 => self.get_dimensions(inst, args),
            69 | 140 => self.buffer_store(args),
            70 => self.buffer_update_counter(inst, args),
            178 => self.allocate_ray_query(inst, args),
            179..=215 => self.ray_query_op(inst, code, args),
            103 => self.load_output_control_point(inst, args),
            104 => self.load_patch_constant(inst, args),
            105 => self.domain_location(inst, args),
            106 => self.store_patch_constant(args),
            107 => self.patch_id(inst, "SV_OutputControlPointID"),
            108 => self.patch_id(inst, "SV_PrimitiveID"),
            78 => self.buffer_atomic(inst, args),
            79 => self.buffer_compare_exchange(inst, args),
            80 => self.barrier(args),
            110..=123 | 135 | 136 => self.wave(inst, code, args),
            75 => self.sample_position(inst, args),
            // LegacyF32ToF16 and LegacyF16ToF32: the half in the low 16 bits of an integer
            130 => {
                self.notes.insert(
                    "f32tof16 is written through pack2x16float: its rounding is the implementation's".into(),
                );
                let x = self.float(args[1])?;
                let zero = self.b.lit_f(0.0);
                let pair = self.b.compose(&[x, zero]);
                let packed = self.b.math(naga::MathFunction::Pack2x16float, &[pair]);
                let low = self.b.lit_u(0xffff);
                let r = self.b.binary(naga::BinaryOperator::And, packed, low);
                scalar!(r)
            }
            131 => {
                let x = self.uint(args[1])?;
                let low = self.b.lit_u(0xffff);
                let x = self.b.binary(naga::BinaryOperator::And, x, low);
                let pair = self.b.math(naga::MathFunction::Unpack2x16float, &[x]);
                let r = self.b.lane(pair, 0);
                scalar!(r)
            }
            90 => {
                let index = self.system_input(SystemValue::SampleIndex)?;
                scalar!(index)
            }
            // ThreadId, GroupId, ThreadIdInGroup (a component of the id), FlattenedThreadIdInGroup
            93..=95 => {
                let system = match code {
                    93 => SystemValue::DispatchThreadId,
                    94 => SystemValue::GroupId,
                    _ => SystemValue::GroupThreadId,
                };
                let id = self.system_input(system)?;
                let Some(component) = const_int(self.m, self.f, args[1]).filter(|c| (0..3).contains(c))
                else {
                    return self.refuse("an invocation id read at a computed component");
                };
                let lane = self.b.lane(id, component as u8);
                scalar!(lane)
            }
            96 => {
                let index = self.system_input(SystemValue::GroupIndex)?;
                scalar!(index)
            }
            82 => {
                let c = self.scalar(args[1])?;
                let c = self.b.bitcast_to(c, Kind::B);
                let mut accept = naga::Block::new();
                accept.push(Statement::Kill, naga::Span::UNDEFINED);
                self.b.push(Statement::If {
                    condition: c.expr,
                    accept,
                    reject: naga::Block::new(),
                });
                Ok(())
            }
            83..=86 => {
                let x = self.float(args[1])?;
                let (axis, ctrl) = match code {
                    83 => (DerivativeAxis::X, DerivativeControl::Coarse),
                    84 => (DerivativeAxis::Y, DerivativeControl::Coarse),
                    85 => (DerivativeAxis::X, DerivativeControl::Fine),
                    _ => (DerivativeAxis::Y, DerivativeControl::Fine),
                };
                let r = Val {
                    expr: self.b.app(E::Derivative {
                        axis,
                        ctrl,
                        expr: x.expr,
                    }),
                    kind: Kind::F,
                    width: 1,
                };
                scalar!(r)
            }
            other => {
                let name = self.m.name(function).unwrap_or("dx.op");
                self.refuse(format!("{name}: dx.op {other} is not lifted"))
            }
        }
    }

    /// `LoadInput(element, row, column)`: one lane of an input element as the call's type.
    fn load_input(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let id = const_int(self.m, self.f, args[1]).expect("checked in the setup");
        let element = self
            .setup
            .input_elements
            .iter()
            .find(|e| i64::from(e.id) == id)
            .cloned()
            .expect("checked in the setup");
        let (Some(row), Some(col)) = (
            const_int(self.m, self.f, args[2]),
            const_int(self.m, self.f, args[3]),
        ) else {
            return self.refuse("an input read at a computed row or column");
        };
        let register = element.row + row as u32;
        let lane = element.column + col as u8;
        let Some((varying, position)) = self.setup.input_cell(register, lane) else {
            return self.refuse(format!(
                "input {} has no lane at register {register}",
                element.name
            ));
        };
        let (index, ty) = self.input_argument(varying)?;
        let expr = self.argument_expr(index);
        // a control point is one element of an array argument: the one the call's vertex index names
        let (expr, carried) = match ty {
            primase_shader::IoType::Value(carried) => (expr, carried),
            primase_shader::IoType::Array(carried, _) => {
                let point = self.uint(args[4])?;
                (
                    self.b.app(E::Access {
                        base: expr,
                        index: point.expr,
                    }),
                    carried,
                )
            }
            primase_shader::IoType::ClipDistances(_) => return self.refuse("an array-typed input"),
        };
        let value = Val {
            expr,
            kind: Kind::of(carried.scalar),
            width: carried.width,
        };
        let lane_value = if carried.width > 1 {
            self.b.lane(value, position)
        } else {
            value
        };
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let kind = self.kind_of_type(ty)?;
        let r = self.b.bitcast_to(lane_value, kind);
        self.define_result(inst.result, Lv::Scalar(r))
    }

    /// The lane number of a thread group laid out in more than one dimension, which WebGPU gives no built-in for
    /// (naga rejects `subgroup_invocation_id` there): the rank of the invocation among the subgroup's invocations at
    /// the start of the program. At the start no invocation has diverged or returned, so every invocation of the
    /// subgroup that exists is active, and the exclusive count of the active invocations below this one is its index in
    /// the subgroup. D3D leaves the numbering of a wave's lanes to the hardware just as WebGPU leaves a subgroup's
    /// order to the device, so the subgroup's own order is a lane numbering as valid as any. Compute has no helper
    /// invocations. The one assumption: a subgroup the thread group fills only in part has its existing invocations at
    /// the lowest lanes, as every device that fills subgroups in order does.
    pub(super) fn lane_at_start(&mut self) {
        if !self.setup.lane_by_rank {
            return;
        }
        self.notes.insert(
            "the lane number of a thread group laid out in more than one dimension (WebGPU gives such a group no \
             lane-number built-in) is the invocation's rank among its subgroup's invocations when the program starts: \
             every invocation that exists is active then, so the exclusive count of active invocations is the index in \
             the subgroup, which is as valid a numbering as the hardware's own that D3D leaves open; a subgroup the \
             thread group fills only in part is assumed to have its invocations at the lowest lanes"
                .into(),
        );
        let one = self.b.lit_u(1);
        let rank = self.collective(
            naga::SubgroupOperation::Add,
            naga::CollectiveOperation::ExclusiveScan,
            one,
        );
        let ty = self.b.ty_of(Kind::U, 1);
        let local = self.b.local("lane", ty);
        let p = self.b.local_ptr(local);
        self.b.store(p, rank);
        self.lane = Some(local);
    }

    /// The value of the input varying that carries a system value the signature does not list.
    pub(super) fn system_input(&mut self, system: SystemValue) -> Result<Val> {
        if system == SystemValue::LaneIndex
            && let Some(lane) = self.lane
        {
            let p = self.b.local_ptr(lane);
            return Ok(self.b.load(p, Kind::U, 1));
        }
        let io = self.setup.io.clone();
        let Some(index) = io
            .arguments
            .iter()
            .position(|item| self.setup.interface.inputs[item.varyings[0]].system == Some(system))
        else {
            return self.refuse(format!("{system:?} has no input"));
        };
        let primase_shader::IoType::Value(carried) = io.arguments[index].ty else {
            return self.refuse("an array-typed input");
        };
        let expr = self.argument_expr(index);
        Ok(Val {
            expr,
            kind: Kind::of(carried.scalar),
            width: carried.width,
        })
    }

    /// `StoreOutput(element, row, column, value)`: one lane of an output element, kept as raw bits.
    fn store_output(&mut self, args: &[ValueId]) -> Result<()> {
        let id = const_int(self.m, self.f, args[1]).expect("checked in the setup");
        let element = self
            .setup
            .output_elements
            .iter()
            .find(|e| i64::from(e.id) == id)
            .cloned()
            .expect("checked in the setup");
        let value = self.scalar(args[4])?;
        let bits = self.b.bitcast_to(value, Kind::U);
        let (local, lane) = if element.placed {
            let (Some(row), Some(col)) = (
                const_int(self.m, self.f, args[2]),
                const_int(self.m, self.f, args[3]),
            ) else {
                return self.refuse("an output written at a computed row or column");
            };
            let register = element.row + row as u32;
            let lane = element.column + col as u8;
            if self.setup.output_cell(register, lane).is_none() {
                return self.refuse(format!(
                    "output {} has no lane at register {register}",
                    element.name
                ));
            }
            (self.outputs[&register], lane)
        } else {
            let local = match element.system_value {
                17..=19 => self.depth,
                14 => self.coverage,
                _ => None,
            };
            match local {
                Some(l) => (l, 0),
                None => return self.refuse(format!("the unplaced output {}", element.name)),
            }
        };
        let pointer = self.b.local_ptr(local);
        let target = self.b.app(E::AccessIndex {
            base: pointer,
            index: u32::from(lane),
        });
        self.b.store(target, bits);
        Ok(())
    }

    /// Pack the output registers into the result and return it.
    pub(super) fn epilogue(&mut self) -> Result<()> {
        if self.sig.is_some() {
            return self.function_epilogue();
        }
        let Some(result) = self.result_type else {
            self.b.push(Statement::Return { value: None });
            return Ok(());
        };
        let io = self.setup.io.clone();
        let mut parts = Vec::new();
        for item in &io.results {
            let part = match item.ty {
                primase_shader::IoType::Array(..) => return self.refuse("an array-typed output"),
                primase_shader::IoType::ClipDistances(n) => {
                    let mut components = Vec::new();
                    for &i in &item.varyings {
                        let v = self.setup.interface.outputs[i].clone();
                        let d3d = v.provenance.d3d.expect("a clip distance has a register");
                        let local = self.outputs[&d3d.register];
                        let p = self.b.local_ptr(local);
                        let row = self.b.load(p, Kind::U, 4);
                        for lane in (0..4u8).filter(|l| d3d.mask >> l & 1 != 0) {
                            let c = self.b.lane(row, lane);
                            components.push(self.b.bitcast(c, Kind::F).expr);
                        }
                    }
                    let f = self.b.ty_of(Kind::F, 1);
                    let ty = self.b.ty(naga::TypeInner::Array {
                        base: f,
                        size: naga::ArraySize::Constant(
                            std::num::NonZeroU32::new(n).expect("clip distances"),
                        ),
                        stride: 4,
                    });
                    self.b.app(E::Compose { ty, components })
                }
                primase_shader::IoType::Value(carried) => {
                    let v = self.setup.interface.outputs[item.varyings[0]].clone();
                    let (local, lanes): (_, Vec<u8>) = match v.system {
                        Some(
                            SystemValue::Depth | SystemValue::DepthGreaterEqual | SystemValue::DepthLessEqual,
                        ) => (self.depth.expect("a depth output has its local"), vec![0]),
                        Some(SystemValue::Coverage) => {
                            (self.coverage.expect("a coverage output has its local"), vec![0])
                        }
                        Some(SystemValue::ClipPosition | SystemValue::Target(_)) => {
                            let d3d = v.provenance.d3d.expect("a signature element has a register");
                            (self.outputs[&d3d.register], vec![0, 1, 2, 3])
                        }
                        _ => {
                            let d3d = v.provenance.d3d.expect("a signature element has a register");
                            (
                                self.outputs[&d3d.register],
                                (0..4u8).filter(|l| d3d.mask >> l & 1 != 0).collect(),
                            )
                        }
                    };
                    let p = self.b.local_ptr(local);
                    let row = self.b.load(p, Kind::U, 4);
                    let picked = self.b.swizzle(row, &lanes);
                    let value = self.b.bitcast(picked, Kind::of(carried.scalar));
                    value.expr
                }
            };
            parts.push(part);
        }
        let composed = self.b.app(E::Compose {
            ty: result,
            components: parts,
        });
        self.b.push(Statement::Return {
            value: Some(composed),
        });
        Ok(())
    }
}
