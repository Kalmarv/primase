//! A backwards slice of straight-line register writes, with no meaning assigned to the chosen output.
use std::collections::BTreeMap;

use crate::{Register, Result, error, read};
use primase_shader::{Program, Stage};

pub(crate) fn project(bytes: &[u8], register: u32, mask: u8) -> Result<Program> {
    let mut source = read(bytes)?;
    if source.stage != Stage::Vertex {
        return Err(error(0, "vertex output projection requires vs_3_0"));
    }
    let declared = source
        .outputs
        .iter()
        .filter(|o| o.register == register)
        .fold(0, |m, o| m | o.mask);
    if mask == 0 || mask & !declared != 0 {
        return Err(error(0, "vertex output projection mask is empty or not declared"));
    }
    // Validate the full program first: slicing never hides an unsupported instruction or invalid dead output.
    crate::lower::lift(&source, bytes)?;
    let root = Register::Output(register);
    let mut live = BTreeMap::from([(root, mask)]);
    let mut kept = Vec::new();
    for instruction in source.instructions.iter().rev() {
        let Some(mut destination) = instruction.destination else {
            continue;
        };
        let needed = live.get(&destination.register).copied().unwrap_or(0);
        let written = needed & destination.mask;
        if written == 0 {
            continue;
        }
        live.insert(destination.register, needed & !destination.mask);
        destination.mask = written;
        let mut selected = instruction.clone();
        selected.destination = Some(destination);
        for (operand, &s) in selected.sources.iter().enumerate() {
            *live.entry(s.register).or_default() |= s.read_mask(selected.source_mask(operand));
        }
        if destination.register == root {
            selected
                .destination
                .as_mut()
                .ok_or_else(|| error(instruction.word, "projected destination missing"))?
                .register = Register::Target(0);
        }
        kept.push(selected);
    }
    kept.reverse();
    source.instructions = kept;
    source.outputs.clear();
    source.stage = Stage::Pixel;
    let mut program = crate::lower::vertex_output(&source, bytes)?;
    program.provenance.target = "vs_3_0/output-invocation".into();
    program.provenance.lifter = format!("primase-d3d9/vertex-output:o{register}:mask={mask}");
    program.notes.push(format!("derived invocation of original vs_3_0 o{register}, mask {mask:#x}; selected lanes keep original RGBA positions; unselected lanes are placeholders (0,0,0,1), not source outputs"));
    program.notes.sort();
    Ok(program)
}
