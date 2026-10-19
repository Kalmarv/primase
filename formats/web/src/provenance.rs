//! Per-tile origins. The glTF writer copies each entity's `core.provenance` into the extras of the object made from
//! it (mesh, node, material, texture; instanced placements keep theirs in `web.instances`). Repeated per object, the
//! same archive and file paths would fill a tile's JSON, so a tile keeps one origin table in its scene's
//! `extras.primase["web.origins"]` (`{"version": 1, "value": [origin, ...]}`, first-seen order) and each
//! `core.provenance` becomes `web.provenance` (`{"version": 1, "value": {"origins": [i, ...], "parts": [i, ...],
//! "passes": [...]}}`) whose `origins` and `parts` are indices into that table: `parts[k]` is the origin of the
//! mesh's primitive `k`. A tile without provenance is left byte for byte as it was.

use std::collections::BTreeMap;

use primase_glb::Document;
use serde_json::{Map, Value, json};

use crate::{Error, Result};

const SOURCE: &str = "core.provenance";
const TABLE: &str = "web.origins";
const SHARED: &str = "web.provenance";

/// The origin table being built, and the rewrite of one provenance value onto it.
#[derive(Default)]
struct Table {
    origins: Vec<Value>,
    index: BTreeMap<String, u64>,
}

impl Table {
    fn intern(&mut self, origin: &Value) -> u64 {
        let key = origin.to_string();
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.origins.len() as u64;
        self.origins.push(origin.clone());
        self.index.insert(key, i);
        i
    }

    /// `{"version": 1, "value": {origins, parts?, passes?}}` (a `core.provenance` extra) onto the table.
    fn share(&mut self, extra: &Value) -> Result<Value> {
        let bad = |m: &str| Error::Invalid(format!("{SOURCE} extra: {m}"));
        if extra["version"] != json!(1) {
            return Err(bad("unknown version"));
        }
        let value = &extra["value"];
        let local = value["origins"].as_array().ok_or_else(|| bad("no origins"))?;
        let shared: Vec<u64> = local.iter().map(|o| self.intern(o)).collect();
        let mut out = Map::new();
        out.insert("origins".into(), json!(shared));
        if let Some(parts) = value.get("parts") {
            let parts = parts
                .as_array()
                .ok_or_else(|| bad("parts is not a list"))?
                .iter()
                .map(|p| {
                    p.as_u64()
                        .and_then(|i| shared.get(i as usize).copied())
                        .ok_or_else(|| bad("part out of range"))
                })
                .collect::<Result<Vec<u64>>>()?;
            out.insert("parts".into(), json!(parts));
        }
        if let Some(passes) = value.get("passes") {
            out.insert("passes".into(), passes.clone());
        }
        Ok(json!({"version": 1, "value": out}))
    }

    /// Rewrite the provenance in `object.extras.primase`, if any; true when there was one.
    fn rewrite(&mut self, object: &mut Value) -> Result<bool> {
        let Some(primase) = object
            .get_mut("extras")
            .and_then(|e| e.get_mut("primase"))
            .and_then(Value::as_object_mut)
        else {
            return Ok(false);
        };
        let Some(extra) = primase.shift_remove(SOURCE) else {
            return Ok(false);
        };
        let shared = self.share(&extra)?;
        primase.insert(SHARED.into(), shared);
        Ok(true)
    }
}

pub(super) fn apply(mut tile: Document) -> Result<Document> {
    let doc = &mut tile.json;
    let mut table = Table::default();
    let mut found = false;
    for kind in ["meshes", "nodes", "materials", "textures"] {
        let Some(objects) = doc.get_mut(kind).and_then(Value::as_array_mut) else {
            continue;
        };
        for object in objects {
            found |= table.rewrite(object)?;
            if kind == "nodes"
                && let Some(instances) = object
                    .pointer_mut("/extras/primase/web.instances/value")
                    .and_then(Value::as_array_mut)
            {
                for instance in instances {
                    found |= table.rewrite(instance)?;
                }
            }
        }
    }
    if found {
        let scene = doc
            .get_mut("scenes")
            .and_then(|s| s.get_mut(0))
            .ok_or_else(|| Error::Invalid("tile has no scene for its origin table".into()))?;
        scene["extras"]["primase"][TABLE] = json!({"version": 1, "value": table.origins});
    }
    Ok(tile)
}
