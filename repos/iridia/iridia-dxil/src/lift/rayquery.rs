//! Inline ray tracing: the `rayQuery_*` operations as naga's ray query (a local `RayQuery` object, `Initialize`,
//! `Proceed`, `Terminate`, the confirmed hit and the intersection a query returns). Capability `RAY_QUERY`.

use iridia::build::{Kind, Val};
use naga::{Expression as E, Handle, LocalVariable, RayQueryFunction, Statement};

use super::setup::const_int;
use super::{Lift, Lv};
use crate::Result;
use dxil_reader::module::{Instruction, ValueId};

/// A field of naga's `RayDesc`, by member index.
const RAY_FLAGS: u32 = 0;
const RAY_T_MIN: u32 = 2;
const RAY_ORIGIN: u32 = 4;
const RAY_DIRECTION: u32 = 5;

/// A field of naga's `RayIntersection`, by member index.
const KIND: u32 = 0;
const T: u32 = 1;
const INSTANCE_CUSTOM_DATA: u32 = 2;
const INSTANCE_INDEX: u32 = 3;
const SBT_RECORD_OFFSET: u32 = 4;
const GEOMETRY_INDEX: u32 = 5;
const PRIMITIVE_INDEX: u32 = 6;
const BARYCENTRICS: u32 = 7;
const FRONT_FACE: u32 = 8;
const OBJECT_TO_WORLD: u32 = 9;
const WORLD_TO_OBJECT: u32 = 10;

impl Lift<'_> {
    /// The query a handle value names.
    fn ray_query(&mut self, v: ValueId) -> Result<(Handle<LocalVariable>, u32, Handle<LocalVariable>)> {
        match self.get(v)? {
            Lv::RayQuery(local, flags, ray) => Ok((local, flags, ray)),
            _ => self.refuse("a ray query operation on something that is not a ray query"),
        }
    }

    /// `AllocateRayQuery(flags)`: a ray query object; the flags it is declared with join the ones it is traced with.
    pub(super) fn allocate_ray_query(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let Some(flags) = const_int(self.m, self.f, args[1]).and_then(|f| u32::try_from(f).ok()) else {
            return self.refuse("a ray query declared with flags that are not constant");
        };
        self.notes
            .insert("ray queries need the RAY_QUERY capability of the device".into());
        let ty = self.b.ty(naga::TypeInner::RayQuery { vertex_return: false });
        let local = self.b.local("ray_query", ty);
        let ray_ty = self.b.module.generate_ray_desc_type();
        let ray = self.b.local("ray", ray_ty);
        self.define_result(inst.result, Lv::RayQuery(local, flags, ray))
    }

    /// The ray query operations: `TraceRayInline` (179), `Proceed` (180), `Abort` (181), the confirmed hits (182, 183)
    /// and the state of the candidate or committed intersection (184 to 215).
    pub(super) fn ray_query_op(&mut self, inst: &Instruction, code: u64, args: &[ValueId]) -> Result<()> {
        let (local, declared, ray) = self.ray_query(args[1])?;
        let query = self.b.local_ptr(local);
        match code {
            179 => {
                let (_, structure) = self.handle_expr(args[2], 0)?;
                let flags = self.scalar(args[3])?;
                let mut flags = self.b.bitcast_to(flags, Kind::U);
                // the flags the query is declared with join the ones it is traced with
                if declared != 0 {
                    let declared = self.b.lit_u(declared);
                    flags = self.b.binary(naga::BinaryOperator::InclusiveOr, flags, declared);
                }
                let mask = self.scalar(args[4])?;
                let mask = self.b.bitcast_to(mask, Kind::U);
                let vec3 = |lift: &mut Self, first: usize| -> Result<Val> {
                    let parts = [
                        lift.float(args[first])?,
                        lift.float(args[first + 1])?,
                        lift.float(args[first + 2])?,
                    ];
                    Ok(lift.b.compose(&parts))
                };
                let origin = vec3(self, 5)?;
                let tmin = self.float(args[8])?;
                let direction = vec3(self, 9)?;
                let tmax = self.float(args[12])?;
                let ty = self.b.module.generate_ray_desc_type();
                let descriptor = self.b.app(E::Compose {
                    ty,
                    components: vec![
                        flags.expr,
                        mask.expr,
                        tmin.expr,
                        tmax.expr,
                        origin.expr,
                        direction.expr,
                    ],
                });
                // the ray is kept for the queries about it
                let kept = self.b.local_ptr(ray);
                self.b.push(Statement::Store {
                    pointer: kept,
                    value: descriptor,
                });
                self.b.push(Statement::RayQuery {
                    query,
                    fun: RayQueryFunction::Initialize {
                        acceleration_structure: structure,
                        descriptor,
                    },
                });
                Ok(())
            }
            180 => {
                let result = self.b.result_expr(E::RayQueryProceedResult);
                self.b.push(Statement::RayQuery {
                    query,
                    fun: RayQueryFunction::Proceed { result },
                });
                let more = Val {
                    expr: result,
                    kind: Kind::B,
                    width: 1,
                };
                self.define_result(inst.result, Lv::Scalar(more))
            }
            181 => {
                self.b.push(Statement::RayQuery {
                    query,
                    fun: RayQueryFunction::Terminate,
                });
                Ok(())
            }
            182 => {
                self.b.push(Statement::RayQuery {
                    query,
                    fun: RayQueryFunction::ConfirmIntersection,
                });
                Ok(())
            }
            183 => {
                let hit = self.float(args[2])?;
                self.b.push(Statement::RayQuery {
                    query,
                    fun: RayQueryFunction::GenerateIntersection { hit_t: hit.expr },
                });
                Ok(())
            }
            _ => self.ray_query_state(inst, code, args, query, ray),
        }
    }

    /// One field of the candidate or committed intersection.
    fn ray_query_state(
        &mut self,
        inst: &Instruction,
        code: u64,
        args: &[ValueId],
        query: Handle<E>,
        ray: Handle<LocalVariable>,
    ) -> Result<()> {
        let committed = matches!(code, 184 | 188 | 189 | 192 | 194 | 200 | 207..=212 | 215);
        // the structure the intersection is
        self.b.module.generate_ray_intersection_type();
        let intersection = self.b.app(E::RayQueryGetIntersection { query, committed });
        let field = |lift: &mut Self, index: u32, kind: Kind, width: u8| Val {
            expr: lift.b.app(E::AccessIndex {
                base: intersection,
                index,
            }),
            kind,
            width,
        };
        let component = |lift: &Self, n: usize| -> Result<u32> {
            const_int(lift.m, lift.f, args[n])
                .and_then(|c| u32::try_from(c).ok())
                .map_or_else(
                    || lift.refuse("a ray query component read at a computed index"),
                    Ok,
                )
        };
        // one field of the ray the query was traced with: RayDesc's flags, t_min, origin or direction
        let traced = |lift: &mut Self, index: u32, kind: Kind, width: u8| {
            let ptr = lift.b.local_ptr(ray);
            let field = lift.b.app(E::AccessIndex { base: ptr, index });
            lift.b.load(field, kind, width)
        };
        let value = match code {
            195 => traced(self, RAY_FLAGS, Kind::U, 1),
            198 => traced(self, RAY_T_MIN, Kind::F, 1),
            196 | 197 => {
                let n = component(self, 2)?;
                let vector = traced(
                    self,
                    if code == 196 { RAY_ORIGIN } else { RAY_DIRECTION },
                    Kind::F,
                    3,
                );
                self.b.lane(vector, n as u8)
            }
            // the status of the committed hit: nothing (0), a triangle (1) or a procedural primitive (2), as naga's kind
            184 => field(self, KIND, Kind::U, 1),
            // the candidate: a non-opaque triangle (0) or a procedural primitive (1); naga's kind is 1 and 3
            185 => {
                let kind = field(self, KIND, Kind::U, 1);
                let triangle = self.b.lit_u(1);
                let is_triangle = self.b.binary(naga::BinaryOperator::Equal, kind, triangle);
                let (zero, one) = (self.b.lit_u(0), self.b.lit_u(1));
                self.b.select(is_triangle, zero, one)
            }
            191 | 192 => field(self, FRONT_FACE, Kind::B, 1),
            193 | 194 => {
                let n = component(self, 2)?;
                let pair = field(self, BARYCENTRICS, Kind::F, 2);
                self.b.lane(pair, n as u8)
            }
            199 | 200 => field(self, T, Kind::F, 1),
            201 | 207 => field(self, INSTANCE_INDEX, Kind::U, 1),
            202 | 208 => field(self, INSTANCE_CUSTOM_DATA, Kind::U, 1),
            203 | 209 => field(self, GEOMETRY_INDEX, Kind::U, 1),
            204 | 210 => field(self, PRIMITIVE_INDEX, Kind::U, 1),
            214 | 215 => field(self, SBT_RECORD_OFFSET, Kind::U, 1),
            // a 3 by 4 matrix, row `r` column `c`: naga's is four columns of three rows
            186..=189 => {
                let (row, column) = (component(self, 2)?, component(self, 3)?);
                let index = if matches!(code, 186 | 188) {
                    OBJECT_TO_WORLD
                } else {
                    WORLD_TO_OBJECT
                };
                let matrix = field(self, index, Kind::F, 1);
                let column = self.b.app(E::AccessIndex {
                    base: matrix.expr,
                    index: column,
                });
                Val {
                    expr: self.b.app(E::AccessIndex {
                        base: column,
                        index: row,
                    }),
                    kind: Kind::F,
                    width: 1,
                }
            }
            other => {
                return self.refuse(format!(
                    "ray query state {other} is not carried (naga's query keeps the ray's own values to itself)"
                ));
            }
        };
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let kind = self.kind_of_type(ty)?;
        let r = self.b.bitcast_to(value, kind);
        self.define_result(inst.result, Lv::Scalar(r))
    }
}
