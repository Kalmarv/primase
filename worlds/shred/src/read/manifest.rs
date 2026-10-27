//! Joins of `rme_formats::manifest`'s database rows to the entities of a read level.
use rme_formats::manifest::DropSpot;

/// The spot chunks of `world` (a level's `read_level`) that a drop spot names: those whose name hash is sdbm of the
/// row's `SPOT_NAME`, as written or lower-cased (the game hashes either). Rows with no `SPOT_NAME` (`-1`, `na`) or no
/// matching chunk have an empty list. The chunks are all classes; the caller checks `shred.spot` class.
pub fn resolve_drop_spots<'a>(
    world: &primase_core::World,
    rows: &'a [DropSpot],
) -> Vec<(&'a DropSpot, Vec<primase_core::Entity>)> {
    let mut by_name: std::collections::BTreeMap<u32, Vec<primase_core::Entity>> =
        std::collections::BTreeMap::new();
    for (e, s) in world.iter::<crate::components::Spot>() {
        by_name.entry(s.name).or_default().push(e);
    }
    rows.iter()
        .map(|r| {
            let mut hits = Vec::new();
            for variant in [r.spot_name.clone(), r.spot_name.to_ascii_lowercase()] {
                for e in by_name
                    .get(&primase_foundations::hash::sdbm(variant.as_bytes()))
                    .into_iter()
                    .flatten()
                {
                    if !hits.contains(e) {
                        hits.push(*e);
                    }
                }
            }
            (r, hits)
        })
        .collect()
}
