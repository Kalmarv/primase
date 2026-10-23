//! The program's edges: locals for the registers, the prologue that unpacks the entry arguments into input registers,
//! and the epilogue that packs output registers into the result.

use naga::{Expression as E, Handle, Statement, TypeInner};
use primase_shader::build::{Builder, Kind, Val};
use primase_shader::{IoType, ScalarType, Stage, SystemValue};

use super::resources::{Resources, lanes_of};
use super::setup::Setup;
use super::{Frame, Lift};
use crate::Result;
use crate::decode::{Declared, Decoded, Register};

fn kind_of(s: ScalarType) -> Kind {
    Kind::of(s)
}

impl<'a> Lift<'a> {
    pub(super) fn new(decoded: &'a Decoded, stage: Stage, setup: Setup) -> Result<Self> {
        let mut lift = Lift {
            b: Builder::new(),
            decoded,
            stage,
            temps: Vec::new(),
            inputs: Default::default(),
            outputs: Default::default(),
            indexable: Default::default(),
            ids: Default::default(),
            primitive_id: None,
            input_coverage: None,
            depth: None,
            coverage: None,
            resources: Resources::default(),
            frames: Vec::<Frame>::new(),
            notes: setup.notes.clone(),
            word: 0,
            returned: false,
            arguments: Vec::new(),
            result_type: None,
            setup,
        };
        let u4 = lift.u4();
        for n in 0..lift.setup.temps {
            let local = lift.b.local(&format!("r{n}"), u4);
            lift.temps.push(local);
        }
        for (register, size) in lift.setup.indexable.clone() {
            let Some(len) = std::num::NonZeroU32::new(size) else {
                continue;
            };
            let ty = lift.b.ty(TypeInner::Array {
                base: u4,
                size: naga::ArraySize::Constant(len),
                stride: 16,
            });
            let local = lift.b.local(&format!("x{register}"), ty);
            lift.indexable.insert(register, (local, size));
        }
        for register in lift.setup.declared_inputs.keys().copied().collect::<Vec<_>>() {
            let local = lift.b.local(&format!("v{register}"), u4);
            lift.inputs.insert(register, local);
        }
        // declared special inputs
        for d in &decoded.declarations {
            if let Declared::Input { operand, .. } = &d.kind {
                match operand.register {
                    Register::PrimitiveId if lift.primitive_id.is_none() => {
                        lift.primitive_id = Some(lift.b.local("vPrim", u4));
                    }
                    Register::InputCoverage if lift.input_coverage.is_none() => {
                        lift.input_coverage = Some(lift.b.local("vCoverage", u4));
                    }
                    Register::ThreadId
                    | Register::ThreadGroupId
                    | Register::ThreadIdInGroup
                    | Register::ThreadIdInGroupFlattened => {
                        let (system, name) = match operand.register {
                            Register::ThreadId => (SystemValue::DispatchThreadId, "vThreadID"),
                            Register::ThreadGroupId => (SystemValue::GroupId, "vThreadGroupID"),
                            Register::ThreadIdInGroup => (SystemValue::GroupThreadId, "vThreadIDInGroup"),
                            _ => (SystemValue::GroupIndex, "vThreadIDInGroupFlattened"),
                        };
                        lift.ids.entry(system).or_insert_with(|| lift.b.local(name, u4));
                    }
                    _ => {}
                }
            }
            if let Declared::Output { operand, .. } = &d.kind {
                match operand.register {
                    Register::Output => {
                        let register = operand.indices.first().map_or(0, |i| i.base);
                        if !lift.outputs.contains_key(&register) {
                            let local = lift.b.local(&format!("o{register}"), u4);
                            lift.outputs.insert(register, local);
                        }
                    }
                    Register::OutputDepth
                    | Register::OutputDepthGreaterEqual
                    | Register::OutputDepthLessEqual
                        if lift.depth.is_none() =>
                    {
                        lift.depth = Some(lift.b.local("oDepth", u4));
                    }
                    Register::OutputCoverage if lift.coverage.is_none() => {
                        lift.coverage = Some(lift.b.local("oMask", u4));
                    }
                    _ => {}
                }
            }
        }
        // registers the output signature places elements in
        for v in &lift.setup.interface.outputs {
            if let Some(d3d) = v.provenance.d3d
                && d3d.register != u32::MAX
                && !lift.outputs.contains_key(&d3d.register)
            {
                let local = lift.b.local(&format!("o{}", d3d.register), u4);
                lift.outputs.insert(d3d.register, local);
            }
        }
        lift.declare_resources()?;
        Ok(lift)
    }

    /// The entry arguments and result type, and the stores of each argument into its input register lanes.
    pub(super) fn prologue(&mut self) -> Result<()> {
        let io = self.setup.io.clone();
        let declared = match self.b.declare_entry(&io) {
            Ok(d) => d,
            Err(detail) => return self.refuse(detail),
        };
        self.arguments = declared.arguments;
        self.result_type = declared.result;
        for (i, item) in io.arguments.iter().enumerate() {
            let IoType::Value(v) = item.ty else {
                return self.refuse("an array-typed input");
            };
            let expr = self.b.app(E::FunctionArgument(i as u32));
            let mut value = Val {
                expr,
                kind: kind_of(v.scalar),
                width: v.width,
            };
            let varying = self.setup.interface.inputs[item.varyings[0]].clone();
            // raw bits: a bool is D3D's all-ones or zero
            if value.kind == Kind::B {
                let ones = self.b.lit_u(u32::MAX);
                let zero = self.b.lit_u(0);
                value = self.b.select(value, ones, zero);
            } else {
                value = self.b.bitcast(value, Kind::U);
            }
            if let Some(d3d) = varying.provenance.d3d {
                let declared = self
                    .setup
                    .declared_inputs
                    .get(&d3d.register)
                    .copied()
                    .unwrap_or(0);
                if declared & d3d.mask != 0 {
                    let local = self.inputs[&d3d.register];
                    let pointer = self.b.local_ptr(local);
                    for (j, lane) in lanes_of(d3d.mask).into_iter().enumerate() {
                        let component = self.b.lane(value, j as u8);
                        let target = self.b.app(E::AccessIndex {
                            base: pointer,
                            index: u32::from(lane),
                        });
                        self.b.store(target, component);
                    }
                }
            }
            let special = match varying.system {
                Some(SystemValue::PrimitiveIndex) => self.primitive_id,
                Some(SystemValue::Coverage) => self.input_coverage,
                Some(
                    SystemValue::DispatchThreadId
                    | SystemValue::GroupId
                    | SystemValue::GroupThreadId
                    | SystemValue::GroupIndex,
                ) => {
                    // an id's lanes, then zero in the lanes it does not have
                    let system = varying.system.expect("matched");
                    let Some(&local) = self.ids.get(&system) else {
                        continue;
                    };
                    let pointer = self.b.local_ptr(local);
                    for lane in 0..value.width {
                        let component = self.b.lane(value, lane);
                        let target = self.b.app(E::AccessIndex {
                            base: pointer,
                            index: u32::from(lane),
                        });
                        self.b.store(target, component);
                    }
                    None
                }
                _ => None,
            };
            if let Some(local) = special {
                let pointer = self.b.local_ptr(local);
                let target = self.b.app(E::AccessIndex {
                    base: pointer,
                    index: 0,
                });
                self.b.store(target, value);
            }
        }
        Ok(())
    }

    /// Pack the output registers into the result and return it.
    pub(super) fn epilogue(&mut self) -> Result<()> {
        let Some(result) = self.result_type else {
            self.b.push(Statement::Return { value: None });
            return Ok(());
        };
        let io = self.setup.io.clone();
        let mut parts = Vec::new();
        for item in &io.results {
            let part = match item.ty {
                IoType::Array(..) => return self.refuse("an array-typed output"),
                IoType::ClipDistances(_) => {
                    let mut components = Vec::new();
                    for &i in &item.varyings {
                        let v = self.setup.interface.outputs[i].clone();
                        let d3d = v.provenance.d3d.expect("a clip distance has a register");
                        let local = self.outputs[&d3d.register];
                        let p = self.b.local_ptr(local);
                        let row = self.b.load(p, Kind::U, 4);
                        for lane in lanes_of(d3d.mask) {
                            let c = self.b.lane(row, lane);
                            components.push(self.b.bitcast(c, Kind::F).expr);
                        }
                    }
                    let ty = self.array_type(&item.ty);
                    self.b.app(E::Compose { ty, components })
                }
                IoType::Value(carried) => {
                    let v = self.setup.interface.outputs[item.varyings[0]].clone();
                    let (local, lanes): (Handle<naga::LocalVariable>, Vec<u8>) = match v.system {
                        Some(
                            SystemValue::Depth | SystemValue::DepthGreaterEqual | SystemValue::DepthLessEqual,
                        ) => (self.depth.expect("a depth output has its local"), vec![0]),
                        Some(SystemValue::Coverage) => {
                            (self.coverage.expect("a coverage output has its local"), vec![0])
                        }
                        Some(SystemValue::ClipPosition | SystemValue::Target(_)) => {
                            let d3d = v.provenance.d3d.expect("a signature element has a register");
                            match self.outputs.get(&d3d.register) {
                                Some(&l) => (l, vec![0, 1, 2, 3]),
                                None => {
                                    return self.refuse(format!(
                                        "output register o{} is not declared",
                                        d3d.register
                                    ));
                                }
                            }
                        }
                        _ => {
                            let d3d = v.provenance.d3d.expect("a signature element has a register");
                            match self.outputs.get(&d3d.register) {
                                Some(&l) => (l, lanes_of(d3d.mask)),
                                None => {
                                    return self.refuse(format!(
                                        "output register o{} is not declared",
                                        d3d.register
                                    ));
                                }
                            }
                        }
                    };
                    let p = self.b.local_ptr(local);
                    let row = self.b.load(p, Kind::U, 4);
                    let picked = self.b.swizzle(row, &lanes);
                    let value = self.b.bitcast(picked, kind_of(carried.scalar));
                    debug_assert_eq!(value.width, carried.width);
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

    fn array_type(&mut self, ty: &IoType) -> Handle<naga::Type> {
        let IoType::ClipDistances(n) = ty else {
            unreachable!("an array type")
        };
        let f = self.b.ty_of(Kind::F, 1);
        self.b.ty(TypeInner::Array {
            base: f,
            size: naga::ArraySize::Constant(std::num::NonZeroU32::new(*n).expect("clip distances")),
            stride: 4,
        })
    }
}
