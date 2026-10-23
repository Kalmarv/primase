//! The stages that are functions, not entry points: a hull program's two phases, a domain program, a vertex program
//! that writes no position. What reads their arguments and fills their results (see
//! [`primase_shader::stage_functions`]).

use naga::{Expression as E, Handle};
use primase_shader::build::{Kind, Val};
use primase_shader::{IoType, Program, StageFacts, Varying};

use super::setup::const_int;
use super::{Lift, Lv};
use crate::Result;
use crate::module::{Instruction, ValueId};

impl Lift<'_> {
    /// The expression of the current function's argument `index`, made once: a pre-emitted one, readable from any
    /// block.
    pub(super) fn argument_expr(&mut self, index: usize) -> Handle<E> {
        match self.argument_exprs.get(&index) {
            Some(&e) => e,
            None => {
                let e = self.b.app(E::FunctionArgument(index as u32));
                self.argument_exprs.insert(index, e);
                e
            }
        }
    }

    /// The argument that carries input varying `varying`, and its type.
    pub(super) fn input_argument(&self, varying: usize) -> Result<(usize, IoType)> {
        match &self.sig {
            None => {
                let io = &self.setup.io;
                match io.arguments.iter().position(|item| item.varyings[0] == varying) {
                    Some(index) => Ok((index, io.arguments[index].ty)),
                    None => self.refuse("a kept input varying without an argument"),
                }
            }
            Some(sig) => {
                let name = Program::varying_name(&self.setup.interface.inputs[varying].label);
                self.argument_named(sig, &name)
            }
        }
    }

    fn argument_named(&self, sig: &primase_shader::FunctionSig, name: &str) -> Result<(usize, IoType)> {
        match sig.arguments.iter().position(|a| a.name == name) {
            Some(index) => Ok((index, sig.arguments[index].ty)),
            None => self.refuse(format!("{}: no argument {name}", sig.name)),
        }
    }

    /// The value of the argument the pipeline supplies under this name.
    fn special_argument(&mut self, name: &str) -> Result<Val> {
        let Some(sig) = self.sig.clone() else {
            return self.refuse(format!("{name} outside a stage function"));
        };
        let (index, ty) = self.argument_named(&sig, name)?;
        let IoType::Value(v) = ty else {
            return self.refuse(format!("{name} is not a value"));
        };
        let expr = self.argument_expr(index);
        Ok(Val {
            expr,
            kind: Kind::of(v.scalar),
            width: v.width,
        })
    }

    /// The patch constants of the current program: a hull program's outputs, a domain program's inputs.
    fn patch_constants(&self) -> &[Varying] {
        match &self.setup.interface.facts {
            StageFacts::Hull(h) => &h.patch_constants,
            StageFacts::Domain(d) => &d.patch_constants,
            _ => &[],
        }
    }

    /// The result of a call as the type it has, from a value carried as `kind`.
    fn typed_result(&mut self, inst: &Instruction, value: Val) -> Result<()> {
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let kind = self.kind_of_type(ty)?;
        let r = self.b.bitcast_to(value, kind);
        self.define_result(inst.result, Lv::Scalar(r))
    }

    /// `LoadPatchConstant(element, row, column)` in a domain program: one lane of a patch constant.
    pub(super) fn load_patch_constant(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let (Some(id), Some(row), Some(col)) = (
            const_int(self.m, self.f, args[1]),
            const_int(self.m, self.f, args[2]),
            const_int(self.m, self.f, args[3]),
        ) else {
            return self.refuse("a patch constant read at a computed element, row or column");
        };
        let Some(element) = self
            .setup
            .patch_elements
            .iter()
            .find(|e| i64::from(e.id) == id)
            .cloned()
        else {
            return self.refuse(format!("patch constant element {id}"));
        };
        let (register, lane) = (element.row + row as u32, element.column + col as u8);
        let Some((varying, position)) = patch_cell(self.patch_constants(), register, lane) else {
            return self.refuse(format!(
                "patch constant {} has no lane at register {register}",
                element.name
            ));
        };
        let name = Program::varying_name(&self.patch_constants()[varying].label);
        let value = self.special_argument(&name)?;
        let lane_value = if value.width > 1 {
            self.b.lane(value, position)
        } else {
            value
        };
        self.typed_result(inst, lane_value)
    }

    /// `StorePatchConstant(element, row, column, value)` in a hull program: one lane of a patch constant, kept as raw
    /// bits in its register's local until the function returns.
    pub(super) fn store_patch_constant(&mut self, args: &[ValueId]) -> Result<()> {
        let (Some(id), Some(row), Some(col)) = (
            const_int(self.m, self.f, args[1]),
            const_int(self.m, self.f, args[2]),
            const_int(self.m, self.f, args[3]),
        ) else {
            return self.refuse("a patch constant written at a computed element, row or column");
        };
        let Some(element) = self
            .setup
            .patch_elements
            .iter()
            .find(|e| i64::from(e.id) == id)
            .cloned()
        else {
            return self.refuse(format!("patch constant element {id}"));
        };
        let (register, lane) = (element.row + row as u32, element.column + col as u8);
        if patch_cell(self.patch_constants(), register, lane).is_none() {
            return self.refuse(format!(
                "patch constant {} has no lane at register {register}",
                element.name
            ));
        }
        let value = self.scalar(args[4])?;
        let bits = self.b.bitcast_to(value, Kind::U);
        let local = self.outputs[&register];
        let pointer = self.b.local_ptr(local);
        let target = self.b.app(E::AccessIndex {
            base: pointer,
            index: u32::from(lane),
        });
        self.b.store(target, bits);
        Ok(())
    }

    /// `LoadOutputControlPoint(element, row, column, vertex)` in a hull program's patch-constant function: one lane of
    /// an output control point.
    pub(super) fn load_output_control_point(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let (Some(id), Some(row), Some(col)) = (
            const_int(self.m, self.f, args[1]),
            const_int(self.m, self.f, args[2]),
            const_int(self.m, self.f, args[3]),
        ) else {
            return self.refuse("an output control point read at a computed element, row or column");
        };
        let Some(element) = self
            .setup
            .output_elements
            .iter()
            .find(|e| i64::from(e.id) == id)
            .cloned()
        else {
            return self.refuse(format!("output element {id}"));
        };
        let (register, lane) = (element.row + row as u32, element.column + col as u8);
        let Some((varying, position)) = self.setup.output_cell(register, lane) else {
            return self.refuse(format!(
                "output {} has no lane at register {register}",
                element.name
            ));
        };
        let name = format!(
            "out_{}",
            Program::varying_name(&self.setup.interface.outputs[varying].label)
        );
        let Some(sig) = self.sig.clone() else {
            return self.refuse("an output control point read outside a stage function");
        };
        let (index, ty) = self.argument_named(&sig, &name)?;
        let IoType::Array(v, _) = ty else {
            return self.refuse("output control points are not an array");
        };
        let points = self.argument_expr(index);
        let point = self.uint(args[4])?;
        let element = Val {
            expr: self.b.app(E::Access {
                base: points,
                index: point.expr,
            }),
            kind: Kind::of(v.scalar),
            width: v.width,
        };
        let lane_value = if v.width > 1 {
            self.b.lane(element, position)
        } else {
            element
        };
        self.typed_result(inst, lane_value)
    }

    /// `DomainLocation(component)`: a component of the vertex's place in its patch.
    pub(super) fn domain_location(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let Some(component) = const_int(self.m, self.f, args[1]).filter(|c| (0..3).contains(c)) else {
            return self.refuse("a domain location read at a computed component");
        };
        let location = self.special_argument("SV_DomainLocation")?;
        let lane = self.b.lane(location, component as u8);
        self.typed_result(inst, lane)
    }

    /// `PrimitiveID` and `OutputControlPointID`: the patch's number, and the control point a hull program's
    /// control-point phase is for.
    pub(super) fn patch_id(&mut self, inst: &Instruction, name: &str) -> Result<()> {
        let id = self.special_argument(name)?;
        self.typed_result(inst, id)
    }
}

impl Lift<'_> {
    /// Pack the result registers into the function's result structure and return it.
    pub(super) fn function_epilogue(&mut self) -> Result<()> {
        let Some(result) = self.result_type else {
            self.b.push(naga::Statement::Return { value: None });
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
                _ => (0..4u8).filter(|l| d3d.mask >> l & 1 != 0).collect(),
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
        self.b.push(naga::Statement::Return {
            value: Some(composed),
        });
        Ok(())
    }
}

/// The patch-constant varying whose register and lanes hold this cell, and the lane's position in its value.
fn patch_cell(list: &[Varying], register: u32, lane: u8) -> Option<(usize, u8)> {
    list.iter().enumerate().find_map(|(i, v)| {
        let d3d = v.provenance.d3d?;
        (d3d.register == register && d3d.mask >> lane & 1 != 0)
            .then(|| (i, (d3d.mask & ((1u8 << lane) - 1)).count_ones() as u8))
    })
}
