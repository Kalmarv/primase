//! CompiledBytecode resources as programs: each DXBC container of a payload ([`frostbite_formats::bytecode`]) as a DXIL
//! core shader with the interface it states.

use frostbite_formats::bytecode::{containers, tag};
use frostbite_formats::{Error, Result};
use iridia_interface::ShaderFormat;
use primase_core::shader::Shader;

use crate::components::FrostbiteBytecode;

const W: &str = "compiled bytecode";

/// A payload's programs: each container as a DXIL core shader with the interface it states, and the bytes before the
/// first container and after each kept as stored ([`FrostbiteBytecode`]). A container whose interface cannot be read
/// is an [`Error::Unsupported`] naming it and the reason.
pub fn read(payload: &[u8]) -> Result<Vec<(Shader, FrostbiteBytecode)>> {
    let spans = containers(payload)?;
    let tag = tag(payload);
    let mut out = Vec::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        let end = span.offset + span.size;
        let next = spans.get(i + 1).map_or(payload.len(), |n| n.offset);
        let container = &payload[span.offset..end];
        let interface = iridia_dxil::interface(container).map_err(|e| {
            Error::Unsupported(format!(
                "{W}: container {i} of {} at {}: {e}",
                spans.len(),
                span.offset
            ))
        })?;
        out.push((
            Shader {
                format: ShaderFormat::Dxil,
                bytecode: container.to_vec(),
                interface,
            },
            FrostbiteBytecode {
                index: i,
                count: spans.len(),
                offset: span.offset,
                tag,
                before: if i == 0 {
                    payload[..span.offset].to_vec()
                } else {
                    Vec::new()
                },
                after: payload[end..next].to_vec(),
            },
        ));
    }
    Ok(out)
}
