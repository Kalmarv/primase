//! Elden Ring's far tiles hold environment probe groups that no box of their layout names
//! ([`FarTileEnvironmentGroup`]); `place-fromsoft-far-tile-environment` gives each a region, its box fitted to the
//! groups' volumes, since the game's own placement of them is not traced.
use crate::sekiro::environment_data::{
    EnvironmentRegion, FarTileEnvironmentGroup, FarTilePlacement, Placement,
};
use fromsoft_formats::envmap::BoxFields;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, ContentNote, World};
use std::collections::BTreeMap;

/// What a far tile's fitted boxes rest on.
pub const FAR_TILE_FIT: &str = "far tile environment groups placed by boxes fitted to their volumes and stacked in group order, not by the game's placement (untraced): the volumes' open and buried cells against the collision of the tile and its neighbours (mean AUC 0.971 over 12 Elden Ring far tiles, 8 with two layers; vertical placement known to about +-20 m)";
/// The far tile's box fields, which no layout stores.
pub const FAR_TILE_FIELDS: &str = "far tile environment group fields are stand-ins: fade distance 0, priority 0, diffuse and specular scale 1, reflection origin the box centre";

/// The far tile's width and depth: the four by four small tiles it covers, 256 m each.
const FAR_TILE_SPAN: f32 = 1024.;
/// The lowest layer's bottom and top: 84 rows of 13.25 m from -105 m, the fit's best (bottoms from -120 to -80 m with
/// rows of 12.75 to 13.625 m score within 0.001 of it, so the vertical placement is known to about +-20 m).
const FAR_TILE_BOTTOM: f32 = -105.;
const FAR_TILE_TOP: f32 = FAR_TILE_BOTTOM + 84. * 13.25;

/// The box of a far tile's environment layer `layer` (0 the lowest) in the tile's stored frame (centre, half extents in
/// metres), fitted to the volumes of Elden Ring far tiles ([`FAR_TILE_FIT`]): the volume's texel columns, rows and
/// slices run along the tile's +X, +Y and +Z; horizontally it spans the tile ([`FAR_TILE_SPAN`] about the origin), and
/// vertically the lowest layer spans [`FAR_TILE_BOTTOM`] to [`FAR_TILE_TOP`], one height range for every far tile, each
/// further layer the same height directly above the one before.
pub fn far_tile_box(layer: usize) -> ([f32; 3], [f32; 3]) {
    let half = FAR_TILE_SPAN / 2.;
    let vertical = (FAR_TILE_TOP - FAR_TILE_BOTTOM) / 2.;
    let bottom = FAR_TILE_BOTTOM + layer as f32 * 2. * vertical;
    ([0., bottom + vertical, 0.], [half, vertical, half])
}

#[derive(Clone, Copy, Debug)]
pub struct PlaceFromSoftFarTileEnvironment;
pub const PLACE_FROMSOFT_FAR_TILE_ENVIRONMENT: PassSpec = PassSpec {
    name: "place-fromsoft-far-tile-environment",
    options: &[],
    make: |_| Ok(Box::new(PlaceFromSoftFarTileEnvironment)),
};
impl Pass for PlaceFromSoftFarTileEnvironment {
    fn name(&self) -> &'static str {
        "place-fromsoft-far-tile-environment"
    }
    /// A map other than a far tile has no far-tile groups to place.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[FarTileEnvironmentGroup::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[EnvironmentRegion::NAME, ContentNote::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[EnvironmentRegion::NAME, ContentNote::NAME]
    }
    fn approximates(&self) -> &'static str {
        "far tile environment boxes fitted to the groups' volumes and stacked in group order, their fields stand-ins; the game's placement is not traced"
    }
    /// Each far tile's groups, stacked from the ground up in the order of their numbers, one fitted box each, with
    /// stand-in fields; both are approximations, counted per group.
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut by_tile = BTreeMap::<String, Vec<_>>::new();
        for (entity, g) in world.iter::<FarTileEnvironmentGroup>() {
            by_tile
                .entry(g.area.clone())
                .or_default()
                .push((g.group, entity, g.clone()));
        }
        let mut placed = 0;
        for groups in by_tile.values_mut() {
            groups.sort_by_key(|(group, ..)| *group);
            for (layer, (_, entity, g)) in groups.iter().enumerate() {
                let (centre, half_extents) = far_tile_box(layer);
                world.insert(
                    *entity,
                    EnvironmentRegion {
                        area: g.area.clone(),
                        placement: Placement::FarTile {
                            layer: u8::try_from(layer)
                                .map_err(|_| format!("{}: too many far tile layers", g.area))?,
                            centre,
                            half_extents,
                            how: FarTilePlacement::Fitted,
                        },
                        fields: BoxFields {
                            fade_distance: 0.,
                            priority: 0,
                            group_override: -1,
                            specular_scale: 1.,
                            diffuse_scale: 1.,
                        },
                        group: g.group,
                        available_packages: g.available_packages.clone(),
                        reflection_origin: crate::pose::FRAME.point(centre),
                    },
                );
                placed += 1;
            }
        }
        log.count("far tile environment groups placed", placed);
        if placed > 0 {
            for (key, notice) in [
                ("environment-far-tile-placement", FAR_TILE_FIT),
                ("environment-far-tile-fields", FAR_TILE_FIELDS),
            ] {
                log.approximate(notice, placed);
                super::environment_probes::note(world, key, notice.into(), false);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};

    /// Groups of two tiles, given out of order: each tile's are stacked in group order from layer 0, each layer's box
    /// directly on the one below, with stand-in fields, and both approximations reported once per group.
    #[test]
    fn far_tile_groups_are_stacked_in_group_order_and_reported() {
        let mut registry = PassRegistry::new();
        super::super::register(&mut registry);
        let manager = PassManager::new(registry).with_pass_checks(true);
        let mut world = World::new();
        for (area, group) in [
            ("m60_12_13_02", 948),
            ("m60_12_13_02", 932),
            ("m60_08_11_02", 901),
        ] {
            let e = world.spawn();
            world.insert(
                e,
                FarTileEnvironmentGroup {
                    area: area.into(),
                    group,
                    available_packages: vec!["p".into()],
                },
            );
        }
        let pipeline = manager
            .registry
            .parse("place-fromsoft-far-tile-environment")
            .unwrap();
        let mut report = RunReport::default();
        manager.run(&mut world, &pipeline, &mut report).unwrap();
        let mut regions: Vec<_> = world
            .iter::<EnvironmentRegion>()
            .map(|(_, r)| r.clone())
            .collect();
        regions.sort_by_key(|r| (r.area.clone(), r.group));
        let layers: Vec<_> = regions
            .iter()
            .map(|r| match r.placement {
                Placement::FarTile {
                    layer,
                    centre,
                    half_extents,
                    how,
                } => {
                    assert_eq!(how, FarTilePlacement::Fitted);
                    assert_eq!((r.fields.priority, r.fields.fade_distance), (0, 0.));
                    (
                        r.area.as_str(),
                        r.group,
                        layer,
                        centre[1] - half_extents[1],
                        centre[1] + half_extents[1],
                    )
                }
                Placement::Authored(_) => panic!("authored"),
            })
            .collect();
        assert_eq!(
            layers,
            [
                ("m60_08_11_02", 901, 0, FAR_TILE_BOTTOM, FAR_TILE_TOP),
                ("m60_12_13_02", 932, 0, FAR_TILE_BOTTOM, FAR_TILE_TOP),
                (
                    "m60_12_13_02",
                    948,
                    1,
                    FAR_TILE_TOP,
                    2. * FAR_TILE_TOP - FAR_TILE_BOTTOM
                ),
            ]
        );
        let text = format!("{report:?}");
        assert!(
            text.contains(FAR_TILE_FIT) && text.contains(FAR_TILE_FIELDS),
            "{text}"
        );
    }
}
