//! Names for hashed spline ids, from the game's own names (`rme_formats::names`).
use crate::components::Spline;
use primase_core::{Entity, Name, World};
use rme_formats::names::guide_name;
use std::collections::BTreeMap;

/// Give a `core.name` to every spline of `world` whose id is sdbm of a `NavGuide_` name or of one of `extra` (as
/// written or lower-cased, the game hashes either). Returns how many splines were named.
pub fn name_splines(world: &mut World, extra: &[String]) -> usize {
    let mut by_hash: BTreeMap<u32, &str> = BTreeMap::new();
    for name in extra {
        for variant in [name.clone(), name.to_ascii_lowercase()] {
            by_hash
                .entry(primase_foundations::hash::sdbm(variant.as_bytes()))
                .or_insert(name);
        }
    }
    let found: Vec<(Entity, String)> = world
        .iter::<Spline>()
        .filter_map(|(e, s)| {
            guide_name(s.id)
                .or_else(|| by_hash.get(&s.id).copied())
                .map(|n| (e, n.to_owned()))
        })
        .collect();
    let n = found.len();
    for (e, name) in found {
        world.insert(e, Name(name));
    }
    n
}
