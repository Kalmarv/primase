//! `lower-gta-surfaces`: the class of every collision surface named by GTA's material table (`materials.dat`), from
//! [`GTA_SURFACE_CLASSES`], a table of its material names.
//!
//! The table is ours, read off the names and what they are in the game: tarmac is asphalt; paving slabs, breeze
//! blocks and pavement concrete are concrete; rock, cobbles, marble and sandstone are stone; ceramic and roof tiles are
//! tile; sand, gravel, mud (marsh too), snow and ice by name; soil, clay, dirt tracks and wood chips are dirt; grass,
//! hay, bushes, leaves and twigs are grass; cloth, carpets, leather, tarpaulin and soft tops are fabric; linoleum,
//! laminate, fibreglass and polystyrene are plastic; perspex, screens and car glass are glass; water is water; the
//! effect materials on metal (electrified, steam, flame, the water tower) and electric fences and barbed wire metal,
//! the beer barrel wood, the pool table's felt and cushions fabric. Puddles (solid ground under a wet look), the
//! no-friction walls (solid, slippery: they keep a body off ledges), plaster, cardboard, paper, foam and the default
//! material are classified other. The names with no surface a body stands on (body parts, the other physics-only
//! materials: balls, casters, the ped capsule and car void, dynamic cover bounds; liquids other than water, meat, the
//! TEMP slots) are left unclassified and counted by name, as are names the table does not know.
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::{SurfaceClass, Surfaces, World};

use SurfaceClass::*;

/// GTA's collision material names and their classes (see the module documentation).
pub const GTA_SURFACE_CLASSES: &[(&str, SurfaceClass)] = &[
    ("DEFAULT", Other),
    ("CONCRETE", Concrete),
    ("CONCRETE_POTHOLE", Concrete),
    ("CONCRETE_DUSTY", Concrete),
    ("CONCRETE_PAVEMENT", Concrete),
    ("PAVING_SLAB", Concrete),
    ("BREEZE_BLOCK", Concrete),
    ("TARMAC", Asphalt),
    ("TARMAC_PAINTED", Asphalt),
    ("TARMAC_POTHOLE", Asphalt),
    ("RUMBLE_STRIP", Asphalt),
    ("ROCK", Stone),
    ("ROCK_MOSSY", Stone),
    ("ROCK_NOINST", Stone),
    ("STONE", Stone),
    ("COBBLESTONE", Stone),
    ("MARBLE", Stone),
    ("SANDSTONE_SOLID", Stone),
    ("SANDSTONE_BRITTLE", Stone),
    ("BRICK", Brick),
    ("BRICK_PAVEMENT", Brick),
    ("CERAMIC", Tile),
    ("ROOF_TILE", Tile),
    ("SAND_LOOSE", Sand),
    ("SAND_COMPACT", Sand),
    ("SAND_WET", Sand),
    ("SAND_TRACK", Sand),
    ("SAND_UNDERWATER", Sand),
    ("SAND_DRY_DEEP", Sand),
    ("SAND_WET_DEEP", Sand),
    ("ICE", Ice),
    ("ICE_TARMAC", Ice),
    ("SNOW_LOOSE", Snow),
    ("SNOW_COMPACT", Snow),
    ("SNOW_DEEP", Snow),
    ("SNOW_TARMAC", Snow),
    ("GRAVEL_SMALL", Gravel),
    ("GRAVEL_LARGE", Gravel),
    ("GRAVEL_DEEP", Gravel),
    ("GRAVEL_TRAIN_TRACK", Gravel),
    ("DIRT_TRACK", Dirt),
    ("SOIL", Dirt),
    ("CLAY_HARD", Dirt),
    ("CLAY_SOFT", Dirt),
    ("WOODCHIPS", Dirt),
    ("MUD_HARD", Mud),
    ("MUD_POTHOLE", Mud),
    ("MUD_SOFT", Mud),
    ("MUD_UNDERWATER", Mud),
    ("MUD_DEEP", Mud),
    ("MARSH", Mud),
    ("MARSH_DEEP", Mud),
    ("GRASS_LONG", Grass),
    ("GRASS", Grass),
    ("GRASS_SHORT", Grass),
    ("HAY", Grass),
    ("BUSHES", Grass),
    ("BUSHES_NOINST", Grass),
    ("TWIGS", Grass),
    ("LEAVES", Grass),
    ("TREE_BARK", Wood),
    ("METAL_SOLID_SMALL", Metal),
    ("METAL_SOLID_MEDIUM", Metal),
    ("METAL_SOLID_LARGE", Metal),
    ("METAL_HOLLOW_SMALL", Metal),
    ("METAL_HOLLOW_MEDIUM", Metal),
    ("METAL_HOLLOW_LARGE", Metal),
    ("METAL_CHAINLINK_SMALL", Metal),
    ("METAL_CHAINLINK_LARGE", Metal),
    ("METAL_CORRUGATED_IRON", Metal),
    ("METAL_GRILLE", Metal),
    ("METAL_RAILING", Metal),
    ("METAL_DUCT", Metal),
    ("METAL_GARAGE_DOOR", Metal),
    ("METAL_MANHOLE", Metal),
    ("METAL_SOLID_ROAD_SURFACE", Metal),
    ("CAR_METAL", Metal),
    ("CAR_ENGINE", Metal),
    ("STUNT_RAMP_SURFACE", Metal),
    ("VFX_METAL_ELECTRIFIED", Metal),
    ("VFX_METAL_WATER_TOWER", Metal),
    ("VFX_METAL_STEAM", Metal),
    ("VFX_METAL_FLAME", Metal),
    ("PHYS_ELECTRIC_FENCE", Metal),
    ("PHYS_ELECTRIC_METAL", Metal),
    ("PHYS_BARBED_WIRE", Metal),
    ("WOOD_SOLID_SMALL", Wood),
    ("WOOD_SOLID_MEDIUM", Wood),
    ("WOOD_SOLID_LARGE", Wood),
    ("WOOD_SOLID_POLISHED", Wood),
    ("WOOD_FLOOR_DUSTY", Wood),
    ("WOOD_HOLLOW_SMALL", Wood),
    ("WOOD_HOLLOW_MEDIUM", Wood),
    ("WOOD_HOLLOW_LARGE", Wood),
    ("WOOD_CHIPBOARD", Wood),
    ("WOOD_OLD_CREAKY", Wood),
    ("WOOD_HIGH_DENSITY", Wood),
    ("WOOD_LATTICE", Wood),
    ("WOOD_HIGH_FRICTION", Wood),
    ("VFX_WOOD_BEER_BARREL", Wood),
    ("ROOF_FELT", Fabric),
    ("TARPAULIN", Fabric),
    ("CLOTH", Fabric),
    ("CARPET_SOLID", Fabric),
    ("CARPET_SOLID_DUSTY", Fabric),
    ("CARPET_FLOORBOARD", Fabric),
    ("LEATHER", Fabric),
    ("FEATHER_PILLOW", Fabric),
    ("CAR_SOFTTOP", Fabric),
    ("CAR_SOFTTOP_CLEAR", Fabric),
    ("PHYS_POOLTABLE_SURFACE", Fabric),
    ("PHYS_POOLTABLE_CUSHION", Fabric),
    ("PLASTIC", Plastic),
    ("PLASTIC_HOLLOW", Plastic),
    ("PLASTIC_HIGH_DENSITY", Plastic),
    ("PLASTIC_CLEAR", Plastic),
    ("PLASTIC_HOLLOW_CLEAR", Plastic),
    ("PLASTIC_HIGH_DENSITY_CLEAR", Plastic),
    ("FIBREGLASS", Plastic),
    ("FIBREGLASS_HOLLOW", Plastic),
    ("LINOLEUM", Plastic),
    ("LAMINATE", Plastic),
    ("POLYSTYRENE", Plastic),
    ("CAR_PLASTIC", Plastic),
    ("EMISSIVE_PLASTIC", Plastic),
    ("SLATTED_BLINDS", Plastic),
    ("RUBBER", Rubber),
    ("RUBBER_HOLLOW", Rubber),
    ("GLASS_SHOOT_THROUGH", Glass),
    ("GLASS_BULLETPROOF", Glass),
    ("GLASS_OPAQUE", Glass),
    ("PERSPEX", Glass),
    ("TVSCREEN", Glass),
    ("EMISSIVE_GLASS", Glass),
    ("CAR_GLASS_WEAK", Glass),
    ("CAR_GLASS_MEDIUM", Glass),
    ("CAR_GLASS_STRONG", Glass),
    ("CAR_GLASS_BULLETPROOF", Glass),
    ("CAR_GLASS_OPAQUE", Glass),
    ("WATER", Water),
    ("PUDDLE", Other),
    ("PHYS_NO_FRICTION", Other),
    ("PLASTER_SOLID", Other),
    ("PLASTER_BRITTLE", Other),
    ("CARDBOARD_SHEET", Other),
    ("CARDBOARD_BOX", Other),
    ("PAPER", Other),
    ("FOAM", Other),
];

/// A material name's class from [`GTA_SURFACE_CLASSES`] (names compared without case).
pub fn surface_class(name: &str) -> Option<SurfaceClass> {
    GTA_SURFACE_CLASSES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, c)| c)
}

/// `lower-gta-surfaces` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaSurfaces;

impl Pass for LowerGtaSurfaces {
    fn name(&self) -> &'static str {
        "lower-gta-surfaces"
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
    use primase_core::Surface;
    use primase_core::pass::{Error, PassManager, PassRegistry, RunReport};

    #[test]
    fn surfaces_provided_after_they_are_classified_refuse_the_pipeline() {
        let mut r = PassRegistry::new();
        crate::register(&mut r);
        let m = PassManager::new(r);
        let run = |text: &str| {
            m.run(
                &mut World::new(),
                &m.registry.parse(text).unwrap(),
                &mut RunReport::default(),
            )
        };
        let err = run("lower-gta-surfaces, lower-gta-embedded-collision").unwrap_err();
        assert!(matches!(err, Error::SettledThenChanged { .. }), "{err}");
        // the default order (embedded collision first) is not refused for that
        let r = run("lower-gta-embedded-collision, lower-gta-surfaces");
        assert!(!matches!(r, Err(Error::SettledThenChanged { .. })), "{r:?}");
    }

    #[test]
    fn the_table_names_each_material_once() {
        let mut names: Vec<String> = GTA_SURFACE_CLASSES
            .iter()
            .map(|(n, _)| n.to_ascii_uppercase())
            .collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "a material twice in the table");
    }

    #[test]
    fn the_pass_classifies_known_names_and_leaves_the_rest() {
        let mut w = World::new();
        w.insert_resource(Surfaces(vec![
            Surface::new("TARMAC_PAINTED"),
            Surface::new("concrete_pavement"),
            Surface::new("SPINE2"),
        ]));
        let mut log = PassLog::default();
        LowerGtaSurfaces.run(&mut w, &mut log).unwrap();
        let s = &w.resource::<Surfaces>().unwrap().0;
        assert_eq!(
            s.iter().map(|s| s.class).collect::<Vec<_>>(),
            vec![Some(Asphalt), Some(Concrete), None]
        );
        assert_eq!(log.counts.get("surfaces left unclassified: SPINE2"), Some(&1));
    }
}
