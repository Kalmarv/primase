//! Scene-interchange layered material syntax. Core owns the blend semantics;
//! this component retains optional spelling and unknown fields for round trips.
use crate::components::SkateMaterial;
use crate::{Error, Result};
use primase_core::pbr::UvTransform;
use primase_core::{Component, Entity, Layer, LayerWeights, LayeredMaterial, Semantic, WeightMapping, World};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct Syntax {
    layers: Value,
    weights: Value,
}
impl Component for Syntax {
    const NAME: &'static str = "skate.layered_syntax";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}
fn bad(s: impl Into<String>) -> Error {
    Error::Scene(format!("layered material: {}", s.into()))
}
fn number(v: &Value) -> Result<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| bad("expected finite number"))
}
fn pair(v: &Value) -> Result<[f32; 2]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| bad("expected two numbers"))?;
    Ok([number(&a[0])?, number(&a[1])?])
}
fn uv(v: &Value) -> Result<u8> {
    v.as_u64()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(|| bad("UV set must fit u8"))
}
fn transform(v: &Value) -> Result<UvTransform> {
    let o = v
        .as_object()
        .ok_or_else(|| bad("uv_transform must be an object"))?;
    Ok(UvTransform {
        offset: o.get("offset").map(pair).transpose()?.unwrap_or([0.; 2]),
        scale: o.get("scale").map(pair).transpose()?.unwrap_or([1.; 2]),
        rotation: o.get("rotation").map(number).transpose()?.unwrap_or(0.),
    })
}
fn attribute(v: &Value) -> Result<Semantic> {
    let s = v
        .as_str()
        .ok_or_else(|| bad("weight attribute must be a string"))?;
    if let Some(i) = s.strip_prefix("COLOR_").and_then(|i| i.parse::<u8>().ok()) {
        return Ok(Semantic::Color(i));
    }
    Err(bad(format!("unsupported weight attribute {s}; expected COLOR_n")))
}
pub(crate) fn read(
    world: &mut World,
    entity: Entity,
    layers: Value,
    weights: Value,
    keys: &BTreeMap<String, Entity>,
) -> Result<()> {
    let slots = layers.as_array().ok_or_else(|| bad("layers must be an array"))?;
    let mut decoded = Vec::new();
    for value in slots {
        if value.is_null() {
            decoded.push(None);
            continue;
        }
        let o = value
            .as_object()
            .ok_or_else(|| bad("layer must be an object or null"))?;
        let key = o
            .get("material")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("layer has no material key"))?;
        let material = *keys
            .get(key)
            .ok_or_else(|| bad(format!("unknown material {key}")))?;
        let mut texcoord = o.get("uv").map(uv).transpose()?;
        let t = o.get("uv_transform").map(transform).transpose()?;
        if let Some(value) = o.get("uv_transform").and_then(|t| t.get("texCoord")) {
            texcoord = Some(uv(value)?);
        }
        decoded.push(Some(Layer {
            material,
            texcoord,
            transform: t,
        }));
    }
    let attribute = attribute(&weights["attribute"])?;
    let mapping = match weights["mapping"].as_str() {
        Some("identity") => WeightMapping::Identity,
        Some("rgb_product_4") => WeightMapping::RgbProduct4,
        _ => return Err(bad("unknown weight mapping")),
    };
    let component = LayeredMaterial {
        layers: decoded,
        weights: LayerWeights::new(attribute, mapping),
    };
    component.verify(entity, world).map_err(bad)?;
    world.insert(entity, component);
    world.insert(entity, Syntax { layers, weights });
    Ok(())
}
pub(crate) fn write(world: &World, entity: Entity) -> Result<(Value, Value)> {
    let material = world
        .get::<LayeredMaterial>(entity)
        .ok_or_else(|| bad("class layered has no core.layered"))?;
    material.verify(entity, world).map_err(bad)?;
    let source = world.get::<Syntax>(entity);
    let mut slots = Vec::new();
    for (i, slot) in material.layers.iter().enumerate() {
        let Some(layer) = slot else {
            slots.push(Value::Null);
            continue;
        };
        let key = &world
            .get::<SkateMaterial>(layer.material)
            .ok_or_else(|| bad("layer reference is not a scene material"))?
            .key;
        let mut o = source
            .and_then(|s| s.layers.get(i))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        o.insert("material".into(), json!(key));
        // KHR texCoord overrides the layer's uv. Preserve that spelling only
        // while it still selects the core value; an edit removes the override.
        let override_uv = o
            .get("uv_transform")
            .and_then(|v| v.get("texCoord"))
            .map(uv)
            .transpose()?;
        if override_uv.is_none() || override_uv != layer.texcoord {
            if let Some(t) = o.get_mut("uv_transform").and_then(Value::as_object_mut) {
                t.remove("texCoord");
            }
            if let Some(uv) = layer.texcoord {
                o.insert("uv".into(), json!(uv));
            } else {
                o.remove("uv");
            }
        }
        match layer.transform {
            None => {
                o.remove("uv_transform");
            }
            Some(t) => {
                if o.get("uv_transform").map(transform).transpose()? != Some(t) {
                    o.insert(
                        "uv_transform".into(),
                        json!({"offset":t.offset,"scale":t.scale,"rotation":t.rotation}),
                    );
                }
            }
        }
        slots.push(Value::Object(o));
    }
    let mut weights = source
        .and_then(|s| s.weights.as_object())
        .cloned()
        .unwrap_or_default();
    let Semantic::Color(i) = material.weights.attribute else {
        return Err(bad("weight attribute must be COLOR_n"));
    };
    weights.insert("attribute".into(), json!(format!("COLOR_{i}")));
    weights.insert(
        "mapping".into(),
        json!(match material.weights.mapping {
            WeightMapping::Identity => "identity",
            WeightMapping::RgbProduct4 => "rgb_product_4",
        }),
    );
    Ok((Value::Array(slots), Value::Object(weights)))
}
