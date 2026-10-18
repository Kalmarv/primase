//! The GTA side of every conversion: an install opened once, a region read into a world of placements (both maps'
//! states, every entity within reach), and the subsets a writer asks for loaded with their drawables and textures.
//! The reader only reads: which map state, hour and placements a conversion keeps are passes ([`select_passes`]), run
//! on the read world before a writer plans; what a loaded world becomes is passes too ([`DEFAULT_PASSES`]). A writer
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

/// The passes a loaded GTA world goes through before a writer, in order: each prototype's finest level of detail (a
/// writer that draws chooses no level itself; the passes after it then have no discarded forms to lower), the embedded
/// collision bounds of props as their prototypes' collision (their surfaces classified), fragments'
/// models at their bones' poses, shaders lowered to `pbr.material`, winding turned to the stored normals, tint palettes
/// as per-vertex colour, terrain as layered materials, vertex colours to glTF's meaning, water surfaces as meshes,
/// normal maps to glTF's convention.
pub const DEFAULT_PASSES: &str = "select-lod{form=finest}, lower-gta-embedded-collision, lower-gta-surfaces, gta-pose-fragments, lower-gta-pbr, gta-winding, gta-tint, gta-terrain-layers, gta-vertex-colours, lower-gta-water, normalize-normals";

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

/// What a conversion keeps of a read region: how far outside it entities are read, the map state, the hour, the region
/// rules' minimum size and maximum priority, and whether entities placed in interiors stay. Each is the composing
/// program's choice; [`Selection::new`] keeps everything the region holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// `[x0, y0, x1, y1]`, GTA ground coordinates.
    pub region: [f32; 4],
    /// How far outside the region entities are read (metres), so large pieces of the world that reach in are seen.
    pub reach: f32,
    pub map: Map,
    /// The hour time archetypes are resolved at; `None`: the `gta.hour` axis's default (12).
    pub hour: Option<u8>,
    /// The region rules' minimum placement size ([`crate::select::SelectRegion::min_size`]).
    pub min_size: f32,
    /// The region rules' maximum priority level ([`crate::select::SelectRegion::max_priority`]; 3 keeps every
    /// placement, 2 leaves out the optional ones the game does not spawn at its default density).
    pub max_priority: u8,
    /// Keep the entities placed in interiors. The game draws an interior only from inside it (or through its portals),
    /// so a view of the streets leaves them out.
    pub interiors: bool,
}

impl Selection {
    /// The region `[x0, y0, x1, y1]` of `map` as read: entities only inside it, at the `gta.hour` axis's default hour,
    /// every size and priority, interiors kept.
    pub fn new(region: [f32; 4], map: Map) -> Selection {
        Selection {
            region,
            reach: 0.0,
            map,
            hour: None,
            min_size: 0.0,
            max_priority: 3,
            interiors: true,
        }
    }
}

/// The selection as passes on the read world: `resolve-state` to the map state (and hour), `gta-select-region` with
/// the region rules, unless interiors stay `gta-leave-out-interiors`, then `lower-gta-lights` for the placements kept
/// (inside the region; given an hour, those on then), `gta-select-collision` (the plain map bounds) and
/// `lower-gta-collision` for them (triangles inside the region).
pub fn select_passes(s: &Selection) -> String {
    let mut assign = format!("gta.map:{}", s.map.value());
    if let Some(h) = s.hour {
        assign.push_str(&format!(";gta.hour:{h}"));
    }
    let [x0, y0, x1, y1] = s.region;
    let mut p = format!(
        "resolve-state{{assign={assign}}}, gta-select-region{{x0={x0},y0={y0},x1={x1},y1={y1},min_size={},max_priority={}}}",
        s.min_size, s.max_priority
    );
    if !s.interiors {
        p.push_str(", gta-leave-out-interiors");
    }
    // every light the kept placements carry (each with its hours) unless an hour is asked for
    let hour = s.hour.map_or(String::new(), |h| format!("hour={h},"));
    p.push_str(&format!(
        ", lower-gta-lights{{{hour}x0={x0},y0={y0},x1={x1},y1={y1}}}"
    ));
    p.push_str(", gta-select-collision");
    p.push_str(&format!(
        ", lower-gta-collision{{x0={x0},y0={y0},x1={x1},y1={y1}}}"
    ));
    p.push_str(", lower-gta-surfaces");
    p.push_str(&format!(
        ", gta-select-water{{x0={x0},y0={y0},x1={x1},y1={y1},clip=kept}}"
    ));
    p
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
        let mut w = world.subset(&world.closure(roots.iter().copied()));
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
