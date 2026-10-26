//! What a compiled pixel program does, read from its microcode and not from its name.
//!
//! A program that samples one texture at the first texture coordinate and only copies or multiplies the result with
//! uniforms reads no normal, light, fog or other interpolant: whatever it draws does not depend on lighting. Every
//! other program is not classified (the answer is "not known", not "lit").
use crate::components::PixelProgram;
use crate::{Result, invalid};
use primase_rsx::cgbinary::{self, Stage};
use primase_rsx::fragment::{self, Opcode, SourceKind};

/// The fragment input register of the first texture coordinate (RSX: position, two colours, fog, then TEX0).
const TEX0: u8 = 4;

/// Classify a `.psp` payload: fragment programs, each followed by zero padding to 64 bytes. It is a class only when
/// every program in it has that class; a file of differing programs is not classified.
pub fn classify(payload: &[u8]) -> Result<PixelProgram> {
    let mut at = 0;
    let mut class: Option<PixelProgram> = None;
    while at < payload.len() {
        let program = cgbinary::parse_prefix(&payload[at..]).map_err(|e| invalid(at + e.offset, e.detail))?;
        let end = at + program.bytes.len();
        let padded = end
            .checked_add(63)
            .ok_or_else(|| invalid(at, "padding overflow"))?
            & !63;
        let padding = payload
            .get(end..padded)
            .ok_or_else(|| invalid(end, "truncated 64-byte padding"))?;
        if padding.iter().any(|&b| b != 0) {
            return Err(invalid(end, "nonzero padding after a program"));
        }
        at = padded;
        let this = match program.stage {
            Stage::Fragment(_) => classify_program(&program)?,
            Stage::Vertex(_) => PixelProgram::Unclassified,
        };
        match &class {
            Some(first) if *first != this => return Ok(PixelProgram::Unclassified),
            _ => class = Some(this),
        }
    }
    Ok(class.unwrap_or(PixelProgram::Unclassified))
}

fn classify_program(program: &cgbinary::Program<'_>) -> Result<PixelProgram> {
    let code = fragment::decode(program.microcode).map_err(|e| invalid(e.offset, e.detail))?;
    let mut unit = None;
    let mut uniforms = Vec::new();
    for i in &code.instructions {
        // always executed, no branches or loops, no condition-code update
        if i.predicate.test != 7 || i.control.is_some() || i.update_condition {
            return Ok(PixelProgram::Unclassified);
        }
        match i.opcode {
            Opcode::TEX => {
                let coordinate = i.sources.first().filter(|s| {
                    s.kind == SourceKind::Input(TEX0) && s.swizzle == [0, 1, 2, 3] && !s.negate && !s.absolute
                });
                if unit.is_some() || coordinate.is_none() || i.perspective_input || i.indexed_input {
                    return Ok(PixelProgram::Unclassified);
                }
                unit = i.texture_unit;
            }
            Opcode::MOV | Opcode::MUL => {
                if i.sources.iter().any(|s| matches!(s.kind, SourceKind::Input(_))) {
                    return Ok(PixelProgram::Unclassified);
                }
                if i.constant.is_some() {
                    let slot = u32::try_from(i.offset + 16).map_err(|_| invalid(i.offset, "offset"))?;
                    let name = program
                        .parameters
                        .iter()
                        .find(|p| p.embedded_constants.contains(&slot))
                        .and_then(|p| p.name)
                        .map(|n| String::from_utf8_lossy(n).into_owned());
                    match name {
                        Some(n) => {
                            if !uniforms.contains(&n) {
                                uniforms.push(n);
                            }
                        }
                        None => return Ok(PixelProgram::Unclassified),
                    }
                }
            }
            _ => return Ok(PixelProgram::Unclassified),
        }
    }
    Ok(match unit {
        Some(unit) => PixelProgram::ScaledTexture {
            unit: u32::from(unit),
            uniforms,
        },
        None => PixelProgram::Unclassified,
    })
}
