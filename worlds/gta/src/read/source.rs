//! The GTA side of every conversion: an install opened once, a region read into a world of placements (both maps'
//! states, every entity within reach), and the subsets a writer asks for loaded with their drawables and textures,
//! each prototype's from the map state it is resolved in (a payload one state resolves differently is a prototype of
//! that state, [`crate::read::region`]).
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
use crate::read::assets::{AssetReport, LightsReport, Lookup, Lookups, attach_assets_with, attach_lights};
use crate::read::collision::{BoundsReport, attach_map_bounds};
use crate::read::gtxd::TextureParents;
use crate::read::region::{Region, RegionReport, States, archive_sets, read_region};
use crate::read::shaders::{
    EffectJoin, ShaderTable, add_shader_database, add_used_shader_effects, join_materials,
};

/// The file name of the Enhanced shader database.
const SHADER_DATABASE: &str = "sga_win32_60_final.awc";

/// What reading a region did.
#[derive(Clone, Debug)]
pub struct ReadReport {
    pub region: RegionReport,
    pub lights: LightsReport,
    pub bounds: BoundsReport,
    /// Each water file: its path with its water, calming and wave quad counts.
    pub water: Vec<(String, [usize; 3])>,
}

/// A live map file and the map states it is live in ([`GtaSource::list_maps`]).
#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub map: crate::read::region::MapListing,
    /// Live in story mode's archive set, and in GTA Online's.
    pub story: bool,
    pub online: bool,
}

/// An opened install: its key material, both maps' archive sets and each one's texture dictionary parents.
pub struct GtaSource {
    keys: Keys,
    story: ArchiveSet,
    online: ArchiveSet,
    story_parents: TextureParents,
    online_parents: TextureParents,
    /// Each state's archetypes by asset, read from its map types files the first time an entry needs them
    /// ([`GtaSource::read_entry`]).
    pub(crate) story_archetypes:
        std::sync::OnceLock<std::result::Result<crate::read::entry::Archetypes, String>>,
    pub(crate) online_archetypes:
        std::sync::OnceLock<std::result::Result<crate::read::entry::Archetypes, String>>,
}

impl std::fmt::Debug for GtaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaSource").finish_non_exhaustive()
    }
}

impl GtaSource {
    /// Opens the install at `dir` with the key material in `keys_dir`: Story's and GTA Online's archive sets and
    /// their texture dictionary parents.
    pub fn open(dir: &Path, keys_dir: &Path) -> Result<GtaSource> {
        let keys = Keys::from_dir(keys_dir)?;
        let install = Install::open(dir, &keys)?;
        let (story, online) = archive_sets(&install, &keys, States::All)?;
        let online = online.ok_or_else(|| crate::invalid("install", 0, "no GTA Online archive set"))?;
        let story_parents = TextureParents::read(&story, &keys)?;
        let online_parents = TextureParents::read(&online, &keys)?;
        Ok(GtaSource {
            keys,
            story,
            online,
            story_parents,
            online_parents,
            story_archetypes: std::sync::OnceLock::new(),
            online_archetypes: std::sync::OnceLock::new(),
        })
    }

    /// The opened install: its archives and their files ([`Install::listing`]).
    pub fn install(&self) -> &Install {
        &self.story.install
    }

    /// A binary file of the install as its archive stores it, decrypted with the install's keys: a compressed file's
    /// raw deflate stream with the size it inflates to, or an uncompressed file's contents with `None`
    /// ([`Install::stored`]).
    pub fn stored(&self, f: crate::read::archive::archive_set::FileRef) -> Result<(Vec<u8>, Option<usize>)> {
        self.install().stored(f, &self.keys)
    }

    /// The Enhanced shader database of the install ([`crate::read::shaders`]: its programs as `core.shader` entities and
    /// its effects as `gta.effect`) added to `w`, and every material of `w` linked to its effect
    /// ([`crate::read::shaders::join_materials`]). The database is the install's `sga_win32_60_final.awc` of the
    /// `fxdb` folder, found by the listing; an install with none, or with several, is an error. Reads and lifts the
    /// interface of about 11,000 programs: call it once per world.
    pub fn add_shader_database(&self, w: &mut World) -> Result<(ShaderTable, EffectJoin)> {
        let bytes = self.shader_database_bytes()?;
        let db = crate::rage::sgd2::parse(&bytes)?;
        let table = add_shader_database(w, &db);
        let join = join_materials(w, &table.effects);
        Ok((table, join))
    }

    /// The effects `w`'s materials name, with the programs they list, added to `w` and every material linked to its
    /// effect, from the database bytes ([`GtaSource::shader_database_bytes`], which a caller that does this for several
    /// worlds reads once): what [`GtaSource::add_shader_database`] makes, for the part of the database a part of the map
    /// uses.
    pub fn add_used_shader_effects(
        &self,
        w: &mut World,
        database: &[u8],
    ) -> Result<(ShaderTable, EffectJoin)> {
        let db = crate::rage::sgd2::parse(database)?;
        let table = add_used_shader_effects(w, &db);
        let join = join_materials(w, &table.effects);
        Ok((table, join))
    }

    /// The install's shader database file (`sga_win32_60_final.awc` of the `fxdb` folder), decrypted and unpacked as
    /// stored; an install with none, or with several, is an error.
    pub fn shader_database_bytes(&self) -> Result<Vec<u8>> {
        let mut found = self
            .install()
            .listing()
            .filter(|f| f.name.eq_ignore_ascii_case(SHADER_DATABASE) && f.path.contains("\\fxdb\\"));
        let file = found.next().ok_or_else(|| {
            crate::invalid(
                "shader database",
                0,
                format!("no {SHADER_DATABASE} in the install"),
            )
        })?;
        if found.next().is_some() {
            return Err(crate::invalid(
                "shader database",
                0,
                format!("more than one {SHADER_DATABASE} in the install"),
            ));
        }
        self.install().binary(file.file, self.keys())
    }

    pub(crate) fn keys(&self) -> &Keys {
        &self.keys
    }

    pub(crate) fn story(&self) -> Lookup<'_> {
        Lookup {
            set: &self.story,
            parents: &self.story_parents,
        }
    }

    pub(crate) fn online(&self) -> Lookup<'_> {
        Lookup {
            set: &self.online,
            parents: &self.online_parents,
        }
    }

    pub(crate) fn lookups(&self) -> Result<Lookups<'_>> {
        Lookups::new(self.story(), Some(self.online()))
    }

    /// Every live map file of both map states as its header states it ([`crate::read::region::list_maps`]): one entry
    /// per file, with the states it is live in (a file both states mount once, with both), sorted by path then file.
    /// On `threads` threads: the same list on any count.
    pub fn list_maps(&self, threads: primase_core::parallel::Threads) -> Result<Vec<MapEntry>> {
        let mut by_file: std::collections::BTreeMap<
            (String, crate::read::archive::archive_set::FileRef),
            MapEntry,
        > = std::collections::BTreeMap::new();
        for (set, story) in [(&self.story, true), (&self.online, false)] {
            for map in crate::read::region::list_maps(set, &self.keys, threads)? {
                let entry = by_file
                    .entry((map.path.clone(), map.file))
                    .or_insert_with(|| MapEntry {
                        map,
                        story: false,
                        online: false,
                    });
                if story {
                    entry.story = true;
                } else {
                    entry.online = true;
                }
            }
        }
        Ok(by_file.into_values().collect())
    }

    /// Every entity of both maps within `reach` metres of `region` (`[x0, y0, x1, y1]`), with its prototype and its
    /// map-state and hour conditions ([`crate::read::region::read_region`]), each prototype's light records from the
    /// state it is resolved in ([`crate::read::assets::attach_lights`]), both states' static map bounds meeting the
    /// region, a bound live in one only with its `gta.map` condition ([`crate::read::collision::attach_map_bounds`]),
    /// and every water file ([`crate::read::water::attach_water`]: each quad of
    /// the whole map placed with its file's `gta.water` condition, the rest of each file a `gta.water`); no geometry
    /// or textures yet.
    pub fn read(&self, region: [f32; 4], reach: f32) -> Result<(World, ReadReport)> {
        let bounds_region = region;
        let (mut w, region) = read_region(
            self.story(),
            Some(self.online()),
            &self.keys,
            &Region {
                min: [region[0], region[1]],
                max: [region[2], region[3]],
                reach,
            },
        )?;
        let lights = attach_lights(&mut w, self.lookups()?, &self.keys)?;
        let bounds = attach_map_bounds(&mut w, &self.story, Some(&self.online), &self.keys, bounds_region)?;
        // the water files are the install's, the same for both states
        let water = crate::read::water::attach_water(&mut w, &self.story.install, &self.keys)?;
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
    /// textures of its placements' prototypes attached, each from the state it is resolved in. Entity ids stay `world`'s; what attaching spawns (LOD forms,
    /// materials, textures) is reached through the prototypes.
    pub fn load(&self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, AssetReport)> {
        // the world's remaining axes (global state a consumer resolves itself, a kept gta.hour) are in every part, so
        // a per-part pass can refer to them
        let axes = world.iter::<primase_core::state::StateAxis>().map(|(e, _)| e);
        let mut w = world.subset(&world.closure(roots.iter().copied().chain(axes)));
        let report = attach_assets_with(&mut w, self.lookups()?, &self.keys)?;
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
        return world
            .get::<crate::components::water::GtaWaterSurface>(prototype)?
            .bounds();
    };
    let (lo, hi) = (a.bb_min.map(f64::from), a.bb_max.map(f64::from));
    // GTA (x, y, z) is core (x, z, -y)
    Some([[lo[0], lo[2], -hi[1]], [hi[0], hi[2], -lo[1]]])
}
