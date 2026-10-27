//! The stages that are functions, not entry points: a hull program's phases, a domain program, a vertex program that
//! writes no position (see [`primase_shader::stage_functions`]). What builds their arguments into registers, reads
//! the registers only they have, and returns their results.

use naga::{Expression as E, Handle, LocalVariable, Statement};
use primase_shader::build::{Kind, Val};
use primase_shader::{
    Conventions, D3dModes, FunctionSig, IoType, Program, Provenance, ShaderFormat, StageFacts, Varying,
};

use super::Lift;
use super::resources::lanes_of;
use super::setup::PhaseKind;
use crate::Result;
use crate::decode::{Operand, Register};

/// How many fork or join instances a phase may be unrolled into.
const MAX_INSTANCES: u32 = 64;

/// One output varying of a geometry program: its array local, the register local it is emitted from, the register's
/// lanes it takes and the type it is carried as.
type GsOutputLocal = (
    Handle<LocalVariable>,
    Handle<LocalVariable>,
    Vec<u8>,
    primase_shader::Value,
);

/// A geometry program's output so far: how many vertices it has emitted, whether the next begins a strip, and for each
/// output varying and each vertex the value emitted.
#[derive(Clone)]
pub(super) struct GsState {
    count: Handle<LocalVariable>,
    pending: Handle<LocalVariable>,
    strips: Handle<LocalVariable>,
    /// Per output varying: the array local, the register it is emitted from, its lanes and carried type.
    outputs: Vec<GsOutputLocal>,
    max: u32,
}

impl Lift<'_> {
    /// The varyings the current function's result is made of: the outputs, or a hull program's patch constants.
    pub(super) fn result_varyings(&self) -> Vec<Varying> {
        match (&self.sig, &self.setup.interface.facts) {
            (Some(sig), StageFacts::Hull(h)) if sig.name == "patch_constant" => h.patch_constants.clone(),
            _ => self.setup.interface.outputs.clone(),
        }
    }

    /// An array of `count` registers of four raw lanes, as a local.
    fn register_array(&mut self, name: &str, count: u32) -> Option<Handle<LocalVariable>> {
        let len = std::num::NonZeroU32::new(count)?;
        let u4 = self.u4();
        let ty = self.b.ty(naga::TypeInner::Array {
            base: u4,
            size: naga::ArraySize::Constant(len),
            stride: 16,
        });
        Some(self.b.local(name, ty))
    }

    /// Store `value` in lane `lane` of register `index` of the register array `array` (a constant position).
    fn store_register_lane(&mut self, array: Handle<LocalVariable>, index: u32, lane: u8, value: Val) {
        let pointer = self.b.local_ptr(array);
        let register = self.b.app(E::AccessIndex { base: pointer, index });
        let target = self.b.app(E::AccessIndex {
            base: register,
            index: u32::from(lane),
        });
        self.b.store(target, value);
    }

    /// The registers per control point of a list of varyings: one past the highest register.
    fn registers_of(list: &[Varying]) -> u32 {
        list.iter()
            .filter_map(|v| v.provenance.d3d.map(|d| d.register))
            .filter(|r| *r != u32::MAX)
            .map(|r| r + 1)
            .max()
            .unwrap_or(0)
    }

    /// A local of four raw lanes whose first lane is `value`.
    fn id_local(&mut self, name: &str, value: Val) -> Handle<LocalVariable> {
        let u4 = self.u4();
        let local = self.b.local(name, u4);
        let bits = self.b.bitcast_to(value, Kind::U);
        let pointer = self.b.local_ptr(local);
        let target = self.b.app(E::AccessIndex {
            base: pointer,
            index: 0,
        });
        self.b.store(target, bits);
        local
    }

    /// The function's arguments become the registers the instructions read.
    fn prologue_function(&mut self, sig: &FunctionSig) -> Result<()> {
        let decl = match self.b.declare_function(sig) {
            Ok(d) => d,
            Err(detail) => return self.refuse(detail),
        };
        self.result_type = decl.result;
        self.pending_decl = Some(decl);
        let facts = self.setup.interface.facts.clone();
        let inputs = self.setup.interface.inputs.clone();
        let outputs = self.setup.interface.outputs.clone();
        let (points, out_points) = match &facts {
            StageFacts::Hull(h) => (h.input_control_points, h.output_control_points),
            StageFacts::Domain(d) => (d.input_control_points, 0),
            StageFacts::Geometry(g) => (g.input.vertices(), 0),
            _ => (0, 0),
        };
        // the input control points as one array of registers, `registers` to a point
        if points > 0 {
            let registers = Self::registers_of(&inputs);
            if let Some(array) = self.register_array("vicp", points * registers) {
                self.control_points = Some((array, registers));
            }
        }
        if out_points > 0 && sig.name == "patch_constant" {
            let registers = Self::registers_of(&outputs);
            if let Some(array) = self.register_array("vocp", out_points * registers) {
                self.output_points = Some((array, registers));
            }
        }
        if let StageFacts::Domain(d) = &facts {
            let registers = Self::registers_of(&d.patch_constants);
            self.patch_in = self.register_array("vpc", registers);
        }
        if let StageFacts::Geometry(g) = &facts {
            self.start_geometry(g.max_vertices)?;
        }
        for (index, item) in sig.arguments.iter().enumerate() {
            let expr = self.b.app(E::FunctionArgument(index as u32));
            match (item.name.as_str(), item.ty) {
                ("SV_GSInstanceID", IoType::Value(v)) => {
                    let value = Val {
                        expr,
                        kind: Kind::of(v.scalar),
                        width: 1,
                    };
                    self.gs_instance = Some(self.id_local("vGSInstanceID", value));
                }
                ("SV_OutputControlPointID", IoType::Value(v)) => {
                    let value = Val {
                        expr,
                        kind: Kind::of(v.scalar),
                        width: 1,
                    };
                    self.point_id = Some(self.id_local("vOutputControlPointID", value));
                }
                ("SV_PrimitiveID", IoType::Value(v)) => {
                    let value = Val {
                        expr,
                        kind: Kind::of(v.scalar),
                        width: 1,
                    };
                    self.primitive_id = Some(self.id_local("vPrim", value));
                }
                ("SV_DomainLocation", IoType::Value(v)) => {
                    let u4 = self.u4();
                    let local = self.b.local("vDomain", u4);
                    let value = Val {
                        expr,
                        kind: Kind::of(v.scalar),
                        width: v.width,
                    };
                    let pointer = self.b.local_ptr(local);
                    for lane in 0..v.width {
                        let component = self.b.lane(value, lane);
                        let bits = self.b.bitcast_to(component, Kind::U);
                        let target = self.b.app(E::AccessIndex {
                            base: pointer,
                            index: u32::from(lane),
                        });
                        self.b.store(target, bits);
                    }
                    self.domain_point = Some(local);
                }
                _ => {
                    if let Some(varying) = item.varying {
                        self.unpack_argument(item, expr, varying)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// One argument that carries a varying: its lanes go to the register the varying places them in.
    fn unpack_argument(
        &mut self,
        item: &primase_shader::FnItem,
        expr: Handle<E>,
        varying: usize,
    ) -> Result<()> {
        let facts = self.setup.interface.facts.clone();
        let is_patch_constant =
            matches!(&facts, StageFacts::Domain(_)) && !matches!(item.ty, IoType::Array(..));
        let (v, array) = if is_patch_constant {
            let StageFacts::Domain(d) = &facts else {
                unreachable!("checked")
            };
            (d.patch_constants[varying].clone(), self.patch_in.map(|a| (a, 0)))
        } else if item.name.starts_with("out_") {
            (self.setup.interface.outputs[varying].clone(), self.output_points)
        } else {
            (self.setup.interface.inputs[varying].clone(), self.control_points)
        };
        // a system value the signature does not place in a register has none
        let Some(d3d) = v.provenance.d3d.filter(|d| d.register != u32::MAX) else {
            return Ok(());
        };
        let bits_of = |lift: &mut Self, base: Handle<E>, carried: primase_shader::Value| -> Val {
            let value = Val {
                expr: base,
                kind: Kind::of(carried.scalar),
                width: carried.width,
            };
            if value.kind == Kind::B {
                let ones = lift.b.lit_u(u32::MAX);
                let zero = lift.b.lit_u(0);
                lift.b.select(value, ones, zero)
            } else {
                lift.b.bitcast(value, Kind::U)
            }
        };
        match item.ty {
            IoType::Array(carried, n) => {
                let Some((array, registers)) = array else {
                    return Ok(());
                };
                for point in 0..n {
                    let element = self.b.app(E::AccessIndex {
                        base: expr,
                        index: point,
                    });
                    let bits = bits_of(self, element, carried);
                    for (j, lane) in lanes_of(d3d.mask).into_iter().enumerate() {
                        let component = self.b.lane(bits, j as u8);
                        self.store_register_lane(array, point * registers + d3d.register, lane, component);
                    }
                }
            }
            IoType::Value(carried) => {
                let bits = bits_of(self, expr, carried);
                if is_patch_constant {
                    if let Some((array, _)) = array {
                        for (j, lane) in lanes_of(d3d.mask).into_iter().enumerate() {
                            let component = self.b.lane(bits, j as u8);
                            self.store_register_lane(array, d3d.register, lane, component);
                        }
                    }
                } else if let Some(&local) = self.inputs.get(&d3d.register) {
                    // a vertex program's input: its register `v<n>`
                    let pointer = self.b.local_ptr(local);
                    for (j, lane) in lanes_of(d3d.mask).into_iter().enumerate() {
                        let component = self.b.lane(bits, j as u8);
                        let target = self.b.app(E::AccessIndex {
                            base: pointer,
                            index: u32::from(lane),
                        });
                        self.b.store(target, component);
                    }
                }
            }
            IoType::ClipDistances(_) => return self.refuse("clip distances as an argument"),
            IoType::Memory => return self.refuse("a pointer as an argument"),
        }
        Ok(())
    }

    /// A geometry program's output locals: an array per output varying, the count, the strip starts.
    fn start_geometry(&mut self, max: u32) -> Result<()> {
        let one = std::num::NonZeroU32::new(max).expect("verified with the facts");
        let u1 = self.b.ty_of(Kind::U, 1);
        let strips_ty = self.b.ty(naga::TypeInner::Array {
            base: u1,
            size: naga::ArraySize::Constant(one),
            stride: 4,
        });
        let count = self.b.local("emitted", u1);
        let pending = self.b.local("strip_pending", u1);
        let strips = self.b.local("strip_starts", strips_ty);
        let (zero, first) = (self.b.lit_u(0), self.b.lit_u(1));
        let pointer = self.b.local_ptr(count);
        self.b.store(pointer, zero);
        let pointer = self.b.local_ptr(pending);
        self.b.store(pointer, first);
        let mut outputs = Vec::new();
        let sig = self.sig.clone().expect("a stage function");
        for (i, v) in self.setup.interface.outputs.clone().into_iter().enumerate() {
            let Some(d3d) = v.provenance.d3d else {
                return self.refuse("an output without a register");
            };
            let Some(&register) = self.outputs.get(&d3d.register) else {
                return self.refuse("an output register that is not declared");
            };
            let Some(IoType::Array(carried, _)) =
                sig.result.iter().find(|r| r.varying == Some(i)).map(|r| r.ty)
            else {
                return self.refuse("an output without its result array");
            };
            let ty = self
                .b
                .io_array(carried, max)
                .map_err(|d| crate::error(self.word, d))?;
            let array = self
                .b
                .local(&format!("emit_{}", Program::varying_name(&v.label)), ty);
            let lanes = match v.system {
                Some(primase_shader::SystemValue::ClipPosition) => vec![0, 1, 2, 3],
                _ => lanes_of(d3d.mask),
            };
            outputs.push((array, register, lanes, carried));
        }
        self.gs = Some(GsState {
            count,
            pending,
            strips,
            outputs,
            max,
        });
        Ok(())
    }

    /// `emit`: the output registers become the next vertex (past the most vertices, nothing).
    pub(super) fn emit_vertex(&mut self) -> Result<()> {
        let Some(gs) = self.gs.clone() else {
            return self.refuse("a vertex emitted outside a geometry program");
        };
        let count = {
            let p = self.b.local_ptr(gs.count);
            self.b.load(p, Kind::U, 1)
        };
        let max = self.b.lit_u(gs.max);
        let inside = self.b.binary(naga::BinaryOperator::Less, count, max);
        self.b.when(inside, |b| {
            let index = {
                let p = b.local_ptr(gs.count);
                b.load(p, Kind::U, 1)
            };
            for (array, register, lanes, carried) in &gs.outputs {
                let p = b.local_ptr(*register);
                let row = b.load(p, Kind::U, 4);
                let picked = b.swizzle(row, lanes);
                let value = b.bitcast(picked, Kind::of(carried.scalar));
                let a = b.local_ptr(*array);
                let target = b.app(E::Access {
                    base: a,
                    index: index.expr,
                });
                b.store(target, value);
            }
            let pending = {
                let p = b.local_ptr(gs.pending);
                b.load(p, Kind::U, 1)
            };
            let s = b.local_ptr(gs.strips);
            let target = b.app(E::Access {
                base: s,
                index: index.expr,
            });
            b.store(target, pending);
            let zero = b.lit_u(0);
            let p = b.local_ptr(gs.pending);
            b.store(p, zero);
            let one = b.lit_u(1);
            let next = b.binary(naga::BinaryOperator::Add, index, one);
            let p = b.local_ptr(gs.count);
            b.store(p, next);
        });
        Ok(())
    }

    /// `cut`: the next vertex begins a strip.
    pub(super) fn cut_strip(&mut self) -> Result<()> {
        let Some(gs) = self.gs.clone() else {
            return self.refuse("a strip cut outside a geometry program");
        };
        let one = self.b.lit_u(1);
        let p = self.b.local_ptr(gs.pending);
        self.b.store(p, one);
        Ok(())
    }

    /// The geometry program's result: the emitted vertices, how many, and where strips start.
    fn geometry_epilogue(&mut self) -> Result<()> {
        let result = self.result_type.expect("a geometry program returns its vertices");
        let gs = self.gs.clone().expect("a geometry program");
        let mut parts = Vec::new();
        for (array, ..) in &gs.outputs {
            let p = self.b.local_ptr(*array);
            parts.push(self.b.load(p, Kind::U, 1).expr);
        }
        let p = self.b.local_ptr(gs.count);
        parts.push(self.b.load(p, Kind::U, 1).expr);
        let p = self.b.local_ptr(gs.strips);
        parts.push(self.b.load(p, Kind::U, 1).expr);
        let composed = self.b.app(E::Compose {
            ty: result,
            components: parts,
        });
        self.b.push(Statement::Return {
            value: Some(composed),
        });
        Ok(())
    }

    /// Pack the result registers into the function's result structure and return it.
    pub(super) fn function_epilogue(&mut self) -> Result<()> {
        if self.gs.is_some() {
            return self.geometry_epilogue();
        }
        let Some(result) = self.result_type else {
            self.b.push(Statement::Return { value: None });
            return Ok(());
        };
        let sig = self.sig.clone().expect("a stage function");
        let varyings = self.result_varyings();
        let mut parts = Vec::new();
        for item in &sig.result {
            let (IoType::Value(carried), Some(index)) = (item.ty, item.varying) else {
                return self.refuse("a result that is not a varying's value");
            };
            let v = &varyings[index];
            let Some(d3d) = v.provenance.d3d else {
                return self.refuse(format!("{} has no register", item.name));
            };
            let Some(&local) = self.outputs.get(&d3d.register) else {
                return self.refuse(format!("{} has no register {}", item.name, d3d.register));
            };
            let lanes: Vec<u8> = match v.system {
                Some(primase_shader::SystemValue::ClipPosition) => vec![0, 1, 2, 3],
                _ => lanes_of(d3d.mask),
            };
            let p = self.b.local_ptr(local);
            let row = self.b.load(p, Kind::U, 4);
            let picked = self.b.swizzle(row, &lanes);
            parts.push(self.b.bitcast(picked, Kind::of(carried.scalar)).expr);
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

    /// The local of a fork or join phase's instance number, made the first time it is needed.
    fn instance_local(&mut self, kind: PhaseKind) -> Handle<LocalVariable> {
        let existing = if kind == PhaseKind::Fork {
            self.fork_id
        } else {
            self.join_id
        };
        if let Some(local) = existing {
            return local;
        }
        let u4 = self.u4();
        let name = if kind == PhaseKind::Fork {
            "vForkInstanceID"
        } else {
            "vJoinInstanceID"
        };
        let local = self.b.local(name, u4);
        if kind == PhaseKind::Fork {
            self.fork_id = Some(local);
        } else {
            self.join_id = Some(local);
        }
        local
    }

    /// Lift one function of the program as the module's function `sig`.
    pub(super) fn run_function(&mut self, sig: &FunctionSig) -> Result<()> {
        self.sig = Some(sig.clone());
        self.frames.clear();
        self.returned = false;
        self.declare_locals();
        self.prologue_function(sig)?;
        if matches!(self.setup.interface.facts, StageFacts::Hull(_)) {
            // the control-point phase is `main`; the fork and join phases are the patch constants, each instance of a
            // phase unrolled with its number
            self.in_phase = true;
            for phase in self.setup.phases.clone() {
                if (sig.name == "main") != (phase.kind == PhaseKind::ControlPoint) {
                    continue;
                }
                let instances = if phase.kind == PhaseKind::ControlPoint {
                    1
                } else {
                    phase.instances
                };
                if instances > MAX_INSTANCES {
                    return self.refuse(format!("a hull phase of {instances} instances"));
                }
                for instance in 0..instances {
                    if phase.kind != PhaseKind::ControlPoint {
                        let local = self.instance_local(phase.kind);
                        let n = self.b.lit_u(instance);
                        let pointer = self.b.local_ptr(local);
                        let target = self.b.app(E::AccessIndex {
                            base: pointer,
                            index: 0,
                        });
                        self.b.store(target, n);
                    }
                    self.returned = false;
                    self.body_range(phase.words.clone())?;
                    if !self.frames.is_empty() {
                        return self.refuse("a hull phase that ends inside control flow");
                    }
                }
            }
            self.in_phase = false;
            self.function_epilogue()?;
        } else {
            self.body_range(0..usize::MAX)?;
            if !self.returned {
                self.function_epilogue()?;
            }
        }
        let decl = self.pending_decl.take().expect("declared in the prologue");
        self.b.end_function(sig.name, decl);
        Ok(())
    }

    pub(super) fn finish_functions(mut self, target: String) -> Result<Program> {
        self.b.check().map_err(|detail| crate::error(self.word, detail))?;
        let stage = self.setup.interface.stage;
        let module = self.b.finish_functions();
        let mut limit_notes = primase_shader::compute_limit_notes(&module);
        let interface = self.setup.interface.clone();
        limit_notes.extend(primase_shader::interface_notes(&interface));
        Ok(Program {
            module,
            stage,
            entry: "main".into(),
            interface: self.setup.interface,
            bindings: self.resources.bound,
            conventions: Conventions::D3D,
            modes: D3dModes::ALL,
            provenance: Provenance {
                format: ShaderFormat::Dxbc,
                target,
                lifter: "primase-dxbc".into(),
                lifter_version: super::LIFTER_VERSION,
                // the hash of the bytes, which `lift` sets
                source_hash: None,
                instruction_count: Some(primase_shader::InstructionCount {
                    count: self.decoded.instructions.len() as u64,
                    unit: primase_shader::InstructionUnit::DxbcInstructions,
                }),
            },
            notes: self.notes.into_iter().chain(limit_notes).collect(),
        })
    }

    /// The register a control-point operand names: `v[point][register]` or `vocp[point][register]`.
    pub(super) fn control_point_register(&mut self, op: &Operand, output: bool) -> Result<Val> {
        let array = if output {
            self.output_points
        } else {
            self.control_points
        };
        let Some((array, registers)) = array else {
            return self.refuse("a control point is read where the function has none");
        };
        if op.indices.len() != 2 || op.indices[1].relative.is_some() {
            return self.refuse("a control point register indexed by a register");
        }
        let point = self.index_expr(&op.indices[0].clone())?;
        let stride = self.b.lit_u(registers);
        let base = self.b.binary(naga::BinaryOperator::Multiply, point, stride);
        let register = self.b.lit_u(op.indices[1].base);
        let at = self.b.binary(naga::BinaryOperator::Add, base, register);
        let pointer = self.b.local_ptr(array);
        let element = self.b.app(E::Access {
            base: pointer,
            index: at.expr,
        });
        Ok(self.b.load(element, Kind::U, 4))
    }

    /// The register `vpc[register]` of a domain program's patch constants.
    pub(super) fn patch_constant_register(&mut self, op: &Operand) -> Result<Val> {
        let Some(array) = self.patch_in else {
            return self.refuse("a patch constant is read where the function has none");
        };
        let index = self.index_expr(&op.indices[0].clone())?;
        let pointer = self.b.local_ptr(array);
        let element = self.b.app(E::Access {
            base: pointer,
            index: index.expr,
        });
        Ok(self.b.load(element, Kind::U, 4))
    }

    /// A register that is one local of four lanes: `vDomain`, `vOutputControlPointID`, the phase numbers.
    pub(super) fn id_register(&mut self, register: Register) -> Result<Val> {
        let local = match register {
            Register::InputDomainPoint => self.domain_point,
            Register::OutputControlPointId => self.point_id,
            Register::GsInstanceId => self.gs_instance,
            Register::ForkInstanceId => self.fork_id,
            _ => self.join_id,
        };
        let Some(local) = local else {
            return self.refuse(format!("{register:?} is used where the function has none"));
        };
        let p = self.b.local_ptr(local);
        Ok(self.b.load(p, Kind::U, 4))
    }
}
