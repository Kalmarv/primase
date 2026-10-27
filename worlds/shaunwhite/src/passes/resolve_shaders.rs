//! Exact generic-material constraints on SDB2's stored forward-colour keys. Native setters determine
//! material bits; draw/frame bits remain explicit record variants. Nothing selects a nearby permutation.
use iridia_interface::{
    ConstantBinding, ConstantRole, InputBinding, InputRole, Interface, KeyPart, OutputBinding, OutputRole,
    ProgramResourceFacts, RenderState, ResourceKind, ShaderFormat, Stage, VariantKey,
};
use std::collections::{BTreeMap, BTreeSet};

use crate::components::{
    MaterialRef, MaterialShader, MeshFormat, RealTree, ShaderInputs, ShaderLibrary, ShaderPasses,
};
use anvil_formats::shaunwhite::shaders::{Effect, Payload};
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::shader::*;
use primase_core::{Component, Entity, Mesh, Morphs, Name, Resource as _, World};

pub const RECORD_FIELD: &str = "shaunwhite.shader.record";
#[derive(Clone, Copy, Debug)]
pub struct ResolveShaunwhiteShaders;
pub const RESOLVE_SHAUNWHITE_SHADERS: PassSpec = PassSpec {
    name: "resolve-shaunwhite-shaders",
    options: &[],
    make: |_| Ok(Box::new(ResolveShaunwhiteShaders)),
};

/// The native generic material's required bits and values, translated from global define IDs to
/// family-3 packing (template in bits0..5). A present normal map is permitted, not required: the native
/// required/allowed keys differ at NORMALMAP. Fog, lights, influence, static lighting and pass bits
/// are deliberately not material constraints. The returned pair is `(mask, value)`.
pub fn material_key(
    m: &MaterialRef,
    f: &anvil_formats::shaunwhite::shader_material::MaterialShader,
) -> Result<(u64, u64), String> {
    let s = m.state.ok_or("material fields not read")?;
    if s.water {
        return Err("water family is not generic".into());
    }
    if !f.unsupported.is_empty() {
        return Err(f.unsupported.join(", "));
    }
    let mut mask = 63 | (1 << 6) | (1 << 25); // generic, no skin/morph
    let mut value = 3;
    let mut bit = |at: u8, on: bool| {
        mask |= 1u64 << at;
        if on {
            value |= 1u64 << at;
        }
    };
    let has = |name: &str| f.texture_slots.iter().any(|s| s == name);
    let unlit = f.flags & (1 << 29) != 0;
    let no_specular = unlit || s.specular_level == 0.;
    for (at, present) in [
        (9, has("diffuse")),
        (10, f.layers[0] && has("diffuse_b")),
        (11, f.layers[1] && has("diffuse_c")),
        (13, !no_specular && has("specular")),
        (14, !no_specular && f.layers[0] && has("specular_b")),
        (15, !no_specular && has("generic")),
        (16, has("mask")),
        (19, s.alpha_test),
        (30, f.layers[0]),
        (31, f.layers[1]),
        (36, f.half_lambert && !unlit),
        (41, f.hide != 0),
        (43, f.layer3_no_specular && f.layers[1] && !no_specular),
        (46, f.dynamic_uv == 1),
        (47, f.dynamic_uv == 2),
        (49, f.glossiness && !no_specular),
        (50, no_specular),
        (56, f.hide == 2),
        (57, f.rim_alpha && unlit && f.flags & 16 != 0 && !f.half_lambert),
    ] {
        bit(at, present);
    }
    if !has("normal") {
        bit(12, false);
    }
    // Dynamic UV position-offset is another material switch; this slice admits no dynamic UV draws.
    if f.dynamic_uv != 0 {
        return Err("dynamic UV material key not resolved".into());
    }
    bit(48, false);
    Ok((mask, value))
}

fn slot(parameter: &str) -> Option<&'static str> {
    Some(match parameter {
        "g_Layer1_DiffuseTexture" => "diffuse",
        "g_Layer2_DiffuseTexture" => "diffuse_b",
        "g_Layer3_DiffuseTexture" => "diffuse_c",
        "g_Layer1_NormalTexture" => "normal",
        "g_Layer1_SpecularTexture" => "specular",
        "g_Layer2_SpecularTexture" => "specular_b",
        "g_MaskTexture" => "mask",
        "g_EnvTexture" => "generic",
        _ => return None,
    })
}

fn constant(m: &MaterialRef, name: &str, size: u32) -> ConstantRole {
    let value = m.state.and_then(|s| {
        Some(match name {
            "g_Layer1_ModulationColor" => s.color,
            "g_Layer2_ModulationColor" => s.layering.colors[0],
            "g_Layer3_ModulationColor" => s.layering.colors[1],
            _ => return None,
        })
    });
    if let Some(v) = value
        && size == 16
    {
        ConstantRole::Material(v.map(f32::to_bits).to_vec())
    } else {
        ConstantRole::Unknown {
            why: format!("{name}: native frame/object/material constant not resolved"),
        }
    }
}

fn binding(
    m: &MaterialRef,
    effect: &Effect,
    record: u32,
    key: u64,
    shader: Entity,
    interface: &Interface,
    unresolved: &mut BTreeMap<String, u64>,
) -> StageBinding {
    let mut b = StageBinding {
        stage: Stage::Pixel,
        variant: Some(VariantKey {
            field: RECORD_FIELD.into(),
            value: record,
            parts: vec![
                KeyPart {
                    name: "SDB2 permutation".into(),
                    value: key,
                },
                KeyPart {
                    name: "native pixel cache key".into(),
                    value: effect.pixel_key,
                },
            ],
        }),
        shader,
        inputs: Vec::new(),
        constants: Vec::new(),
        textures: Vec::new(),
        buffers: Vec::new(),
        constant_buffers: Vec::new(),
        outputs: Vec::new(),
    };
    for v in &interface.inputs {
        let why = format!(
            "{}{}: packed/transformed vertex output not resolved",
            v.label.name, v.label.index
        );
        if v.used {
            *unresolved.entry(why.clone()).or_default() += 1;
        }
        b.inputs.push(InputBinding {
            label: v.label.clone(),
            role: InputRole::Unknown { why },
        });
    }
    for r in &interface.resources {
        match &r.kind {
            ResourceKind::ConstantBuffer { layout } => {
                let mut spans = Vec::new();
                for c in &layout.constants {
                    let Some(size) = c.ty.size() else { continue };
                    let role = constant(m, &c.name, size);
                    if let ConstantRole::Unknown { why } = &role {
                        *unresolved.entry(why.clone()).or_default() += 1;
                    }
                    b.constants.push(ConstantBinding {
                        resource: r.id,
                        offset: c.offset,
                        size,
                        name: Some(c.name.clone()),
                        role,
                        lines: None,
                    });
                    spans.push((c.offset, c.offset + size));
                }
                spans.sort_unstable();
                let mut end = 0;
                spans.push((layout.size, layout.size));
                for (start, next) in spans {
                    if start > end {
                        let why = "unreflected float constant registers".to_owned();
                        *unresolved.entry(why.clone()).or_default() += 1;
                        b.constants.push(ConstantBinding {
                            resource: r.id,
                            offset: end,
                            size: start - end,
                            name: None,
                            role: ConstantRole::Unknown { why },
                            lines: None,
                        });
                    }
                    end = end.max(next);
                }
            }
            ResourceKind::Texture { .. } => {
                let named = effect
                    .samplers
                    .iter()
                    .chain(&effect.additional_samplers)
                    .find(|s| s.sampler == r.provenance.name);
                let source = named
                    .ok_or_else(|| "sampler has no SDB2 binding metadata".to_owned())
                    .and_then(|s| {
                        let address = r
                            .provenance
                            .d3d
                            .as_ref()
                            .ok_or("sampler has no reflected register")?;
                        if s.address != (0x0080_0000 | address.register) {
                            return Err(format!(
                                "packed address {:#x} disagrees with s{}",
                                s.address, address.register
                            ));
                        }
                        let slot = slot(&s.texture).ok_or("frame/texture parameter not mapped")?;
                        let texture = m
                            .slots
                            .get(slot)
                            .ok_or_else(|| format!("{slot} texture not read"))?
                            .texture;
                        let sampler = iridia_d3d9::sampler_states(s.states.iter().map(|s| (s.kind, s.value)))
                            .map_err(|e| e.to_string())?;
                        Ok(TextureSource::Texture { texture, sampler })
                    })
                    .unwrap_or_else(|why| {
                        let parameter = named.map_or(r.provenance.name.as_str(), |s| s.texture.as_str());
                        *unresolved
                            .entry(format!("texture {parameter}: {why}"))
                            .or_default() += 1;
                        TextureSource::Missing
                    });
                let paired = interface
                    .resources
                    .iter()
                    .find(|s| {
                        matches!(s.kind, ResourceKind::Sampler { .. })
                            && s.provenance.name == r.provenance.name
                    })
                    .map(|s| s.id);
                b.textures.push(TextureBinding {
                    resource: r.id,
                    sampler: paired,
                    source,
                });
            }
            _ => {}
        }
    }
    for v in &interface.outputs {
        let mask = (1u8 << v.value.width) - 1;
        if v.label.index == 0 {
            b.outputs.push(OutputBinding {
                label: v.label.clone(),
                lanes: mask & 7,
                role: OutputRole::Colour,
            });
            if mask & 8 != 0 {
                b.outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes: 8,
                    role: OutputRole::unknown("forward alpha/bloom lane not resolved"),
                });
            }
        } else {
            b.outputs.push(OutputBinding {
                label: v.label.clone(),
                lanes: mask,
                role: OutputRole::unknown("auxiliary forward render target not resolved"),
            });
        }
    }
    b
}

impl Pass for ResolveShaunwhiteShaders {
    fn name(&self) -> &'static str {
        "resolve-shaunwhite-shaders"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[MaterialRef::NAME, ShaderLibrary::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Shader::NAME,
            ShaderBinding::NAME,
            ProgramFacts::NAME,
            MaterialPrograms::NAME,
            ShaderPasses::NAME,
            ShaderInputs::NAME,
            Name::NAME,
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let db = w
            .resource::<ShaderLibrary>()
            .ok_or("shader library not read")?
            .0
            .clone();
        let mut uses = BTreeMap::<Entity, BTreeSet<bool>>::new();
        for (e, mesh) in w.iter::<Mesh>() {
            let reason = if w.has::<RealTree>(e) {
                Some("RealTree vertex program not resolved")
            } else if w.get::<MeshFormat>(e).is_some_and(MeshFormat::skinned) {
                Some("skinned vertex program not resolved")
            } else if w.has::<Morphs>(e) {
                Some("morph vertex program not resolved")
            } else {
                None
            };
            for (i, p) in mesh.primitives().iter().enumerate() {
                for material in super::material::drawn_with(w, e, i, p) {
                    if let Some(reason) = reason {
                        log.count(reason, 1);
                    } else {
                        uses.entry(material)
                            .or_default()
                            .insert(w.get::<MeshFormat>(e).is_some_and(|f| f.flags & 64 != 0));
                    }
                }
            }
        }
        let materials: Vec<_> = w.iter::<MaterialRef>().map(|(e, m)| (e, m.clone())).collect();
        let mut cache = BTreeMap::<(u64, Vec<u8>), Result<(Entity, ProgramFacts), String>>::new();
        let mut unknown = BTreeMap::<String, u64>::new();
        for (entity, m) in materials {
            w.insert(
                entity,
                ShaderInputs {
                    material: m.clone(),
                    bindings: ShaderBinding {
                        stages: Vec::new(),
                        state: RenderState::default(),
                    },
                    unresolved: BTreeMap::new(),
                },
            );
            let key = w
                .get::<MaterialShader>(entity)
                .ok_or("material shader fields not read")
                .and_then(|f| material_key(&m, f).map_err(|_| "material key fields unsupported"));
            let key = match key {
                Ok(k) => k,
                Err(fallback) => {
                    let why = w
                        .get::<MaterialShader>(entity)
                        .and_then(|f| material_key(&m, f).err())
                        .unwrap_or_else(|| fallback.to_owned());
                    w.insert(
                        entity,
                        MaterialPrograms(vec![ProgramFacts::unavailable(&m.name, Stage::Pixel, None, &why)]),
                    );
                    log.omit(&format!("materials unresolved: {why}"), 1);
                    continue;
                }
            };
            let Some(colors) = uses.get(&entity) else {
                w.insert(
                    entity,
                    MaterialPrograms(vec![ProgramFacts::unavailable(
                        &m.name,
                        Stage::Pixel,
                        None,
                        "no supported static draw",
                    )]),
                );
                log.omit("materials unresolved: no supported static draw", 1);
                continue;
            };
            let mut stages = Vec::new();
            let mut filled = Vec::new();
            let mut unfilled = BTreeMap::new();
            let mut facts = Vec::new();
            let mut records = Vec::new();
            for (index, r) in db.records.iter().enumerate() {
                let k = r.key.permutation;
                // Only stored generic colour passes (config2); shadow/depth passes are not colour variants.
                if r.key.kind != 4
                    || k & key.0 != key.1
                    || (k >> 20) & 15 != 2
                    || k & ((1 << 33) | (1 << 35)) != 0
                    || !colors.contains(&(k & (1 << 8) != 0))
                {
                    continue;
                }
                let Payload::Effect(effect) = &r.payload else {
                    continue;
                };
                let Some(bytes) = &effect.pixel else { continue };
                let name = format!("SDB2 record {index}, key {k:016x}");
                let loaded = cache
                    .entry((effect.pixel_key, bytes.clone()))
                    .or_insert_with(|| {
                        let p = iridia_d3d9::lift(bytes).map_err(|e| e.to_string())?;
                        let shader = w.spawn();
                        let fact = ProgramFacts {
                            name: format!("SM3 pixel {:016x}", effect.pixel_key),
                            hash: Some(iridia::source_hash(bytes)),
                            stage: Stage::Pixel,
                            shader: Some(shader),
                            instruction_count: None,
                            inputs: p.interface.inputs.clone(),
                            resources: p
                                .interface
                                .resources
                                .iter()
                                .map(|r| ProgramResourceFacts {
                                    id: r.id,
                                    name: r.provenance.name.clone(),
                                    d3d: r.provenance.d3d,
                                })
                                .collect(),
                            outputs: Vec::new(),
                            surface: Default::default(),
                            not_lifted: None,
                        };
                        w.insert(
                            shader,
                            Shader {
                                format: ShaderFormat::D3d9,
                                bytecode: bytes.clone(),
                                interface: p.interface,
                            },
                        );
                        w.insert(shader, Name(fact.name.clone()));
                        w.insert(shader, fact.clone());
                        Ok((shader, fact))
                    })
                    .clone();
                match loaded {
                    Ok((shader, mut fact)) => {
                        let interface = &w
                            .get::<Shader>(shader)
                            .ok_or("resolved shader disappeared")?
                            .interface;
                        let b = binding(&m, effect, index as u32, k, shader, interface, &mut unknown);
                        match crate::shaders::bind_stage(interface, &b, &m) {
                            Ok(known) => filled.push(known),
                            Err(why) => {
                                log.count(&format!("material binding unresolved: {why}"), 1);
                                unfilled.insert(index as u32, why);
                                filled.push(b.clone());
                            }
                        }
                        fact.name = name;
                        facts.push(fact.bound(shader, &b.outputs));
                        stages.push(b);
                        records.push(index as u32);
                    }
                    Err(why) => {
                        log.omit(&format!("record variants not lifted: {why}"), 1);
                        facts.push(ProgramFacts::unavailable(
                            name,
                            Stage::Pixel,
                            Some(iridia::source_hash(bytes)),
                            why,
                        ));
                    }
                }
            }
            if stages.is_empty() {
                if facts.is_empty() {
                    facts.push(ProgramFacts::unavailable(
                        &m.name,
                        Stage::Pixel,
                        None,
                        format!(
                            "no compatible stored colour record: mask {:016x}, value {:016x}",
                            key.0, key.1
                        ),
                    ));
                }
                log.omit(
                    "materials unresolved: no liftable compatible stored colour record",
                    1,
                );
            } else {
                log.count("materials resolved to compatible colour variants", 1);
                log.count("stored colour record variants", stages.len() as u64);
                log.count(
                    "variants with render state retained only in SDB2 metadata",
                    stages.len() as u64,
                );
                w.insert(
                    entity,
                    ShaderBinding {
                        stages,
                        state: RenderState::default(),
                    },
                );
                w.insert(entity, ShaderPasses(records));
                w.insert(
                    entity,
                    ShaderInputs {
                        material: m.clone(),
                        bindings: ShaderBinding {
                            stages: filled,
                            state: RenderState::default(),
                        },
                        unresolved: unfilled,
                    },
                );
            }
            w.insert(entity, MaterialPrograms(facts));
        }
        for (why, n) in unknown {
            log.count(&format!("unresolved binding: {why}"), n);
        }
        log.count(
            "unique pixel programs",
            cache.values().filter(|v| v.is_ok()).count() as u64,
        );
        Ok(())
    }
}
