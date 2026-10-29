//! Which sampler each texture of a Direct3D program is read through, read from the program's own instructions (`dx`'s
//! `texture_reads`: every sample, gather, level-of-detail query and load that names a texture register): per texture
//! register, the sampler registers its reads name and whether it is loaded. Both games' programs read a texture through
//! one sampler, but a few through two (a shadow map compared through one and read plainly through another), so a texture
//! has a set of samplers.

use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::ShaderFormat;

/// A register: its space and number.
pub type Register = (u32, u32);

/// How a program reads one texture register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureReads {
    /// The sampler registers its samples, gathers and level-of-detail queries name.
    pub samplers: BTreeSet<Register>,
    /// Whether it is also loaded (read by texel, through no sampler).
    pub loaded: bool,
}

/// The texture reads of a program, by texture register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamplerPairs {
    pub textures: BTreeMap<Register, TextureReads>,
    /// Reads whose texture or sampler is not one register: an array of them indexed at run time, or a handle the reader
    /// could not trace to the register it was made for. Their textures are not in [`SamplerPairs::textures`].
    pub untraced: usize,
}

impl SamplerPairs {
    /// The one sampler register texture register `texture` is read through, if it is read through exactly one.
    pub fn sampler_of(&self, texture: Register) -> Option<Register> {
        let reads = self.textures.get(&texture)?;
        match (reads.samplers.first(), reads.samplers.len()) {
            (Some(s), 1) => Some(*s),
            _ => None,
        }
    }
}

/// The sampler pairs of a program's container: Shader Model 5 (`Dxbc`, every register in space 0) or 6 (`Dxil`).
pub fn pairs(format: ShaderFormat, bytes: &[u8]) -> Result<SamplerPairs, String> {
    let mut out = SamplerPairs::default();
    match format {
        ShaderFormat::Dxbc => {
            let program = dxbc_reader::code(bytes)
                .and_then(|code| dxbc_reader::decode(&code))
                .map_err(|e| e.to_string())?;
            for r in dxbc_reader::texture_reads(&program) {
                if r.texture_indexed || r.sampler_indexed {
                    out.untraced = out.untraced.saturating_add(1);
                    continue;
                }
                let t = out.textures.entry((0, r.texture)).or_default();
                match r.sampler {
                    Some(s) => {
                        t.samplers.insert((0, s));
                    }
                    None => t.loaded = true,
                }
            }
        }
        ShaderFormat::Dxil => {
            let chunks = dxil_reader::parts(bytes).map_err(|e| e.to_string())?;
            let module = dxil_reader::module(&chunks, b"DXIL")
                .map_err(|e| e.to_string())?
                .ok_or("a container with no DXIL part")?;
            for r in dxil_reader::sampling::texture_reads(&module).map_err(|e| e.to_string())? {
                let Some(texture) = r.texture else {
                    out.untraced = out.untraced.saturating_add(1);
                    continue;
                };
                // an unordered-access view read with textureLoad is no texture slot
                if texture.class != 0 {
                    continue;
                }
                let Some(register) = texture.register else {
                    out.untraced = out.untraced.saturating_add(1);
                    continue;
                };
                if !r.sampled {
                    out.textures.entry((texture.space, register)).or_default().loaded = true;
                    continue;
                }
                match r.sampler.and_then(|s| Some((s.space, s.register?))) {
                    Some(sampler) => {
                        out.textures
                            .entry((texture.space, register))
                            .or_default()
                            .samplers
                            .insert(sampler);
                    }
                    None => out.untraced = out.untraced.saturating_add(1),
                }
            }
        }
        other => return Err(format!("the sampler pairs of a {other:?} program")),
    }
    Ok(out)
}
