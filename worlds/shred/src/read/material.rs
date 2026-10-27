//! Source materials as entities.
use crate::components::ShredMaterial;
use crate::{Result, invalid};
use primase_core::{Entity, World};
use rme_formats::material::MaterialKey;
use std::collections::{BTreeMap, BTreeSet};

/// Attach one entity per (bank, id). P3's mesh ID-to-bank resolution is a later join.
/// Reject duplicate keys before mutating the world.
pub fn attach(world: &mut World, materials: Vec<ShredMaterial>) -> Result<BTreeMap<MaterialKey, Entity>> {
    let mut keys = BTreeSet::new();
    for m in &materials {
        if !keys.insert(m.key.clone()) {
            return Err(invalid(0, "duplicate (bank,id) material"));
        }
    }
    if world.iter::<ShredMaterial>().any(|(_, m)| keys.contains(&m.key)) {
        return Err(invalid(0, "material key already attached"));
    }
    let mut out = BTreeMap::new();
    for m in materials {
        let key = m.key.clone();
        let e = world.spawn();
        world.insert(e, m);
        out.insert(key, e);
    }
    Ok(out)
}
