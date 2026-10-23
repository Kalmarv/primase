//! The ray tracing pipeline's programs (ray generation, intersection, any-hit, closest-hit, miss, callable), which a
//! library holds: the ray's system values a program reads, and the operations only the pipeline has (`TraceRay`,
//! `ReportHit`, `CallShader`, `IgnoreHit`, `AcceptHitAndEndSearch`), which are calls of the module's functions named by
//! [`primase_shader::rt`].

use std::collections::BTreeMap;

use naga::{AddressSpace, Expression as E, FunctionArgument, Handle, Statement, TypeInner};
use primase_shader::SystemValue;
use primase_shader::build::{Kind, Val};

use super::setup::{const_int, ray_system_value};
use super::{Lift, Lv};
use crate::Result;
use crate::module::{Instruction, Type, TypeId, ValueId};

/// The pipeline's operations the program has called, by name and the structure they take a pointer to: the module's
/// bodiless functions.
#[derive(Default)]
pub(super) struct Operations {
    made: BTreeMap<(&'static str, Option<Handle<naga::Type>>), Handle<naga::Function>>,
}

impl Lift<'_> {
    /// The type of the structure a pointer value points to (the payload of a ray, the attributes of a hit).
    fn pointee(&self, v: ValueId) -> Result<TypeId> {
        let ty = if v < self.f.first {
            self.m.value_types.get(v).copied()
        } else {
            self.f.local_types.get(v - self.f.first).copied()
        };
        match ty.and_then(|t| self.m.types.get(t)) {
            Some(Type::Pointer(t)) => Ok(*t),
            _ => self.refuse("a ray tracing operation on something that is not a pointer to a structure"),
        }
    }

    /// The function that stands for `name` on a pointer to `structure`, made with these arguments the first time.
    fn operation(
        &mut self,
        name: &'static str,
        structure: Option<Handle<naga::Type>>,
        arguments: impl FnOnce(&mut Self) -> Vec<(&'static str, Handle<naga::Type>)>,
        returns_bool: bool,
    ) -> Handle<naga::Function> {
        if let Some(&f) = self.operations.made.get(&(name, structure)) {
            return f;
        }
        let arguments = arguments(self)
            .into_iter()
            .map(|(name, ty)| FunctionArgument {
                name: Some(name.to_owned()),
                ty,
                binding: None,
            })
            .collect();
        let f = self.b.declare_operation(name, arguments, returns_bool);
        self.operations.made.insert((name, structure), f);
        f
    }

    /// A pointer to a structure of function memory, as an argument type.
    fn pointer_type(&mut self, structure: Handle<naga::Type>) -> Handle<naga::Type> {
        self.b.ty(TypeInner::Pointer {
            base: structure,
            space: AddressSpace::Function,
        })
    }

    /// A pointer value of the program as the expression it is, and the naga structure it points to.
    fn memory_pointer(&mut self, v: ValueId) -> Result<(Handle<E>, Handle<naga::Type>)> {
        let structure = self.pointee(v)?;
        let (structure, _) = self.naga_type(structure)?;
        let Lv::Pointer(p) = self.get(v)? else {
            return self.refuse("a ray tracing operation on something that is not a pointer");
        };
        Ok((self.pointer_expr(&p)?, structure))
    }

    fn float3(&mut self, args: &[ValueId], first: usize) -> Result<Val> {
        let parts = [
            self.float(args[first])?,
            self.float(args[first + 1])?,
            self.float(args[first + 2])?,
        ];
        Ok(self.b.compose(&parts))
    }

    fn call_operation(
        &mut self,
        function: Handle<naga::Function>,
        arguments: Vec<Handle<E>>,
        result: Option<Handle<E>>,
    ) {
        self.b.push(Statement::Call {
            function,
            arguments,
            result,
        });
    }

    /// The ray tracing `dx.op`s: the system values of the ray (141 to 154, 161, 213) and the pipeline's operations
    /// (155 to 159).
    pub(super) fn ray_op(&mut self, inst: &Instruction, code: u64, args: &[ValueId]) -> Result<()> {
        let result = inst.result;
        match code {
            // ObjectToWorld(row, column), WorldToObject(row, column): a lane of a row of the instance's transform
            151 | 152 => {
                let (Some(row), Some(column)) = (
                    const_int(self.m, self.f, args[1]),
                    const_int(self.m, self.f, args[2]),
                ) else {
                    return self.refuse("a transform read at a computed row or column");
                };
                if !(0..3).contains(&row) || !(0..4).contains(&column) {
                    return self.refuse("a transform read outside its 3 by 4");
                }
                let system = if code == 151 {
                    SystemValue::ObjectToWorld
                } else {
                    SystemValue::WorldToObject
                };
                let rows = self.system_input_row(system, row as u32)?;
                let lane = self.b.lane(rows, column as u8);
                self.define_result(result, Lv::Scalar(lane))
            }
            155 | 156 => {
                let name = if code == 155 {
                    primase_shader::rt::IGNORE_HIT
                } else {
                    primase_shader::rt::ACCEPT_HIT_AND_END_SEARCH
                };
                let f = self.operation(name, None, |_| Vec::new(), false);
                self.call_operation(f, Vec::new(), None);
                // the program ends
                self.b.push(Statement::Return { value: None });
                Ok(())
            }
            157 => {
                // TraceRay(handle, flags, mask, hit offset, hit stride, miss index, origin, t min, direction, t max,
                // payload)
                let (_, structure) = self.handle_expr(args[1], 0)?;
                let acceleration = structure;
                let (payload, payload_type) = self.memory_pointer(args[15])?;
                let flags = self.uint(args[2])?;
                let mask = self.uint(args[3])?;
                let offset = self.uint(args[4])?;
                let stride = self.uint(args[5])?;
                let miss = self.uint(args[6])?;
                let origin = self.float3(args, 7)?;
                let tmin = self.float(args[10])?;
                let direction = self.float3(args, 11)?;
                let tmax = self.float(args[14])?;
                let f = self.operation(
                    primase_shader::rt::TRACE_RAY,
                    Some(payload_type),
                    |lift| {
                        let accel = lift
                            .b
                            .ty(TypeInner::AccelerationStructure { vertex_return: false });
                        let u = lift.b.ty_of(Kind::U, 1);
                        let f = lift.b.ty_of(Kind::F, 1);
                        let v3 = lift.b.ty_of(Kind::F, 3);
                        let pointer = lift.pointer_type(payload_type);
                        vec![
                            ("acceleration_structure", accel),
                            ("ray_flags", u),
                            ("instance_mask", u),
                            ("hit_group_offset", u),
                            ("hit_group_stride", u),
                            ("miss_index", u),
                            ("origin", v3),
                            ("t_min", f),
                            ("direction", v3),
                            ("t_max", f),
                            ("payload", pointer),
                        ]
                    },
                    false,
                );
                self.call_operation(
                    f,
                    vec![
                        acceleration,
                        flags.expr,
                        mask.expr,
                        offset.expr,
                        stride.expr,
                        miss.expr,
                        origin.expr,
                        tmin.expr,
                        direction.expr,
                        tmax.expr,
                        payload,
                    ],
                    None,
                );
                self.notes.insert(
                    "a ray tracing program calls the pipeline's operations (trace_ray, report_hit, call_shader, \
                     ignore_hit, accept_hit_and_end_search) as functions without a body: it is read, not run"
                        .into(),
                );
                Ok(())
            }
            158 => {
                // ReportHit(t, kind, attributes) -> accepted
                let (attributes, attribute_type) = self.memory_pointer(args[3])?;
                let t = self.float(args[1])?;
                let kind = self.uint(args[2])?;
                let f = self.operation(
                    primase_shader::rt::REPORT_HIT,
                    Some(attribute_type),
                    |lift| {
                        let f = lift.b.ty_of(Kind::F, 1);
                        let u = lift.b.ty_of(Kind::U, 1);
                        let pointer = lift.pointer_type(attribute_type);
                        vec![("t", f), ("hit_kind", u), ("attributes", pointer)]
                    },
                    true,
                );
                let accepted = self.b.result_expr(E::CallResult(f));
                self.call_operation(f, vec![t.expr, kind.expr, attributes], Some(accepted));
                let accepted = Val {
                    expr: accepted,
                    kind: Kind::B,
                    width: 1,
                };
                self.define_result(result, Lv::Scalar(accepted))
            }
            159 => {
                // CallShader(index, parameter)
                let (parameter, parameter_type) = self.memory_pointer(args[2])?;
                let index = self.uint(args[1])?;
                let f = self.operation(
                    primase_shader::rt::CALL_SHADER,
                    Some(parameter_type),
                    |lift| {
                        let u = lift.b.ty_of(Kind::U, 1);
                        let pointer = lift.pointer_type(parameter_type);
                        vec![("shader_index", u), ("parameter", pointer)]
                    },
                    false,
                );
                self.call_operation(f, vec![index.expr, parameter], None);
                Ok(())
            }
            _ => {
                let Some(system) = ray_system_value(code) else {
                    return self.refuse(format!("ray tracing operation {code}"));
                };
                let value = self.system_input(system)?;
                // a vector value is read a component at a time
                let value = if value.width > 1 {
                    let Some(component) = const_int(self.m, self.f, args[1]).filter(|c| (0..3).contains(c))
                    else {
                        return self.refuse("a ray value read at a computed component");
                    };
                    self.b.lane(value, component as u8)
                } else {
                    value
                };
                let ty = result.map(|(_, t)| t).expect("a call has a result");
                let kind = self.kind_of_type(ty)?;
                let r = self.b.bitcast_to(value, kind);
                self.define_result(result, Lv::Scalar(r))
            }
        }
    }
}
