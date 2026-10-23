//! Elden Ring's materials bound to their G-buffer programs (`resolve-fromsoft-shaders` on the read of the Academy of
//! Raya Lucaria, m14_00_00_00, and the Liurnia tile m60_35_46_00), checked against their material binaries, their
//! shader bundles' metaparams and the programs' own interfaces:
//! - every parameter the metaparam declares as a shader input sits at its start word, inside the program's own
//!   `SAT_MtdParam` array, and holds the material binary's value (else the metaparam's default; a colour's first
//!   three lanes times its intensity);
//! - each texture slot's tiling is its uv group's `<group>_CommonUV-UVParam` where the material binary states it, the
//!   binding's stored scale otherwise;
//! - no texture the material binary declares is bound as an engine texture (a program resource without a
//!   `Texture2D_<n>` name falls to that branch: they are listed by name);
//! - the outputs carry Elden Ring's G-buffer roles only ([`Game::gbuffer`]).
//!
//! It prints how many materials were bound and why the others were not.
use primase_core::World;
use primase_core::shader::{ConstantRole, Shader, ShaderBinding, TextureSource};
use primase_fromsoft::materials::{Definition, Material, ParameterValue};
use primase_fromsoft::sekiro::source::{MaterialShader, ShaderBundle};
use primase_fromsoft::{Game, Install, Keys, dcx::Decoder, sekiro};
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

fn floats(value: &ParameterValue) -> Vec<f32> {
    match value {
        ParameterValue::Bool(b) => vec![f32::from(u8::from(*b))],
        ParameterValue::Int(v) => v.iter().map(|&i| i as f32).collect(),
        ParameterValue::Float(v) => v.clone(),
    }
}

#[test]
fn elden_ring_materials_bind_as_their_definitions_state() -> Result<()> {
    let (Some(root), Some(keys), Some(dll)) = (
        primase_testdata::install(primase_testdata::Game::Elden),
        primase_testdata::keys(primase_testdata::Game::Elden),
        primase_testdata::tool(primase_testdata::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = Install::new(root, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let layout = Game::EldenRing.gbuffer();
    let mut failures = Vec::new();
    let mut engine_textures: BTreeMap<String, usize> = BTreeMap::new();
    for map in ["m14_00_00_00", "m60_35_46_00"] {
        let mut world = World::default();
        sekiro::read_game_area(&install, map, &decoder, &sekiro::Options::default(), &mut world)?;
        let report = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
        println!("{map}: {:?}", report.steps[0].log);
        let (mut bound, mut unbound) = (0, 0);
        for (e, m) in world.iter::<Material>() {
            let Some(Definition::Matbin(definition)) = &m.definition else {
                continue;
            };
            let Some(binding) = world.get::<ShaderBinding>(e) else {
                unbound += 1;
                continue;
            };
            bound += 1;
            let what = |detail: String| format!("{map}: {} ({}): {detail}", m.mtd, m.shader);
            let pixel = binding.stages.last().ok_or("no stage")?;
            let shader = world.get::<Shader>(pixel.shader).ok_or("no shader")?;
            let name_of = |id| {
                shader
                    .interface
                    .resources
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| r.provenance.name.clone())
            };
            let metaparam = world
                .get::<MaterialShader>(e)
                .and_then(|s| world.get::<ShaderBundle>(s.0))
                .and_then(|b| b.parameters.as_ref());
            let constant = |name: &str| pixel.constants.iter().find(|c| c.name.as_deref() == Some(name));
            // the SAT array's place in the program's own buffer
            // the program's `SAT_MtdParam`: its buffer and the words its members (the components it reads) occupy
            let array = shader.interface.resources.iter().find_map(|r| match &r.kind {
                primase_core::shader::ResourceKind::ConstantBuffer { layout } => layout
                    .constants
                    .iter()
                    .find(|c| c.name == "SAT_MtdParam")
                    .map(|c| {
                        let read: BTreeSet<u32> =
                            c.ty.members()
                                .iter()
                                .flat_map(|m| {
                                    let at = c.offset + m.offset;
                                    at / 4..(at + m.ty.size().unwrap_or(4)).div_ceil(4)
                                })
                                .collect();
                        (r.id, read)
                    }),
                _ => None,
            });
            if let Some(metaparam) = metaparam {
                // every word the program reads is a parameter's the metaparam places there
                if let Some((_, read)) = &array {
                    for word in read {
                        let placed = metaparam
                            .mtd_params
                            .iter()
                            .any(|p| !p.not_shader_input && p.start <= *word && *word < p.start + 5);
                        if !placed {
                            failures.push(what(format!(
                                "SAT_MtdParam word {word}: no metaparam parameter there"
                            )));
                        }
                    }
                }
                for p in metaparam.mtd_params.iter().filter(|p| !p.not_shader_input) {
                    // a parameter the program does not read is not bound
                    if !array.as_ref().is_some_and(|(_, read)| read.contains(&p.start)) {
                        if constant(&p.name).is_some() {
                            failures.push(what(format!("{} bound, the program does not read it", p.name)));
                        }
                        continue;
                    }
                    let Some(c) = constant(&p.name) else {
                        failures.push(what(format!(
                            "{} not bound (program {}, constants {:?})",
                            p.name,
                            world
                                .get::<primase_core::Name>(pixel.shader)
                                .map_or("", |n| n.0.as_str()),
                            pixel
                                .constants
                                .iter()
                                .filter_map(|c| c.name.as_deref())
                                .collect::<Vec<_>>()
                        )));
                        continue;
                    };
                    if let Some((resource, _)) = &array
                        && (c.resource != *resource || c.offset != p.start * 4)
                    {
                        failures.push(what(format!(
                            "{} at {} of {:?}, its start word {}",
                            p.name, c.offset, c.resource, p.start
                        )));
                    }
                    let stated = definition_value(m, &p.name);
                    let mut want = stated.clone().unwrap_or(p.default.to_vec());
                    if p.is_colour() {
                        let intensity = if want.len() >= 5 { want[4] } else { p.default[4] };
                        want.resize(4, 1.);
                        for v in &mut want[..3] {
                            *v *= intensity;
                        }
                    }
                    let ConstantRole::Material(words) = &c.role else {
                        failures.push(what(format!("{} is not the material's", p.name)));
                        continue;
                    };
                    let got: Vec<f32> = words.iter().map(|w| f32::from_bits(*w)).collect();
                    if got[..] != want[..got.len().min(want.len())] {
                        failures.push(what(format!("{} = {got:?}, the definition's {want:?}", p.name)));
                    }
                }
                // tiling by uv group
                for t in &pixel.textures {
                    let Some(name) = name_of(t.resource) else { continue };
                    let Some(slot) = name
                        .rsplit_once("Texture2D_")
                        .and_then(|(_, s)| s.parse::<u8>().ok())
                    else {
                        continue;
                    };
                    let Some(group) = metaparam
                        .textures
                        .iter()
                        .find(|x| x.slot == slot && !x.uv_group.is_empty())
                        .map(|x| &x.uv_group)
                    else {
                        continue;
                    };
                    let Some(ParameterValue::Float(uv)) = m
                        .definition
                        .as_ref()
                        .and_then(|d| d.as_ref().parameter(&format!("{group}_CommonUV-UVParam")))
                    else {
                        continue;
                    };
                    let register = shader
                        .interface
                        .resources
                        .iter()
                        .find(|r| r.id == t.resource)
                        .and_then(|r| r.provenance.d3d)
                        .map(|b| b.register)
                        .ok_or("texture register")?;
                    if let Some(c) = pixel.constants.iter().find(|c| {
                        c.name.as_deref() == Some("FC_TextureTilingScale") && c.offset == register * 16
                    }) && let ConstantRole::Material(words) = &c.role
                        && [f32::from_bits(words[0]), f32::from_bits(words[1])] != [uv[0], uv[1]]
                    {
                        failures.push(what(format!("{name}: tiling {words:?}, uv group {group} {uv:?}")));
                    }
                }
            }
            // engine textures: none the material binary declares
            let declared: BTreeSet<&str> = definition.samplers.iter().map(|s| s.kind.as_str()).collect();
            for t in &pixel.textures {
                if let TextureSource::Default { why, .. } = &t.source
                    && why.starts_with("engine texture")
                {
                    let name = name_of(t.resource).unwrap_or_default();
                    *engine_textures.entry(name.clone()).or_default() += 1;
                    if declared.contains(name.as_str()) || name.contains("Texture2D_") {
                        failures.push(what(format!("{name} bound as an engine texture")));
                    }
                }
            }
            // outputs: Elden Ring's roles only
            for o in &pixel.outputs {
                if !layout
                    .iter()
                    .any(|(_, lanes, role)| *lanes == o.lanes && *role == o.role)
                {
                    failures.push(what(format!(
                        "output {:?} {:?} not Elden Ring's",
                        o.label, o.role
                    )));
                }
            }
        }
        println!("{map}: material binaries bound {bound}, not bound {unbound}");
        assert!(bound > 0, "{map}: no material binary bound");
    }
    println!("engine textures (by program resource name): {engine_textures:?}");
    for f in failures.iter().take(40) {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    Ok(())
}

/// The definition's value of parameter `name` as floats, where it states it.
fn definition_value(m: &Material, name: &str) -> Option<Vec<f32>> {
    m.definition
        .as_ref()?
        .as_ref()
        .parameter(name)
        .map(|v| floats(&v))
}
