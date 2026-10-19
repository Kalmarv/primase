//! The GTA side of every conversion: an install opened once, a region read into a world of placements (both maps'
//! states, every entity within reach), and the subsets a writer asks for loaded with their drawables and textures.
//! The reader only reads: which map state, hour and placements a conversion keeps are passes the program chooses
//! (`resolve-state`, `gta-select-region`, ...), run on the read world before its parts are planned; what a loaded
//! world becomes is passes too (`lower-gta-pbr`, ...), in the program's order with its options. A writer
//! that writes a large region cell by cell loads each cell's roots ([`GtaSource::load`]); one that writes everything
//! loads every placement once.
use std::collections::BTreeSet;
use std::path::Path;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, World};

use crate::archive_set::ArchiveSet;
use crate::assets::{AssetReport, LightsReport, attach_assets_with, attach_lights};
use crate::collision::{BoundsReport, attach_map_bounds};
use crate::components::{GtaArchetype, GtaEntity};
use crate::gtxd::TextureParents;
use crate::keys::Keys;
use crate::region::{Region, RegionReport, States, archive_sets, read_region};
use crate::{Install, Result};

/// Which of GTA's maps a conversion draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Map {
    /// Story mode's map.
    Story,
    /// GTA Online's map (every pack's map change sets run, as on entering Online).
    Online,
}

impl Map {
    /// The `gta.map` axis value.
    pub fn value(self) -> &'static str {
        match self {
            Map::Story => "story",
            Map::Online => "online",
        }
    }
}

/// The hours GTA's night-only surfaces and lights are on, bit h for hour h: 00-05 and 20-23, the hours the city's
/// street lamps' light records name (`time_flags` 0xF0003F on the lamps of every region read so far).
pub const NIGHT_HOURS: u32 = 0x00F0_003F;

/// Hour `hour` (0-23) is a night hour ([`NIGHT_HOURS`]).
pub fn is_night(hour: u8) -> bool {
    hour < 24 && NIGHT_HOURS & (1 << hour) != 0
}

/// `gta-leave-out-interiors`: placements of entities placed in an interior are despawned (counted). The game draws an
/// interior only from inside it or through its portals; their prototypes stay for other placements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaveOutInteriors;

impl LeaveOutInteriors {
    pub fn from_options(o: &Options) -> std::result::Result<LeaveOutInteriors, String> {
        match o.keys().next() {
            Some(k) => Err(format!("unknown option {k}")),
            None => Ok(LeaveOutInteriors),
        }
    }
}

impl Pass for LeaveOutInteriors {
    fn name(&self) -> &'static str {
        "gta-leave-out-interiors"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.entity"]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let inside: Vec<Entity> = w
            .iter::<GtaEntity>()
            .filter(|(_, g)| g.interior.is_some())
            .map(|(e, _)| e)
            .collect();
        log.count("placements in an interior left out", inside.len() as u64);
        for e in inside {
            w.despawn(e);
        }
        Ok(())
    }
}

/// What reading a region did.
#[derive(Clone, Debug)]
pub struct ReadReport {
    pub region: RegionReport,
    pub lights: LightsReport,
    pub bounds: BoundsReport,
    /// Each water file: its path with its water, calming and wave quad counts.
    pub water: Vec<(String, [usize; 3])>,
}

/// An opened install: its key material, both maps' archive sets and the texture dictionary parents of the map assets
/// are loaded from.
pub struct GtaSource {
    keys: Keys,
    story: ArchiveSet,
    online: ArchiveSet,
    map: Map,
    parents: TextureParents,
}

impl std::fmt::Debug for GtaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaSource")
            .field("map", &self.map)
            .finish_non_exhaustive()
    }
}

impl GtaSource {
    /// Opens the install at `dir` with the key material in `keys_dir`; drawables and textures are loaded from `map`'s
    /// archive set.
    pub fn open(dir: &Path, keys_dir: &Path, map: Map) -> Result<GtaSource> {
        let keys = Keys::from_dir(keys_dir)?;
        let install = Install::open(dir, &keys)?;
        let (story, online) = archive_sets(&install, &keys, States::All)?;
        let online = online.ok_or_else(|| crate::invalid("install", 0, "no GTA Online archive set"))?;
        let parents = TextureParents::read(
            match map {
                Map::Story => &story,
                Map::Online => &online,
            },
            &keys,
        )?;
        Ok(GtaSource {
            keys,
            story,
            online,
            map,
            parents,
        })
    }

    fn assets(&self) -> &ArchiveSet {
        match self.map {
            Map::Story => &self.story,
            Map::Online => &self.online,
        }
    }

    /// Every entity of both maps within `reach` metres of `region` (`[x0, y0, x1, y1]`), with its prototype and its
    /// map-state and hour conditions ([`crate::region::read_region`]), each prototype's light records
    /// ([`crate::assets::attach_lights`]), the static map bounds meeting the region
    /// ([`crate::collision::attach_map_bounds`]) and every water file ([`crate::water::attach_water`]: each quad of
    /// the whole map placed with its file's `gta.water` condition, the rest of each file a `gta.water`); no geometry
    /// or textures yet.
    pub fn read(&self, region: [f32; 4], reach: f32) -> Result<(World, ReadReport)> {
        let bounds_region = region;
        let (mut w, region) = read_region(
            &self.story,
            Some(&self.online),
            &self.keys,
            &Region {
                min: [region[0], region[1]],
                max: [region[2], region[3]],
                reach,
            },
        )?;
        let lights = attach_lights(&mut w, self.assets(), &self.keys)?;
        let bounds = attach_map_bounds(&mut w, self.assets(), &self.keys, bounds_region)?;
        let water = crate::water::attach_water(&mut w, &self.assets().install, &self.keys)?;
        Ok((
            w,
            ReadReport {
                region,
                lights,
                bounds,
                water,
            },
        ))
    }

    /// The part of `world` that `roots` reach ([`World::closure`], as a [`World::subset`]), with the drawables and
    /// textures of its placements' prototypes attached. Entity ids stay `world`'s; what attaching spawns (LOD forms,
    /// materials, textures) is reached through the prototypes.
    pub fn load(&self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, AssetReport)> {
        // the world's remaining axes (global state a consumer resolves itself, a kept gta.hour) are in every part, so
        // a per-part pass can refer to them
        let axes = world.iter::<primase_core::state::StateAxis>().map(|(e, _)| e);
        let mut w = world.subset(&world.closure(roots.iter().copied().chain(axes)));
        let report = attach_assets_with(&mut w, self.assets(), &self.keys, &self.parents)?;
        Ok((w, report))
    }

    /// Every placement of `world` loaded at once (a writer that writes everything together).
    pub fn load_all(&self, world: &World) -> Result<(World, AssetReport)> {
        let roots = world.iter::<Instance>().map(|(e, _)| e).collect();
        self.load(world, &roots)
    }
}

/// A prototype's box before its geometry is loaded: its archetype's bounding box, in the core frame (metres, +Y up);
/// `None` for an entity that is no GTA prototype.
pub fn prototype_box(world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
    let Some(a) = world.get::<GtaArchetype>(prototype) else {
        // a water surface: its outline's box
        return world.get::<crate::water::GtaWaterSurface>(prototype)?.bounds();
    };
    let (lo, hi) = (a.bb_min.map(f64::from), a.bb_max.map(f64::from));
    // GTA (x, y, z) is core (x, z, -y)
    Some([[lo[0], lo[2], -hi[1]], [hi[0], hi[2], -lo[1]]])
}

#[cfg(test)]
mod tests {
    use super::is_night;

    /// The street lamps' hours are night: 00-05 and 20-23.
    #[test]
    fn the_lamps_hours_are_night() {
        let night: Vec<u8> = (0..24).filter(|&h| is_night(h)).collect();
        assert_eq!(night, vec![0, 1, 2, 3, 4, 5, 20, 21, 22, 23]);
        assert!(!is_night(24));
    }

    /// Every GTA pass in the order a conversion runs them (a selection on the read world, then the passes over a
    /// loaded part, by day, at night and with the hour kept as state; examples, not a recipe: the program chooses its
    /// own) is accepted by the pass manager: no pass provides what an earlier one settled (core.surfaces after
    /// lower-gta-surfaces). Declarations only, no game data.
    #[test]
    fn the_gta_passes_in_conversion_order_are_not_refused() {
        use primase_core::pass::{Error, PassManager, PassRegistry, RunReport};
        let mut r = PassRegistry::new();
        r.register("select-lod", primase_core::select_lod);
        r.register("resolve-state", primase_core::state::resolve_state);
        r.register("normalize-normals", |_| {
            Ok(Box::new(primase_image::NormalizeNormals))
        });
        crate::register(&mut r);
        let m = PassManager::new(r);
        let rect = "x0=0,y0=0,x1=1,y1=1";
        let select = |assign: &str, lights: &str| {
            format!(
                "resolve-state{{{assign}}}, gta-select-region{{{rect},min_size=0.5,max_priority=3}}, \
                gta-leave-out-interiors, lower-gta-lights{{{lights}{rect}}}, gta-select-collision{{detail=plain}}, \
                lower-gta-collision{{{rect}}}, lower-gta-surfaces, gta-select-water{{{rect},clip=kept}}"
            )
        };
        let part = |form: &str, night: &str| {
            format!(
                "select-lod{{form={form}}}, lower-gta-embedded-collision, lower-gta-surfaces, \
                gta-pose-fragments, lower-gta-pbr{{night={night}}}, gta-winding, gta-tint, gta-terrain-layers, \
                gta-vertex-colours, lower-gta-water, normalize-normals"
            )
        };
        let pipelines = [
            select("assign=gta.map:online;gta.hour:12", "state=false,"),
            part("finest", "false"),
            select("assign=gta.map:online;gta.hour:22", "hour=22,"),
            part("1", "true"),
            select("assign=gta.map:story,keep=gta.hour", "state=true,"),
            part("finest", "state"),
        ];
        for text in pipelines {
            let p = m.registry.parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            // an empty world: the run may stop for other reasons, never this one
            let r = m.run(&mut primase_core::World::new(), &p, &mut RunReport::default());
            assert!(
                !matches!(r, Err(Error::SettledThenChanged { .. })),
                "{text}: {r:?}"
            );
        }
    }
}
