//! `lower-trackmania-surfaces`: the class of every collision surface named by the game's physics surface ids
//! (`CPlugSurface` materials, named by `gbx_formats::plug::surface::material_id_name`), from
//! [`TRACKMANIA_SURFACE_CLASSES`], a table of those names.
//!
//! The table is ours, read off the names and what the stadium draws with them: the road is asphalt, the platforms and
//! pavements concrete, the barriers, pipes and structures metal (`ResonantMetal` and `MetalTrans` too), the road
//! borders and inflatables rubber (`SlidingRubber`, and the bumper pads, too), rock and stone stone, the dirt roads
//! dirt; the wet variants are their dry surface's class. The turbo, free-wheeling and roulette pads lie on the road and
//! are asphalt. Water is water. The names with no surface a car stands on as a material (`NotCollidable`, which the
//! game does not collide with, and `Player`, `PlayerOnly`, `Bullet`, `Trunk`, `OffZone`, `Tech*`, `WallJump`, `Danger`,
//! the golf surfaces, `Test`) are left unclassified and counted by name, as are names the table does not know.
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{SurfaceClass, Surfaces, World};

use SurfaceClass::*;

/// TrackMania's physics surface names and their classes (see the module documentation).
pub const TRACKMANIA_SURFACE_CLASSES: &[(&str, SurfaceClass)] = &[
    ("Concrete", Concrete),
    ("Pavement", Concrete),
    ("WetPavement", Concrete),
    ("Asphalt", Asphalt),
    ("WetAsphalt", Asphalt),
    ("Turbo_Deprecated", Asphalt),
    ("Turbo2_Deprecated", Asphalt),
    ("TurboRoulette_Deprecated", Asphalt),
    ("FreeWheeling_Deprecated", Asphalt),
    ("Grass", Grass),
    ("WetGrass", Grass),
    ("Ice", Ice),
    ("Snow", Snow),
    ("Metal", Metal),
    ("ResonantMetal", Metal),
    ("MetalTrans", Metal),
    ("Sand", Sand),
    ("Dirt", Dirt),
    ("DirtRoad", Dirt),
    ("WetDirtRoad", Dirt),
    ("Rubber", Rubber),
    ("SlidingRubber", Rubber),
    ("Bumper_Deprecated", Rubber),
    ("Rock", Stone),
    ("Stone", Stone),
    ("Wood", Wood),
    ("SlidingWood", Wood),
    ("Water", Water),
];

/// A surface name's class from [`TRACKMANIA_SURFACE_CLASSES`].
pub fn surface_class(name: &str) -> Option<SurfaceClass> {
    TRACKMANIA_SURFACE_CLASSES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, c)| c)
}

/// `lower-trackmania-surfaces` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerTrackmaniaSurfaces;

/// `lower-trackmania-surfaces`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by
/// the table.
pub const LOWER_TRACKMANIA_SURFACES: PassSpec = PassSpec {
    name: "lower-trackmania-surfaces",
    options: &[],
    make: |_| Ok(Box::new(LowerTrackmaniaSurfaces)),
};

impl Pass for LowerTrackmaniaSurfaces {
    fn name(&self) -> &'static str {
        "lower-trackmania-surfaces"
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    /// It classifies the surfaces in place (a target's "every surface classified" rule plans it).
    fn legalizes(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    /// A pass after it that provides surfaces would add unclassified ones.
    fn settles(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(surfaces) = w.resource_mut::<Surfaces>() else {
            return Ok(());
        };
        let mut unmapped: BTreeMap<String, u64> = BTreeMap::new();
        for s in &mut surfaces.0 {
            match surface_class(&s.name) {
                Some(c) => {
                    s.class = Some(c);
                    log.count(&format!("surfaces classified {}", c.as_str()), 1);
                }
                None => *unmapped.entry(s.name.clone()).or_default() += 1,
            }
        }
        for (name, n) in unmapped {
            log.count(&format!("surfaces left unclassified: {name}"), n);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gbx_formats::plug::surface::material_id_name;

    /// Every name in the table is one of the game's physics surface ids, once.
    #[test]
    fn the_table_names_game_surfaces_once() {
        let ids: Vec<&str> = (0..i16::MAX).map_while(material_id_name).collect();
        let mut seen = std::collections::BTreeSet::new();
        for (name, _) in TRACKMANIA_SURFACE_CLASSES {
            assert!(ids.contains(name), "{name} is not a surface id");
            assert!(seen.insert(*name), "{name} twice");
        }
        for left in ["NotCollidable", "Player", "Danger"] {
            assert!(ids.contains(&left) && surface_class(left).is_none(), "{left}");
        }
    }
}
