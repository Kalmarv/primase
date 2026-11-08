//! The one way primase lifts a DXBC or DXIL container onto the IR: the lift, then [`iridia_passes::prepare`] (a control flow graph made
//! regions, values promoted, simplified, the exits of loops and switches made explicit), which is the form every analysis
//! (`iridia_analysis`) and executor (`ProgramShader::from_ir`, `Executable::from_ir`) of the IR expects. A raw lift can hold a
//! control flow graph, which they refuse or cannot read, so no caller lifts without these. (A caller that wants the raw lift calls
//! `iridia_dxbc::ir::lift` itself and says why.)
//!
//! The error is the lift's own [`Refusal`]; where a pass refuses (an irreducible graph, for one) it is a refusal of the same shape
//! naming the pass.

use iridia_interface::BoundSizes;
use iridia_ir::{Program, Refusal, Unit};

fn prepared(mut program: Program, bytecode: &'static str) -> Result<Program, Refusal> {
    iridia_passes::prepare(&mut program).map_err(|why| Refusal::new(bytecode, Unit::Word, 0, why))?;
    Ok(program)
}

/// A DXBC container's program, lifted and prepared.
pub fn lift_dxbc(bytes: &[u8]) -> Result<Program, Refusal> {
    iridia_dxbc::ir::lift(bytes).and_then(|p| prepared(p, "DXBC"))
}

/// [`lift_dxbc`] with the constant buffers `bound` names bound at the sizes it states.
pub fn lift_dxbc_with(bytes: &[u8], bound: &BoundSizes) -> Result<Program, Refusal> {
    iridia_dxbc::ir::lift_with(bytes, bound).and_then(|p| prepared(p, "DXBC"))
}

/// A DXIL container's program, lifted and prepared.
pub fn lift_dxil(bytes: &[u8]) -> Result<Program, Refusal> {
    iridia_dxil::ir::lift(bytes).and_then(|p| prepared(p, "DXIL"))
}

/// [`lift_dxil`] with the constant buffers `bound` names bound at the sizes it states.
pub fn lift_dxil_with(bytes: &[u8], bound: &BoundSizes) -> Result<Program, Refusal> {
    iridia_dxil::ir::lift_with(bytes, bound).and_then(|p| prepared(p, "DXIL"))
}
