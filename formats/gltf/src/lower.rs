//! `lower-gltf-materials`: every `gltf.material` the reader kept as its glTF JSON ([`crate::read`]) becomes a
//! `pbr.material` on the same entity, per glTF 2.0 §3.9 (the model `pbr.material` is, so the lowering is direct):
//! base colour and its texture, metallic and roughness and the metallic-roughness texture, the normal texture and its
//! scale, the occlusion texture and its strength, emissive and its texture, alpha mode and cutoff, double-sidedness,
//! each slot's `texCoord`, sampler and `KHR_texture_transform` (offset, rotation, scale, `texCoord`), and the two
//! extensions `pbr.material` states exactly: `KHR_materials_emissive_strength` and `KHR_materials_unlit`. Every default
//! is the specification's. The pass takes no option: the document states everything.
//!
//! What it leaves out it names, counted ([`PassLog::omit`]): any other material extension (`KHR_materials_clearcoat`,
//! `KHR_materials_transmission`, ...) and any other extension of a texture info. A slot whose texture the reader did
//! not read (its image was refused: [`crate::textures`]) keeps its factor without the texture, counted as an
//! approximation. A malformed material (a factor that is not a number or outside its range, an unknown alpha mode) is
//! an error naming the material.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef, UvTransform};
use primase_core::{Component, Entity, OpaqueData, World};
use serde_json::{Map, Value};

use crate::read::MATERIAL;
use crate::textures::{GltfMaterialTextures, GltfSlot};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGltfMaterials;

impl Pass for LowerGltfMaterials {
    fn name(&self) -> &'static str {
        "lower-gltf-materials"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    // a world without glTF materials has nothing to lower: no requirement
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, Value)> = w
            .opaque_components()
            .filter(|(name, _)| *name == MATERIAL)
            .flat_map(|(_, c)| c.iter())
            .filter_map(|(e, o)| match &o.data {
                OpaqueData::Json(v) => Some((*e, v.clone())),
                OpaqueData::Bytes(_) => None,
            })
            .collect();
        for (e, json) in materials {
            let textures = w.get::<GltfMaterialTextures>(e).cloned();
            let name = w
                .get::<primase_core::Name>(e)
                .map_or_else(|| format!("{e:?}"), |n| n.0.clone());
            let m = lower(&json, textures.as_ref(), log)
                .map_err(|detail| format!("material {name}: {detail}"))?;
            w.insert(e, m);
            log.count("materials lowered", 1);
        }
        Ok(())
    }
}

/// The key of `o` that is not one of `known`, as an error.
fn only(o: &Map<String, Value>, known: &[&str], what: &str) -> Result<(), String> {
    match o.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(format!("{what}: unknown key {k}")),
        None => Ok(()),
    }
}

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object().ok_or_else(|| format!("{what} is not an object"))
}

/// A number of `o`, or `default` when it has none; within `range`.
fn number(
    o: &Map<String, Value>,
    key: &str,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Result<f32, String> {
    let Some(v) = o.get(key) else { return Ok(default) };
    let n = v.as_f64().ok_or_else(|| format!("{key} is not a number"))? as f32;
    if range.contains(&n) {
        Ok(n)
    } else {
        Err(format!(
            "{key} {n} is outside {}..={}",
            range.start(),
            range.end()
        ))
    }
}

/// `n` numbers of `o[key]`, or `default`, each within 0..=1 when `unit`.
fn numbers<const N: usize>(
    o: &Map<String, Value>,
    key: &str,
    default: [f32; N],
    range: std::ops::RangeInclusive<f32>,
) -> Result<[f32; N], String> {
    let Some(v) = o.get(key) else { return Ok(default) };
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| format!("{key} is not {N} numbers"))?;
    let mut out = [0.0; N];
    for (slot, x) in out.iter_mut().zip(a) {
        let n = x.as_f64().ok_or_else(|| format!("{key} holds a non-number"))? as f32;
        if !range.contains(&n) {
            return Err(format!(
                "{key} {n} is outside {}..={}",
                range.start(),
                range.end()
            ));
        }
        *slot = n;
    }
    Ok(out)
}

fn lower(
    material: &Value,
    textures: Option<&GltfMaterialTextures>,
    log: &mut PassLog,
) -> Result<PbrMaterial, String> {
    let m = object(material, "the material")?;
    only(
        m,
        &[
            "name",
            "extras",
            "extensions",
            "pbrMetallicRoughness",
            "normalTexture",
            "occlusionTexture",
            "emissiveTexture",
            "emissiveFactor",
            "alphaMode",
            "alphaCutoff",
            "doubleSided",
        ],
        "the material",
    )?;
    let unit = 0.0..=1.0;
    let mut out = PbrMaterial::default();
    let empty = Map::new();
    let pbr = match m.get("pbrMetallicRoughness") {
        Some(p) => object(p, "pbrMetallicRoughness")?,
        None => &empty,
    };
    only(
        pbr,
        &[
            "baseColorFactor",
            "baseColorTexture",
            "metallicFactor",
            "roughnessFactor",
            "metallicRoughnessTexture",
            "extensions",
            "extras",
        ],
        "pbrMetallicRoughness",
    )?;
    out.base_color = numbers(pbr, "baseColorFactor", [1.0; 4], unit.clone())?;
    out.metallic = number(pbr, "metallicFactor", 1.0, unit.clone())?;
    out.roughness = number(pbr, "roughnessFactor", 1.0, unit.clone())?;
    out.emissive = numbers(m, "emissiveFactor", [0.0; 3], unit)?;
    out.double_sided = match m.get("doubleSided") {
        None => false,
        Some(v) => v.as_bool().ok_or("doubleSided is not a boolean")?,
    };
    let cutoff = number(m, "alphaCutoff", 0.5, 0.0..=f32::MAX)?;
    out.alpha_mode = match m.get("alphaMode").map(|v| v.as_str()) {
        None | Some(Some("OPAQUE")) => AlphaMode::Opaque,
        Some(Some("MASK")) => AlphaMode::Mask { cutoff },
        Some(Some("BLEND")) => AlphaMode::Blend,
        Some(other) => return Err(format!("alphaMode {other:?} is not OPAQUE, MASK or BLEND")),
    };

    // texture slots: the info's own numbers, the texture the reader read, the transform
    for slot in GltfSlot::ALL {
        let Some(info) = slot.info(material) else { continue };
        let info = object(info, "a texture info")?;
        only(
            info,
            &["index", "texCoord", "scale", "strength", "extensions", "extras"],
            "a texture info",
        )?;
        match slot {
            GltfSlot::Normal => out.normal_scale = number(info, "scale", 1.0, f32::MIN..=f32::MAX)?,
            GltfSlot::Occlusion => out.occlusion_strength = number(info, "strength", 1.0, 0.0..=1.0)?,
            _ => {}
        }
        let (transform, texcoord) = texture_transform(info, log)?;
        let Some(read) = textures.and_then(|t| t.0.get(&slot)) else {
            log.approximate("texture slots left untextured: their image was not read", 1);
            continue;
        };
        let reference = TextureRef {
            texture: read.texture,
            texcoord,
            wrap: read.wrap,
            transform,
            filters: read.filters,
        };
        match slot {
            GltfSlot::BaseColor => out.base_color_texture = Some(reference),
            GltfSlot::MetallicRoughness => out.metallic_roughness_texture = Some(reference),
            GltfSlot::Normal => out.normal_texture = Some(reference),
            GltfSlot::Occlusion => out.occlusion_texture = Some(reference),
            GltfSlot::Emissive => out.emissive_texture = Some(reference),
        }
    }

    // extensions: the two `pbr.material` states exactly; the rest named
    if let Some(extensions) = m.get("extensions") {
        for (name, value) in object(extensions, "extensions")? {
            match name.as_str() {
                "KHR_materials_unlit" => out.shading = Shading::Unlit,
                "KHR_materials_emissive_strength" => {
                    let e = object(value, name)?;
                    only(e, &["emissiveStrength", "extras"], name)?;
                    out.emissive_strength = number(e, "emissiveStrength", 1.0, 0.0..=f32::MAX)?;
                }
                other => log.omit(&format!("material extension {other}, not lowered"), 1),
            }
        }
    }
    Ok(out)
}

/// A texture info's `KHR_texture_transform` as core's transform (`None` when absent or the identity) and the texture
/// coordinate set the slot samples (the extension's `texCoord` overrides the info's). Any other extension of the info
/// is left out, named.
fn texture_transform(
    info: &Map<String, Value>,
    log: &mut PassLog,
) -> Result<(Option<UvTransform>, u8), String> {
    let set = |o: &Map<String, Value>, key: &str| -> Result<Option<u8>, String> {
        match o.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("{key} is not a texture coordinate set")),
        }
    };
    let mut texcoord = set(info, "texCoord")?.unwrap_or(0);
    let mut transform = None;
    if let Some(extensions) = info.get("extensions") {
        for (name, value) in object(extensions, "a texture info's extensions")? {
            if name != "KHR_texture_transform" {
                log.omit(&format!("texture info extension {name}, not lowered"), 1);
                continue;
            }
            let t = object(value, name)?;
            only(t, &["offset", "rotation", "scale", "texCoord", "extras"], name)?;
            let big = f32::MIN..=f32::MAX;
            let x = UvTransform {
                offset: numbers(t, "offset", [0.0; 2], big.clone())?,
                scale: numbers(t, "scale", [1.0; 2], big.clone())?,
                rotation: number(t, "rotation", 0.0, big)?,
            };
            if x != UvTransform::IDENTITY {
                transform = Some(x);
            }
            if let Some(n) = set(t, "texCoord")? {
                texcoord = n;
            }
        }
    }
    Ok((transform, texcoord))
}

/// Register this module's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register("lower-gltf-materials", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(LowerGltfMaterials)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pbr::{Filter, Filters, Wrap};
    use primase_core::{Opaque, World};
    use serde_json::json;

    use crate::textures::SlotTexture;

    /// A world with one material entity holding `json`, and a texture entity per named slot.
    fn world(json: Value, slots: &[GltfSlot]) -> (World, Entity, Vec<Entity>) {
        let mut w = World::new();
        let e = w.spawn();
        w.insert_opaque(
            e,
            MATERIAL,
            Opaque {
                version: 1,
                data: OpaqueData::Json(json),
            },
        )
        .unwrap();
        let mut made = BTreeMapOf::new();
        let mut textures = Vec::new();
        for slot in slots {
            let t = w.spawn();
            textures.push(t);
            made.insert(
                *slot,
                SlotTexture {
                    texture: t,
                    filters: Filters {
                        mag: Some(Filter::Nearest),
                        min: None,
                        mip: None,
                    },
                    wrap: [Wrap::Clamp, Wrap::Mirror],
                },
            );
        }
        if !slots.is_empty() {
            w.insert(e, GltfMaterialTextures(made));
        }
        (w, e, textures)
    }
    type BTreeMapOf = std::collections::BTreeMap<GltfSlot, SlotTexture>;

    fn lowered(json: Value, slots: &[GltfSlot]) -> (Result<PbrMaterial, String>, PassLog, Vec<Entity>) {
        let (mut w, e, textures) = world(json, slots);
        let mut log = PassLog::default();
        let r = LowerGltfMaterials
            .run(&mut w, &mut log)
            .map(|()| w.get::<PbrMaterial>(e).cloned().expect("lowered"));
        (r, log, textures)
    }

    /// A material that states nothing is glTF's default material (§3.9.1's defaults), single-sided and opaque.
    #[test]
    fn an_empty_material_is_the_default_material() {
        let (m, log, _) = lowered(json!({}), &[]);
        assert_eq!(m.unwrap(), PbrMaterial::default());
        assert_eq!(log.counts.get("materials lowered"), Some(&1));
        assert!(log.omitted.is_empty() && log.approximations.is_empty());
    }

    /// Every core field a material states, with the slots' textures, sampler, scale, strength and transform.
    #[test]
    fn every_stated_value_is_read() {
        let info = |index| json!({"index": index});
        let material = json!({
            "name": "all",
            "pbrMetallicRoughness": {
                "baseColorFactor": [0.25, 0.5, 0.75, 0.5],
                "baseColorTexture": {"index": 0, "texCoord": 1},
                "metallicFactor": 0.125,
                "roughnessFactor": 0.375,
                "metallicRoughnessTexture": info(1),
            },
            "normalTexture": {"index": 2, "scale": 2.0},
            "occlusionTexture": {"index": 1, "strength": 0.5},
            "emissiveTexture": {
                "index": 3,
                "extensions": {"KHR_texture_transform": {"offset": [0.5, 0.25], "rotation": 1.0, "scale": [2.0, 4.0], "texCoord": 2}}
            },
            "emissiveFactor": [1.0, 0.5, 0.0],
            "alphaMode": "MASK",
            "alphaCutoff": 0.25,
            "doubleSided": true,
            "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 3.0}}
        });
        let slots = [
            GltfSlot::BaseColor,
            GltfSlot::MetallicRoughness,
            GltfSlot::Normal,
            GltfSlot::Occlusion,
            GltfSlot::Emissive,
        ];
        let (m, log, t) = lowered(material, &slots);
        let m = m.unwrap();
        assert_eq!(m.base_color, [0.25, 0.5, 0.75, 0.5]);
        assert_eq!((m.metallic, m.roughness), (0.125, 0.375));
        assert_eq!((m.normal_scale, m.occlusion_strength), (2.0, 0.5));
        assert_eq!((m.emissive, m.emissive_strength), ([1.0, 0.5, 0.0], 3.0));
        assert_eq!(m.alpha_mode, AlphaMode::Mask { cutoff: 0.25 });
        assert!(m.double_sided && m.shading == Shading::Lit && m.specular.is_none());
        let r = |x: Option<TextureRef>| x.unwrap();
        let base = r(m.base_color_texture);
        assert_eq!((base.texture, base.texcoord, base.transform), (t[0], 1, None));
        assert_eq!(base.wrap, [Wrap::Clamp, Wrap::Mirror]);
        assert_eq!(base.filters.mag, Some(Filter::Nearest));
        assert_eq!(r(m.metallic_roughness_texture).texture, t[1]);
        assert_eq!(r(m.normal_texture).texture, t[2]);
        assert_eq!(r(m.occlusion_texture).texture, t[3]);
        let emissive = r(m.emissive_texture);
        assert_eq!(
            emissive.texcoord, 2,
            "the extension's texCoord overrides the info's"
        );
        assert_eq!(
            emissive.transform,
            Some(UvTransform {
                offset: [0.5, 0.25],
                scale: [2.0, 4.0],
                rotation: 1.0
            })
        );
        assert!(log.omitted.is_empty() && log.approximations.is_empty());
    }

    /// `BLEND` and `OPAQUE`; `MASK` without a cutoff is 0.5; unlit; the extensions `pbr.material` has no field for are
    /// named and counted, not lowered.
    #[test]
    fn alpha_modes_unlit_and_unrepresented_extensions() {
        let (m, ..) = lowered(json!({"alphaMode": "BLEND"}), &[]);
        assert_eq!(m.unwrap().alpha_mode, AlphaMode::Blend);
        let (m, ..) = lowered(json!({"alphaMode": "MASK"}), &[]);
        assert_eq!(m.unwrap().alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        let (m, ..) = lowered(json!({"alphaMode": "OPAQUE", "alphaCutoff": 0.9}), &[]);
        assert_eq!(m.unwrap().alpha_mode, AlphaMode::Opaque);
        let (m, log, _) = lowered(
            json!({"extensions": {"KHR_materials_unlit": {}, "KHR_materials_clearcoat": {"clearcoatFactor": 1.0}}}),
            &[],
        );
        assert_eq!(m.unwrap().shading, Shading::Unlit);
        assert_eq!(
            log.omitted
                .get("material extension KHR_materials_clearcoat, not lowered"),
            Some(&1)
        );
    }

    /// A slot whose image was not read keeps its factor and is counted; an extension of a texture info other than
    /// the transform is named; an identity transform is none.
    #[test]
    fn unread_images_and_texture_extensions_are_counted() {
        let (m, log, _) = lowered(
            json!({
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1.0, 0.0, 0.0, 1.0],
                    "baseColorTexture": {"index": 0}
                },
                "normalTexture": {"index": 1, "extensions": {"KHR_texture_transform": {}, "EXT_other": {}}}
            }),
            &[GltfSlot::Normal],
        );
        let m = m.unwrap();
        assert_eq!(m.base_color, [1.0, 0.0, 0.0, 1.0]);
        assert!(m.base_color_texture.is_none());
        assert_eq!(m.normal_texture.unwrap().transform, None);
        assert_eq!(
            log.approximations
                .get("texture slots left untextured: their image was not read"),
            Some(&1)
        );
        assert_eq!(
            log.omitted.get("texture info extension EXT_other, not lowered"),
            Some(&1)
        );
    }

    /// Malformed values are errors naming the material, never defaults.
    #[test]
    fn malformed_materials_are_errors() {
        for bad in [
            json!({"alphaMode": "CUT"}),
            json!({"pbrMetallicRoughness": {"metallicFactor": 1.5}}),
            json!({"pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0]}}),
            json!({"emissiveFactor": [0.0, "x", 0.0]}),
            json!({"doubleSided": 1}),
            json!({"occlusionTexture": {"index": 0, "strength": 2.0}}),
            json!({"unknownKey": 1}),
        ] {
            let (r, ..) = lowered(bad.clone(), &[]);
            let e = r.expect_err(&bad.to_string());
            assert!(e.contains("material "), "{e}");
        }
    }

    /// The pass is registered, takes no option and names `pbr.material` as what it provides.
    #[test]
    fn the_pass_is_registered_by_name() {
        let mut reg = primase_core::pass::PassRegistry::new();
        register(&mut reg);
        let pass = reg.make_registered("lower-gltf-materials").unwrap();
        assert_eq!(pass.provides(), [PbrMaterial::NAME]);
        let mut o = Options::new();
        o.insert("x".into(), "1".into());
        assert!(reg.make("lower-gltf-materials", &o).is_err());
    }
}
