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

use monogame_formats::mgfx::{Effect, Profile, Shader, Stage as ShaderStage};

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

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::unwrap_used,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use monogame_formats::mgfx::{
        Attribute, ConstantBuffer, Effect, Parameter, Pass, Profile, Sampler, SamplerKind, Shader, Stage,
        Technique,
    };

    use super::{Error as ProgramError, program};

    fn parameter(class: u8, ty: u8, name: &str, columns: u8, defaults: Option<Vec<f32>>) -> Parameter {
        Parameter {
            class,
            ty,
            name: name.into(),
            semantic: String::new(),
            rows: 1,
            columns,
            elements: Vec::new(),
            members: Vec::new(),
            defaults,
        }
    }

    /// A small effect: one constant buffer holding one parameter, a vertex shader with two attributes and a pixel
    /// shader with a sampler, one technique with one pass.
    fn effect() -> Effect<'static> {
        let shader = |stage, code: &'static [u8]| Shader {
            stage,
            source_file: String::new(),
            entry_point: String::new(),
            code,
            samplers: Vec::new(),
            constant_buffers: Vec::new(),
            attributes: Vec::new(),
        };
        let mut pixel = shader(
            Stage::Pixel,
            b"void main() {}
",
        );
        pixel.constant_buffers = vec![0];
        pixel.samplers = vec![Sampler {
            kind: SamplerKind::Sampler2D,
            texture_slot: 3,
            sampler_slot: 3,
            state: None,
            name: "ps_s0".into(),
            parameter: 1,
        }];
        let mut vertex = shader(
            Stage::Vertex,
            b"void main() { }
",
        );
        vertex.attributes = [("vs_v0", 0u8, 0u8), ("vs_v1", 2, 1)]
            .into_iter()
            .map(|(name, usage, index)| Attribute {
                name: name.into(),
                usage,
                index,
                location: i16::from(index),
            })
            .collect();
        Effect {
            profile: Profile::OpenGl,
            key: 0x1234_5678,
            constant_buffers: vec![ConstantBuffer {
                name: "ps_uniforms_vec4".into(),
                size: 16,
                parameters: vec![(0, 0)],
            }],
            shaders: vec![pixel, vertex],
            parameters: vec![
                parameter(1, 3, "Tint", 4, Some(vec![0.25, 0.5, 0.75, 1.0])),
                parameter(3, 7, "Albedo", 1, None),
            ],
            techniques: vec![Technique {
                name: "Render".into(),
                passes: vec![Pass {
                    name: "Pass".into(),
                    vertex_shader: Some(1),
                    pixel_shader: Some(0),
                }],
            }],
        }
    }

    #[test]
    fn a_program_is_not_made_of_tables_that_do_not_hold_what_they_name() {
        let good = effect();
        // the pixel shader (0) reads buffer 0, whose parameter is made one the effect does not have
        let mut effect = good.clone();
        effect.constant_buffers[0].parameters[0].0 = 9;
        assert!(matches!(
            program(&effect, 0),
            Err(ProgramError::NoSuchParameter(9, 2))
        ));
        // a sampler's parameter
        let mut effect = good.clone();
        effect.shaders[0].samplers[0].parameter = 7;
        assert!(matches!(
            program(&effect, 0),
            Err(ProgramError::NoSuchParameter(7, 2))
        ));
        // a constant buffer the shader reads
        let mut effect = good.clone();
        effect.shaders[0].constant_buffers[0] = 3;
        assert!(matches!(
            program(&effect, 0),
            Err(ProgramError::NoSuchConstantBuffer(3, 1))
        ));
        // a vertex usage MonoGame does not have
        let mut effect = good;
        effect.shaders[1].attributes[0].usage = 99;
        let error = program(&effect, 1).unwrap_err().to_string();
        assert!(error.contains("vs_v0") && error.contains("usage 99"), "{error}");
    }
}
