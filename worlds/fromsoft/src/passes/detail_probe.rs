//! A Sekiro G-buffer pixel program run one pixel at a time as the program bake binds it, every texture read answered by
//! the caller ([`primase_shader_exec::probe::AnsweredReads`]): what the program does with what it reads, apart from what
//! any texture holds. The detail form of a material ([`primase_core::detail`]) is read and proven from such runs.
//!
//! The program is resolved as the bake resolves it ([`resolve`], no metaparam: the engine's constants by their roles,
//! frozen as the program bake freezes them, [`layer_bake::program_constants`]), for a material
//! ([`ProgramProbe::with_material`]) or none ([`ProgramProbe::new`]). The material's own words
//! ([`ProgramProbe::material`]) are as the binding writes them, and the caller may set them ([`ProgramProbe::set_words`]):
//! the program reads them as named fields or through `SAT_MtdParam` ([`MaterialForm`]). The engine's own textures read as
//! the bake reads them ([`ProgramProbe::engine`]). The inputs are frozen as stated in [`ProgramProbe::frozen`], the
//! texture coordinate sets take the probe's coordinate unless explicitly overridden by ProgramProbe::set_input.
use std::collections::BTreeMap;

use primase_core::World;
use primase_core::shader::{ConstantRole, OutputBinding, ShaderFormat, StageBinding, TextureSource};
use primase_shader::output_uses::bind_output_uses;
use primase_shader_exec::evaluate::Constants;
use primase_shader_exec::{Layout, PixelShader, ProgramShader, Sampler};

use crate::layer_bake::resolve::{
    ProgramTexture, Unbound, program_texture, register, render_state, resolve, textures_of,
};
use crate::layer_bake::{self, FrozenFrame};
use crate::materials::LayeredMaterial;
use crate::{Result, invalid};

/// How a program reads its material's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialForm {
    /// As named fields, `<shader>_snp_*` (Sekiro's character programs).
    Named,
    /// Through the `SAT_MtdParam` array, at the metaparam's start words.
    Sat,
    /// Neither: it reads no material parameter.
    None,
}

/// Words of the material's in a program's constant buffers: a named field, or the `SAT_MtdParam` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialWords {
    pub name: String,
    pub buffer: u32,
    /// The first word, in the buffer's words.
    pub word: u32,
    pub words: u32,
}

/// A texture coordinate lane of an input register that takes the probe's coordinate.
#[derive(Clone, Copy, Debug)]
struct UvLane {
    register: usize,
    component: usize,
    axis: usize,
}

/// A pixel program bound for probing ([`ProgramProbe::new`]).
#[derive(Debug)]
pub struct ProgramProbe {
    shader: ProgramShader,
    constants: Constants,
    registers: Vec<[u32; 4]>,
    uv: Vec<UvLane>,
    pub form: MaterialForm,
    /// The words the material sets: its named fields or `SAT_MtdParam`, and the constants the binding takes from the
    /// material or leaves unknown (the alpha reference excepted, frozen as the bake freezes it).
    pub material: Vec<MaterialWords>,
    /// The program's other named constants: the engine's, as the bake freezes them
    /// ([`layer_bake::program_constants`]).
    pub engine_fields: Vec<MaterialWords>,
    /// The binding's outputs, by role and target.
    pub outputs: Vec<OutputBinding>,
    /// The resolved roles, including Unknown reasons. A word in `material` is not by itself evidence that
    /// the engine supplies it from the material; callers designing scene controls must inspect this binding.
    pub binding: StageBinding,
    /// The engine's own textures ([`ProgramTexture::Engine`]: the decal textures of the `_Paint` programs), by register,
    /// each the value the bake reads it as (the binding's default, its reason counted in [`ProgramProbe::policies`]):
    /// the answer a caller gives them, the rest of the program's textures being the material's.
    pub engine: BTreeMap<u32, [f32; 4]>,
    /// Each input frozen, and the value it is frozen to: what a result shown from these runs holds constant.
    pub frozen: Vec<String>,
    /// What the binding counted.
    pub policies: BTreeMap<String, u64>,
}

/// An input lane's value for the frozen frame, by the input's semantic; `None` for a texture coordinate set (the probe's
/// coordinate) and an error for an input the probe does not freeze.
fn frozen_input(
    name: &str,
    index: u32,
    lanes: u8,
) -> std::result::Result<Option<([f32; 4], String)>, String> {
    let name = name.to_ascii_uppercase();
    Ok(Some(match name.as_str() {
        // a set of two or more lanes is a coordinate set; a single lane is a value the vertex stage passes
        "TEXCOORD" if lanes >= 2 => return Ok(None),
        "TEXCOORD" => ([0.; 4], format!("TEXCOORD{index} 0")),
        // tangent space is the probe's space: the normal +z, the tangent +x (its w the bitangent's sign, +1)
        "NORMAL" => ([0., 0., 1., 0.], format!("NORMAL{index} (0, 0, 1)")),
        "TANGENT" => ([1., 0., 0., 1.], format!("TANGENT{index} (1, 0, 0, 1)")),
        "COLOR" => ([1.; 4], format!("COLOR{index} white")),
        "SV_POSITION" | "POSITION" => ([0.5, 0.5, 0.5, 1.], format!("{name}{index} (0.5, 0.5, 0.5, 1)")),
        "SV_ISFRONTFACE" => (
            [f32::from_bits(u32::MAX), 0., 0., 0.],
            "SV_ISFRONTFACE front".to_owned(),
        ),
        _ => return Err(format!("no frozen value for input {name}{index}")),
    }))
}

impl ProgramProbe {
    /// Sekiro's DXBC pixel program `bytes`, resolved as the bake resolves it with no material stated, its material's words
    /// zero until set.
    pub fn new(bytes: &[u8]) -> Result<Self> {
        let unstated = LayeredMaterial {
            mtd: "probe".into(),
            definition: crate::mtd::MaterialDefinition {
                shader: String::new(),
                description: String::new(),
                parameters: Vec::new(),
                textures: Vec::new(),
                block_lengths: Vec::new(),
            },
            bindings: BTreeMap::new(),
        };
        Self::with_material(bytes, &unstated)
    }

    /// Sekiro's DXBC pixel program `bytes` bound as the bake binds it for `material`: its material's words the
    /// material's, as the one binding writes them (a colour times its intensity), those it states none for zero until
    /// set.
    pub fn with_material(bytes: &[u8], material: &LayeredMaterial) -> Result<Self> {
        let shader = layer_bake::program_executor(ShaderFormat::Dxbc, bytes)?;
        let mut world = World::default();
        let program = world.spawn();
        let resolution = resolve(
            &shader,
            program,
            &material.view(),
            crate::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )?;
        let mut policies = resolution.policies;
        let interface = PixelShader::interface(&shader).clone();
        // the outputs as the resolve pass binds them: a lane left Unknown is Coverage where the program's discard tests it,
        // Opacity where the material's blend weighs by it
        let mut outputs = resolution.stage.outputs.clone();
        bind_output_uses(
            &mut outputs,
            &interface,
            shader.program().graph().ok().as_ref(),
            &render_state(material),
        );
        let constants = layer_bake::program_constants(
            &shader,
            &interface,
            &resolution.stage,
            &FrozenFrame::static_bake(0.),
            &mut policies,
        )?;
        let layout = Layout::of(&shader).map_err(|e| invalid("detail probe", e.at, e.detail))?;
        let named: Vec<MaterialWords> = layout
            .fields
            .iter()
            .filter(|(n, _)| n.contains("_snp_") && !n.ends_with("_PADDING"))
            .map(|(n, f)| MaterialWords {
                name: n.clone(),
                buffer: f.buffer,
                word: f.offset,
                words: f.words,
            })
            .collect();
        let sat = layout.fields.get("SAT_MtdParam").map(|f| MaterialWords {
            name: "SAT_MtdParam".into(),
            buffer: f.buffer,
            word: f.offset,
            words: f.words,
        });
        let (form, mut words) = match (named.is_empty(), sat) {
            (false, _) => (MaterialForm::Named, named),
            (true, Some(sat)) => (MaterialForm::Sat, vec![sat]),
            (true, None) => (MaterialForm::None, Vec::new()),
        };
        // the constants the binding takes from the material, or leaves unknown where it states none, are the
        // material's too (a parameter the engine names `FC_*`, such as the diffuse colour `FC_DifMapMulCol`)
        for c in &resolution.stage.constants {
            let (ConstantRole::Material(_) | ConstantRole::Unknown { .. }) = c.role else {
                continue;
            };
            // the alpha reference the bake freezes ([`layer_bake::ALPHA_TEST_POLICY`]) stays frozen
            let Some(name) = c.name.clone().filter(|n| n != "FC_AlphaRef") else {
                continue;
            };
            let Some(f) = layout.fields.get(&name) else {
                continue;
            };
            if !words.iter().any(|m| m.name == name) {
                words.push(MaterialWords {
                    name,
                    buffer: f.buffer,
                    word: f.offset,
                    words: f.words,
                });
            }
        }
        let engine_fields = layout
            .fields
            .iter()
            .filter(|(n, _)| {
                !n.ends_with("_PADDING") && *n != "SAT_MtdParam" && !words.iter().any(|m| m.name == **n)
            })
            .map(|(n, f)| MaterialWords {
                name: n.clone(),
                buffer: f.buffer,
                word: f.offset,
                words: f.words,
            })
            .collect();
        let mut engine = BTreeMap::new();
        for resource in textures_of(&interface) {
            if let ProgramTexture::Engine(name) = program_texture(&material.view(), resource) {
                let value = resolution
                    .stage
                    .textures
                    .iter()
                    .find(|b| b.resource == resource.id)
                    .and_then(|b| match b.source {
                        TextureSource::Default { value, .. } => Some(value),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        invalid(
                            "detail probe",
                            0,
                            format!("engine texture {name} is not bound to a value"),
                        )
                    })?;
                engine.insert(register(resource)?, value);
            }
        }
        let mut registers = vec![[0u32; 4]; shader.input_count()];
        let mut uv = Vec::new();
        let mut frozen = Vec::new();
        for v in interface.inputs.iter().filter(|v| v.used) {
            let d3d = v.provenance.d3d.ok_or_else(|| {
                invalid(
                    "detail probe",
                    0,
                    format!("input {}{} has no register", v.label.name, v.label.index),
                )
            })?;
            let register = d3d.register as usize;
            if register >= registers.len() {
                return Err(invalid(
                    "detail probe",
                    0,
                    "an input past the program's registers",
                ));
            }
            let components: Vec<usize> = (0..4).filter(|c| d3d.mask & (1 << c) != 0).collect();
            match frozen_input(&v.label.name, v.label.index, components.len() as u8)
                .map_err(|e| invalid("detail probe", 0, e))?
            {
                Some((value, what)) => {
                    for (k, &c) in components.iter().enumerate() {
                        registers[register][c] = value[k].to_bits();
                    }
                    frozen.push(what);
                }
                None => uv.extend(components.iter().enumerate().map(|(k, &component)| UvLane {
                    register,
                    component,
                    axis: k % 2,
                })),
            }
        }
        Ok(ProgramProbe {
            shader,
            constants,
            registers,
            uv,
            form,
            material: words,
            engine_fields,
            // the binding as resolved, with the output uses the discard and the blend show bound as the probe reads them
            binding: StageBinding {
                outputs: outputs.clone(),
                ..resolution.stage
            },
            outputs,
            engine,
            frozen,
            policies,
        })
    }

    /// The program's interface.
    pub fn interface(&self) -> &primase_shader::Interface {
        PixelShader::interface(&self.shader)
    }

    /// Writes `words` of the buffer `buffer` from word `word` (a material's).
    pub fn set_words(&mut self, buffer: u32, word: u32, words: &[u32]) -> Result<()> {
        self.constants
            .set_words(buffer, word, words)
            .map_err(|e| invalid("detail probe", e.at, e.detail))
    }

    /// The `count` words of the buffer `buffer` from word `word`, as set; `None` past the buffer.
    pub fn words(&self, buffer: u32, word: u32, count: u32) -> Option<Vec<u32>> {
        let (_, rows) = self
            .constants
            .buffers()
            .iter()
            .find(|(slot, _)| *slot == buffer)?;
        (word..word + count)
            .map(|w| rows.get(w as usize / 4).map(|row| row[w as usize % 4]))
            .collect()
    }

    /// Freezes the used input `name``index` to `value` instead. This explicitly overrides the coordinate supplied
    /// to `pixel` for these lanes; other coordinate inputs still take the probe coordinate. Values are in semantic
    /// lane order, including when two semantics occupy different lanes of one register.
    pub fn set_input(&mut self, name: &str, index: u32, value: [f32; 4]) -> Result<()> {
        let v = self
            .interface()
            .inputs
            .iter()
            .find(|v| v.used && v.label.name.eq_ignore_ascii_case(name) && v.label.index == index)
            .ok_or_else(|| invalid("detail probe", 0, format!("no used input {name}{index}")))?;
        let d3d = v.provenance.d3d.expect("a used input placed in new");
        let components: Vec<usize> = (0..4).filter(|c| d3d.mask & (1 << c) != 0).collect();
        for (k, c) in components.into_iter().enumerate() {
            self.registers[d3d.register as usize][c] = value[k].to_bits();
        }
        self.uv
            .retain(|lane| lane.register != d3d.register as usize || d3d.mask & (1 << lane.component) == 0);
        Ok(())
    }

    /// One pixel at the coordinate `uv` (every coordinate set except explicit `set_input` overrides), its texture
    /// reads answered by `sampler`: each output register's four lanes, and whether the program discarded the pixel.
    pub fn pixel(&self, uv: [f32; 2], sampler: &mut dyn Sampler) -> Result<(Vec<[f32; 4]>, bool)> {
        let mut registers = self.registers.clone();
        for lane in &self.uv {
            registers[lane.register][lane.component] = uv[lane.axis].to_bits();
        }
        let buffers: Vec<(u32, &[[u32; 4]])> = self
            .constants
            .buffers()
            .iter()
            .map(|(slot, rows)| (*slot, rows.as_slice()))
            .collect();
        let run = self
            .shader
            .execute(&registers, &buffers, sampler)
            .map_err(|e| invalid("detail probe", e.at, e.detail))?;
        Ok((
            run.outputs.iter().map(|o| o.map(f32::from_bits)).collect(),
            run.discarded,
        ))
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    // Original SDK 10.0.26100.0 fxc ps_5_0 fixture, SHA256:
    // 25ce41923b1fa1e04dca67777c454ed3eed6e516bac79e1c7ae186177090e959
    const INPUTS: &[u8] = include_bytes!("../../tests/fixtures/probe_inputs.dxbc");

    fn pixel(probe: &ProgramProbe, uv: [f32; 2]) -> [f32; 4] {
        let mut no_textures = |_: primase_shader_exec::Sample| Err("fixture has no texture".to_owned());
        let (outputs, discarded) = probe.pixel(uv, &mut no_textures).expect("pixel");
        assert!(!discarded);
        outputs[0]
    }

    #[test]
    fn explicit_coordinates_survive_pixel_without_overriding_other_semantics() {
        let mut probe = ProgramProbe::new(INPUTS).expect("probe");
        assert_eq!(pixel(&probe, [0.25, 0.5]), [0.25, 0.5, 0.25, 0.5]);
        probe
            .set_input("texcoord", 1, [0.75, 1., 0., 0.])
            .expect("override");
        assert_eq!(pixel(&probe, [0.125, 0.25]), [0.125, 0.25, 0.75, 1.]);
        assert_eq!(pixel(&probe, [0.5, 0.125]), [0.5, 0.125, 0.75, 1.]);
        probe
            .set_input("TEXCOORD", 1, [0.25, 0.5, 0., 0.])
            .expect("replace");
        assert_eq!(pixel(&probe, [0.5, 0.125]), [0.5, 0.125, 0.25, 0.5]);
        assert!(probe.set_input("TEXCOORD", 9, [0.; 4]).is_err());
    }

    #[test]
    fn setting_a_probe_word_does_not_claim_a_material_role() {
        let mut probe = ProgramProbe::new(INPUTS).expect("probe");
        let binding = probe.binding.clone();
        let role = &binding
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some("FC_GlowScale"))
            .expect("field")
            .role;
        assert!(matches!(role, ConstantRole::Unknown { why } if !why.is_empty()));
        probe
            .set_words(0, 0, &[1f32.to_bits(); 4])
            .expect("designed answer");
        assert_eq!(pixel(&probe, [0.25, 0.5]), [1.25, 1.5, 1.25, 1.5]);
        assert_eq!(probe.binding, binding);
    }
}
