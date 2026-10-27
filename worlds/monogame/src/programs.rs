//! An effect's shaders as primase shader programs: each GLSL text is lifted by `iridia-mojo` with what the
//! effect's tables say about it (the constants' names and offsets, the samplers' slots and the parameters that hold their
//! textures, the vertex attributes' usages).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use iridia::{Program, Stage};
use iridia_mojo::{Attribute, Constant, Hints, Sampler};

use crate::mgfx::{Effect, Profile, Shader, Stage as ShaderStage};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("shader {0}: the effect has {1} shaders")]
    NoSuchShader(usize, usize),
    #[error("the effect is built for DirectX 11: its shaders are DXBC, not the GLSL this reads")]
    NotGlsl,
    #[error("the shader's code is not UTF-8 text")]
    NotText,
    #[error("the shader reads constant buffer {0}, the effect has {1}")]
    NoSuchConstantBuffer(usize, usize),
    #[error("a constant buffer holds parameter {0}, the effect has {1}")]
    NoSuchParameter(usize, usize),
    #[error(
        "the vertex attribute {name} has the usage {usage}, which MonoGame's VertexElementUsage does not have"
    )]
    UnknownUsage { name: String, usage: u8 },
    #[error("{0}")]
    Lift(#[from] iridia_mojo::Error),
}

/// The vertex usage a number names (MonoGame's `VertexElementUsage`), as the semantic a D3D program writes it.
fn semantic(usage: u8) -> Option<&'static str> {
    Some(match usage {
        0 => "POSITION",
        1 => "COLOR",
        2 => "TEXCOORD",
        3 => "NORMAL",
        4 => "BINORMAL",
        5 => "TANGENT",
        6 => "BLENDINDICES",
        7 => "BLENDWEIGHT",
        8 => "DEPTH",
        9 => "FOG",
        10 => "PSIZE",
        11 => "SAMPLE",
        12 => "TESSELLATEFACTOR",
        _ => return None,
    })
}

/// What the effect says about `shader` that its text does not. An index the tables make that they do not hold is an
/// error, as is a vertex usage MonoGame does not have: nothing is left out.
fn hints(effect: &Effect<'_>, shader: &Shader<'_>) -> Result<Hints, Error> {
    let mut constants = Vec::new();
    for &b in &shader.constant_buffers {
        let buffer = effect
            .constant_buffers
            .get(usize::from(b))
            .ok_or(Error::NoSuchConstantBuffer(
                usize::from(b),
                effect.constant_buffers.len(),
            ))?;
        // a parameter holds the registers up to the next one's offset (or the end of the buffer)
        let mut offsets: Vec<u32> = buffer.parameters.iter().map(|&(_, o)| u32::from(o)).collect();
        offsets.sort_unstable();
        offsets.dedup();
        for &(index, offset) in &buffer.parameters {
            let parameter = usize::try_from(index)
                .ok()
                .and_then(|i| effect.parameters.get(i))
                .ok_or(Error::NoSuchParameter(
                    usize::try_from(index).unwrap_or(usize::MAX),
                    effect.parameters.len(),
                ))?;
            let offset = u32::from(offset);
            let end = offsets
                .iter()
                .copied()
                .find(|&o| o > offset)
                .unwrap_or_else(|| u32::from(buffer.size));
            let bytes = end.saturating_sub(offset);
            // a scalar or a vector is one register; its lanes are its columns
            let width = if parameter.rows <= 1 && parameter.elements.is_empty() {
                parameter.columns.clamp(1, 4)
            } else {
                4
            };
            constants.push(Constant {
                name: parameter.name.clone(),
                offset,
                bytes,
                width,
            });
        }
    }
    let mut samplers = Vec::new();
    for s in &shader.samplers {
        let parameter = effect
            .parameters
            .get(usize::from(s.parameter))
            .ok_or(Error::NoSuchParameter(
                usize::from(s.parameter),
                effect.parameters.len(),
            ))?;
        samplers.push(Sampler {
            glsl_name: s.name.clone(),
            slot: u32::from(s.texture_slot),
            name: parameter.name.clone(),
        });
    }
    let mut attributes = Vec::new();
    for a in &shader.attributes {
        let semantic = semantic(a.usage).ok_or_else(|| Error::UnknownUsage {
            name: a.name.clone(),
            usage: a.usage,
        })?;
        attributes.push(Attribute {
            glsl_name: a.name.clone(),
            semantic: semantic.to_owned(),
            index: u32::from(a.index),
        });
    }
    Ok(Hints {
        target: "mgfx 12 opengl".into(),
        constants,
        samplers,
        attributes,
    })
}

/// Shader `index` of `effect` lifted to a program.
pub fn program(effect: &Effect<'_>, index: usize) -> Result<Program, Error> {
    let shader = effect
        .shaders
        .get(index)
        .ok_or(Error::NoSuchShader(index, effect.shaders.len()))?;
    if effect.profile != Profile::OpenGl {
        return Err(Error::NotGlsl);
    }
    let source = std::str::from_utf8(shader.code).map_err(|_| Error::NotText)?;
    let stage = match shader.stage {
        ShaderStage::Vertex => Stage::Vertex,
        ShaderStage::Pixel => Stage::Pixel,
    };
    Ok(iridia_mojo::lift(source, stage, &hints(effect, shader)?)?)
}
