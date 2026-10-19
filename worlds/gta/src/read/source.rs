//! The GTA side of every conversion: an install opened once, a region read into a world of placements (both maps'
//! states, every entity within reach), and the subsets a writer asks for loaded with their drawables and textures.
//! The reader only reads: which map state, hour and placements a conversion keeps are passes the program chooses
//! (`resolve-state`, `gta-select-region`, ...), run on the read world before its parts are planned; what a loaded
//! world becomes is passes too (`lower-gta-pbr`, ...), in the program's order with its options. A writer
//! that writes a large region cell by cell loads each cell's roots ([`GtaSource::load`]); one that writes everything
//! loads every placement once.
use std::collections::BTreeSet;
use std::path::Path;

use primase_core::{Entity, Instance, World};

use crate::Result;
use crate::components::GtaArchetype;
use crate::read::archive::archive_set::ArchiveSet;
use crate::read::archive::archive_set::Install;
use crate::read::archive::keys::Keys;
use crate::read::assets::{AssetReport, LightsReport, attach_assets_with, attach_lights};
use crate::read::collision::{BoundsReport, attach_map_bounds};
use crate::read::gtxd::TextureParents;
use crate::read::region::{Region, RegionReport, States, archive_sets, read_region};

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
    /// map-state and hour conditions ([`crate::read::region::read_region`]), each prototype's light records
    /// ([`crate::read::assets::attach_lights`]), the static map bounds meeting the region
    /// ([`crate::read::collision::attach_map_bounds`]) and every water file ([`crate::water::attach_water`]: each quad of
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
