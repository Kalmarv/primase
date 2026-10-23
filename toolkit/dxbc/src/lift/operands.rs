//! Reading and writing DXBC operands: registers as four raw lanes, source swizzles and modifiers, write masks.

use naga::{Expression as E, MathFunction as Mf, UnaryOperator as Uo};
use primase_shader::build::{Kind, Val};

use super::Lift;
use super::resources::lanes_of;
use crate::Result;
use crate::decode::{Index, Operand, Register, Selection};

impl Lift<'_> {
    /// The index of a register operand: its base plus the relative register, if any.
    pub(super) fn index_expr(&mut self, ix: &Index) -> Result<Val> {
        let base = self.b.lit_u(ix.base);
        match &ix.relative {
            None => Ok(base),
            Some(r) => {
                let relative = self.read(r, 1, Kind::U)?;
                Ok(self.b.binary(naga::BinaryOperator::Add, base, relative))
            }
        }
    }

    /// The `dcl_index_range` of input or output registers that holds register `base`: its first register and count.
    fn index_range(&self, output: bool, base: u32) -> Option<(u32, u32)> {
        self.setup
            .index_ranges
            .iter()
            .find(|(o, first, count)| *o == output && (*first..first.saturating_add(*count)).contains(&base))
            .map(|(_, first, count)| (*first, *count))
    }

    /// A register addressed by a computed index: the registers of its `dcl_index_range` as one array, indexed from the
    /// range's first; zero outside the range, as D3D reads past a declared one.
    fn relative_register(&mut self, op: &Operand, output: bool) -> Result<Val> {
        let Some((first, count)) = self.index_range(output, op.indices[0].base) else {
            return self.refuse("a register addressed by a computed index outside any dcl_index_range");
        };
        let index = self.index_expr(&op.indices[0].clone())?;
        let u4 = self.u4();
        let len = std::num::NonZeroU32::new(count).expect("a range holds registers");
        let ty = self.b.ty(naga::TypeInner::Array {
            base: u4,
            size: naga::ArraySize::Constant(len),
            stride: 16,
        });
        // the range's registers as a local array the index can address
        let array = self.b.local("range", ty);
        for (k, r) in (first..first + count).enumerate() {
            let value = match (if output {
                self.outputs.get(&r)
            } else {
                self.inputs.get(&r)
            })
            .copied()
            {
                Some(l) => {
                    let p = self.b.local_ptr(l);
                    self.b.load(p, Kind::U, 4)
                }
                None => {
                    let z = self.b.lit_u(0);
                    self.b.splat(z, 4)
                }
            };
            let pointer = self.b.local_ptr(array);
            let target = self.b.app(E::AccessIndex {
                base: pointer,
                index: k as u32,
            });
            self.b.store(target, value);
        }
        let start = self.b.lit_u(first);
        let offset = self.b.binary(naga::BinaryOperator::Subtract, index, start);
        let length = self.b.lit_u(count);
        let inside = self.b.binary(naga::BinaryOperator::Less, offset, length);
        let zero = self.b.lit_u(0);
        let safe = self.b.select(inside, offset, zero);
        let pointer = self.b.local_ptr(array);
        let slot = self.b.app(E::Access {
            base: pointer,
            index: safe.expr,
        });
        let element = self.b.load(slot, Kind::U, 4);
        let none = self.b.splat(zero, 4);
        Ok(self.b.select(inside, element, none))
    }

    /// The register's four raw lanes as a `vec4<u32>`.
    fn reg_value(&mut self, op: &Operand) -> Result<Val> {
        match op.register {
            Register::Temp => {
                let Some(&local) = self.temps.get(op.indices[0].base as usize) else {
                    return self.refuse("a temporary beyond dcl_temps");
                };
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, Kind::U, 4))
            }
            // a hull, domain or geometry program's inputs are indexed by control point (or vertex) first
            Register::Input | Register::InputControlPoint
                if op.indices.len() == 2
                    && matches!(
                        self.stage,
                        primase_shader::Stage::Hull
                            | primase_shader::Stage::Domain
                            | primase_shader::Stage::Geometry
                    ) =>
            {
                self.control_point_register(op, false)
            }
            Register::OutputControlPoint => self.control_point_register(op, true),
            Register::InputPatchConstant => self.patch_constant_register(op),
            Register::InputDomainPoint
            | Register::OutputControlPointId
            | Register::GsInstanceId
            | Register::ForkInstanceId
            | Register::JoinInstanceId => self.id_register(op.register),
            Register::Input => {
                if op.indices[0].relative.is_some() {
                    return self.relative_register(op, false);
                }
                let register = op.indices[0].base;
                let Some(&local) = self.inputs.get(&register) else {
                    return self.refuse(format!("input register v{register} is not in the signature"));
                };
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, Kind::U, 4))
            }
            Register::Output => {
                if op.indices[0].relative.is_some() {
                    return self.relative_register(op, true);
                }
                let register = op.indices[0].base;
                let Some(&local) = self.outputs.get(&register) else {
                    return self.refuse(format!("output register o{register} is read but not declared"));
                };
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, Kind::U, 4))
            }
            Register::IndexableTemp => {
                let Some(&(local, _)) = self.indexable.get(&op.indices[0].base) else {
                    return self.refuse("an undeclared indexable temporary");
                };
                let index = self.index_expr(&op.indices[1].clone())?;
                let p = self.b.local_ptr(local);
                let element = self.b.app(E::Access {
                    base: p,
                    index: index.expr,
                });
                Ok(self.b.load(element, Kind::U, 4))
            }
            Register::Constant => {
                let slot = op.indices[0].base;
                let Some(&(g, rows)) = self.resources.constant_buffers.get(&slot) else {
                    return self.refuse(format!("cb{slot} is used but not declared"));
                };
                let index = self.index_expr(&op.indices[1].clone())?;
                if op.indices[1].relative.is_some() {
                    return Ok(self.guarded_load(g, index, Some(rows), Kind::U, 4));
                }
                let p = self.b.global_ptr(g);
                let element = self.b.app(E::Access {
                    base: p,
                    index: index.expr,
                });
                Ok(self.b.load(element, Kind::U, 4))
            }
            Register::ImmediateBuffer => {
                let Some((g, rows)) = self.resources.immediate else {
                    return self.refuse("the immediate constant buffer is used but not declared");
                };
                let index = self.index_expr(&op.indices[0].clone())?;
                if op.indices[0].relative.is_some() {
                    return Ok(self.guarded_load(g, index, Some(rows), Kind::U, 4));
                }
                let p = self.b.global_ptr(g);
                let element = self.b.app(E::Access {
                    base: p,
                    index: index.expr,
                });
                Ok(self.b.load(element, Kind::U, 4))
            }
            Register::PrimitiveId | Register::InputCoverage => {
                let local = if op.register == Register::PrimitiveId {
                    self.primitive_id
                } else {
                    self.input_coverage
                };
                let Some(local) = local else {
                    return self.refuse(format!("{:?} is used but not declared", op.register));
                };
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, Kind::U, 4))
            }
            Register::ThreadId
            | Register::ThreadGroupId
            | Register::ThreadIdInGroup
            | Register::ThreadIdInGroupFlattened => {
                let system = match op.register {
                    Register::ThreadId => primase_shader::SystemValue::DispatchThreadId,
                    Register::ThreadGroupId => primase_shader::SystemValue::GroupId,
                    Register::ThreadIdInGroup => primase_shader::SystemValue::GroupThreadId,
                    _ => primase_shader::SystemValue::GroupIndex,
                };
                let Some(&local) = self.ids.get(&system) else {
                    return self.refuse(format!("{:?} is used but not declared", op.register));
                };
                let p = self.b.local_ptr(local);
                Ok(self.b.load(p, Kind::U, 4))
            }
            other => self.refuse(format!("a source operand of register type {other:?}")),
        }
    }

    pub(super) fn dest_mask(&self, op: &Operand) -> u8 {
        match op.selection {
            Selection::Mask(m) => m,
            _ => 1,
        }
    }

    pub(super) fn read(&mut self, op: &Operand, mask: u8, kind: Kind) -> Result<Val> {
        self.read_lanes(op, &lanes_of(mask), kind)
    }

    /// The source for the destination lanes `lanes` (the swizzle is indexed by destination lane), as `kind`.
    pub(super) fn read_lanes(&mut self, op: &Operand, lanes: &[u8], kind: Kind) -> Result<Val> {
        let components: Vec<u8> = lanes
            .iter()
            .map(|&l| match op.selection {
                Selection::Swizzle(s) => Ok(s[l as usize]),
                Selection::Scalar(c) => Ok(c),
                Selection::Mask(_) if op.register == Register::Immediate => Ok(l),
                _ => Err(()),
            })
            .collect::<std::result::Result<_, _>>()
            .or_else(|()| self.refuse("a numeric source without a swizzle"))?;
        if op.precision != 0 {
            self.notes
                .insert("min-precision operands are computed at 32 bits".into());
        }
        let mut value = if op.register == Register::Immediate {
            let parts: Vec<Val> = components
                .iter()
                .map(|&c| self.b.lit_bits(kind, op.immediate[c as usize]))
                .collect();
            self.b.compose(&parts)
        } else {
            let base = self.reg_value(op)?;
            let picked = self.b.swizzle(base, &components);
            self.b.bitcast(picked, kind)
        };
        value = match (op.modifier, kind) {
            (0, _) => value,
            (1, Kind::F | Kind::I) => self.b.unary(Uo::Negate, value),
            (1, Kind::U) => {
                let signed = self.b.bitcast(value, Kind::I);
                let negated = self.b.unary(Uo::Negate, signed);
                self.b.bitcast(negated, Kind::U)
            }
            (2, Kind::F | Kind::I) => self.b.math(Mf::Abs, &[value]),
            (3, Kind::F) => {
                let magnitude = self.b.math(Mf::Abs, &[value]);
                self.b.unary(Uo::Negate, magnitude)
            }
            (m, k) => return self.refuse(format!("source modifier {m} on a {k:?} operand")),
        };
        Ok(value)
    }

    /// Write `v` to the destination operand's masked lanes; `saturate` clamps a float result first.
    pub(super) fn write(&mut self, op: &Operand, v: Val, saturate: bool) -> Result<()> {
        if op.register == Register::Null {
            return Ok(());
        }
        let mask = self.dest_mask(op);
        let lanes = lanes_of(mask);
        let mut v = v;
        if v.width == 1 && lanes.len() > 1 {
            v = self.b.splat(v, lanes.len() as u8);
        }
        assert_eq!(
            usize::from(v.width),
            lanes.len(),
            "a result as wide as its destination mask (a lifter bug)"
        );
        if saturate {
            if v.kind != Kind::F {
                return self.refuse("saturation of a result that is not a float");
            }
            v = self.b.d3d_saturate(v);
        }
        let v = self.b.bitcast(v, Kind::U);
        let pointer = match op.register {
            Register::Temp => {
                let Some(&local) = self.temps.get(op.indices[0].base as usize) else {
                    return self.refuse("a temporary beyond dcl_temps");
                };
                self.b.local_ptr(local)
            }
            Register::Output => {
                if op.indices[0].relative.is_some() {
                    // a register of the range the index names: each is written where the index is its number
                    let Some((first, count)) = self.index_range(true, op.indices[0].base) else {
                        return self
                            .refuse("an output addressed by a computed index outside any dcl_index_range");
                    };
                    let index = self.index_expr(&op.indices[0].clone())?;
                    for r in first..first + count {
                        let Some(&local) = self.outputs.get(&r) else {
                            continue;
                        };
                        let number = self.b.lit_u(r);
                        let here = self.b.binary(naga::BinaryOperator::Equal, index, number);
                        let lanes = lanes.clone();
                        self.b.when(here, |b| {
                            let pointer = b.local_ptr(local);
                            for (j, lane) in lanes.iter().enumerate() {
                                let component = b.lane(v, j as u8);
                                let target = b.app(E::AccessIndex {
                                    base: pointer,
                                    index: u32::from(*lane),
                                });
                                b.store(target, component);
                            }
                        });
                    }
                    return Ok(());
                }
                let register = op.indices[0].base;
                let Some(&local) = self.outputs.get(&register) else {
                    return self.refuse(format!("a write to o{register}, which is not declared"));
                };
                self.b.local_ptr(local)
            }
            Register::IndexableTemp => {
                let Some(&(local, _)) = self.indexable.get(&op.indices[0].base) else {
                    return self.refuse("an undeclared indexable temporary");
                };
                let index = self.index_expr(&op.indices[1].clone())?;
                let p = self.b.local_ptr(local);
                self.b.app(E::Access {
                    base: p,
                    index: index.expr,
                })
            }
            Register::OutputDepth | Register::OutputDepthGreaterEqual | Register::OutputDepthLessEqual => {
                let Some(local) = self.depth else {
                    return self.refuse("the depth output is written but not declared");
                };
                self.b.local_ptr(local)
            }
            Register::OutputCoverage => {
                let Some(local) = self.coverage else {
                    return self.refuse("the coverage output is written but not declared");
                };
                self.b.local_ptr(local)
            }
            other => return self.refuse(format!("a destination operand of register type {other:?}")),
        };
        if mask == 15 {
            self.b.store(pointer, v);
        } else {
            for (j, lane) in lanes.iter().enumerate() {
                let component = self.b.lane(v, j as u8);
                let target = self.b.app(E::AccessIndex {
                    base: pointer,
                    index: u32::from(*lane),
                });
                self.b.store(target, component);
            }
        }
        Ok(())
    }
}
