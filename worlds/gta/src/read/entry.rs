//! One archive entry read into a world as itself, for a consumer that browses an install ([`GtaSource::read_entry`]):
//! each kind attaches what the reader reads for it when a placement uses it, and nothing else.
//!
//! - A drawable (`ydr`), a fragment (`yft`) or a drawable dictionary (`ydd`, each entry): a prototype per drawable
//!   with the loading placements get ([`crate::read::assets`]): its mesh and levels of detail, materials, textures
//!   resolved the way the game looks them up, embedded collision, skeleton, and its lights ([`GtaLights`]). The
//!   drawable is the chosen file itself, whichever copy of its name the game keeps live. The textures are resolved
//!   through the map state whose archive set has the file live (Story's when neither does). The archetype that names
//!   the drawable's texture dictionary is the first live map types file's that names the asset ([`GtaArchetype`], as a
//!   placement's prototype has it); when none does, the prototype gets an archetype made of the file's name alone and
//!   no texture dictionary (counted in [`EntryReport::archetypes_made`]). The prototypes are the parts of an asset
//!   (`core.asset`), unplaced: `place-assets` places them at the origin, a dictionary's entries on top of each other.
//! - A map (`ymap`): every entity of the file as a placement of its archetype's prototype, read as a region is
//!   ([`crate::read::region::read_map_file`]: the map states the file is live in, the script-managed map that governs
//!   it) and the light records of the prototypes, with no geometry or textures yet: [`GtaSource::load`] loads them,
//!   as it loads a region's.
//! - A texture dictionary (`ytd`): its textures, each a `core.texture` as the reader makes them for materials.
//! - Static collision (`ybn`): the bound as a [`GtaBounds`] entity at the origin, and the install's material names.
use std::collections::BTreeMap;

use primase_core::asset::{Asset, Role};
use primase_core::{Entity, Name, Provenance, Source, World};

use crate::components::collision::GtaMaterials;
use crate::components::{GtaArchetype, GtaResolvedIn};
use crate::rage::drawable::{DrawableDictionary, resource_lights};
use crate::rage::map::{ArchetypeDef, MapTypes};
use crate::rage::meta::Meta;
use crate::rage::texture::TextureDictionary;
use crate::read::archive::archive_set::{ArchiveSet, FileRef};
use crate::read::archive::keys::Keys;
use crate::read::archive::rpf::EntryKind;
use crate::read::assets::{
    AssetReport, Content, Ctx, LightsReport, Lookup, attach_lights, decode, load_prototype, origin,
    put_lights,
};
use crate::read::collision::{BoundsReport, install_material_names, place_map_bounds};
use crate::read::region::{RegionReport, read_map_file};
use crate::read::source::GtaSource;
use crate::{Result, invalid};

/// A map state's archetypes by their asset (type, name hash), each with the map types file that defines it, in the
/// live files' order: the first is the one a placement takes.
pub type Archetypes = BTreeMap<(i32, u32), Vec<(String, ArchetypeDef)>>;

/// Every archetype of the archive set's live map types files.
fn read_archetypes(set: &ArchiveSet, keys: &Keys) -> Result<Archetypes> {
    let mut out = Archetypes::new();
    for &f in set.tables.get("ytyp").into_iter().flat_map(|t| t.values()) {
        let r = set.install.resource(f, keys)?;
        let types = MapTypes::from_root(&Meta::read(&r)?.root()?)?;
        for a in types.archetypes {
            out.entry((a.asset_type, a.asset_name))
                .or_default()
                .push((set.path(f).to_string(), a));
        }
    }
    Ok(out)
}

/// What reading an entry did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntryReport {
    /// The entry's path and its lowercase extension.
    pub path: String,
    pub extension: String,
    /// The map state whose archive set resolved the entry's textures: `story` or `online`.
    pub state: &'static str,
    /// Prototypes whose archetype a live map types file defines, and those given one made of the file's name alone.
    pub archetypes_found: usize,
    pub archetypes_made: usize,
    /// What loading the drawables counted, and their lights.
    pub assets: AssetReport,
    pub lights: LightsReport,
    /// A dictionary's entries (drawables of a `ydd`, textures of a `ytd`) and, of a `ytd`'s, those with no texture.
    pub entries: usize,
    pub entries_without_texture: usize,
    pub textures: usize,
    pub bounds: BoundsReport,
    /// What reading a map's placements counted (a `ymap`).
    pub region: RegionReport,
}

/// The lowercase name of an entry without its extension, and the extension.
fn split_name(path: &str) -> (String, String) {
    let name = path.rsplit('\\').next().unwrap_or(path).to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_owned(), ext.to_owned()),
        None => (name, String::new()),
    }
}

impl GtaSource {
    /// The file at `path` (lowercase, `\`: [`crate::read::archive::archive_set::Install::file`]).
    pub fn file(&self, path: &str) -> Option<FileRef> {
        self.install().file(path)
    }

    /// The archive set and state a file's textures resolve through: the state whose archive set has it live, Story's
    /// when neither does.
    pub(crate) fn lookup_of(&self, ext: &str, stem: &str, file: FileRef) -> (Lookup<'_>, bool) {
        let hash = primase_foundations::hash::joaat(stem.as_bytes());
        let live = |l: Lookup<'_>| l.set.tables.get(ext).and_then(|t| t.get(&hash)) == Some(&file);
        if !live(self.story()) && live(self.online()) {
            (self.online(), true)
        } else {
            (self.story(), false)
        }
    }

    fn archetypes(&self, online: bool) -> Result<&Archetypes> {
        let (cell, lookup) = if online {
            (&self.online_archetypes, self.online())
        } else {
            (&self.story_archetypes, self.story())
        };
        cell.get_or_init(|| read_archetypes(lookup.set, self.keys()).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| invalid("archetypes", 0, e.clone()))
    }

    /// The file `file` read into a world as itself (see the module documentation): `ydr`, `yft`, `ydd`, `ytd`, `ybn` and
    /// `ymap` files; any other kind is an error. Reads from the install only, and chooses nothing: a drawable's
    /// prototypes stay unplaced, at their authored levels of detail.
    pub fn read_entry(&self, file: FileRef) -> Result<(World, EntryReport)> {
        let entry = self
            .install()
            .archives
            .get(file.archive)
            .and_then(|a| a.entries.get(file.entry))
            .filter(|e| !matches!(e.kind, EntryKind::Directory { .. }))
            .ok_or_else(|| {
                invalid(
                    "entry",
                    0,
                    format!("no file at archive {} entry {}", file.archive, file.entry),
                )
            })?;
        let path = entry.path.clone();
        let (stem, ext) = split_name(&path);
        let mut report = EntryReport {
            path: path.clone(),
            extension: ext.clone(),
            state: "story",
            ..EntryReport::default()
        };
        let world = match ext.as_str() {
            "ydr" | "yft" | "ydd" => self.drawables(file, &stem, &ext, &mut report)?,
            "ytd" => self.textures(file, &mut report)?,
            "ybn" => self.bounds(file, &stem, &mut report)?,
            "ymap" => self.map(file, &mut report)?,
            _ => {
                return Err(invalid(
                    "entry",
                    0,
                    format!("{path}: reading a .{ext} file as itself is not supported"),
                ));
            }
        };
        Ok((world, report))
    }

    fn drawables(&self, file: FileRef, stem: &str, ext: &str, report: &mut EntryReport) -> Result<World> {
        let (install, keys) = (self.install(), self.keys());
        let (lookup, online) = self.lookup_of(ext, stem, file);
        report.state = if online { "online" } else { "story" };
        let stem_hash = primase_foundations::hash::joaat(stem.as_bytes());
        let resource = decode(install, keys, file)?;
        // the drawables the file holds: its own, or a dictionary's entries by name hash
        let (asset_type, assets): (i32, Vec<u32>) = match ext {
            "ydr" => (2, vec![stem_hash]),
            "yft" => (1, vec![stem_hash]),
            _ => (3, DrawableDictionary::read_resource(&resource)?.hashes),
        };
        report.entries = if ext == "ydd" { assets.len() } else { 0 };
        let archetypes = self.archetypes(online)?;
        let mut w = World::new();
        let mut ctx = Ctx::new(install, keys);
        let mut parts = Vec::new();
        for asset_name in assets {
            let defined = archetypes.get(&(asset_type, asset_name)).and_then(|list| {
                list.iter()
                    .find(|(_, d)| asset_type != 3 || d.drawable_dictionary == stem_hash)
            });
            let proto = w.spawn();
            let a = match defined {
                Some((ytyp, def)) => {
                    report.archetypes_found += 1;
                    w.insert(proto, Source(ytyp.clone()));
                    let item = format!("archetype {:#010x}", def.name);
                    w.insert(proto, Provenance::new(origin(ytyp, Some(&item))));
                    GtaArchetype::new(ytyp, def)
                }
                None => {
                    report.archetypes_made += 1;
                    made_archetype(asset_type, asset_name, stem_hash)
                }
            };
            w.insert(
                proto,
                GtaResolvedIn {
                    story: !online,
                    online,
                },
            );
            let name = if ext == "ydd" {
                format!("{stem}#{asset_name:#010x}")
            } else {
                stem.to_owned()
            };
            w.insert(proto, Name(name));
            w.insert(proto, a.clone());
            load_prototype(&mut ctx, &mut w, proto, &a, lookup, file, Content::Whole)?;
            let entry = if ext == "ydd" { asset_name } else { 0 };
            put_lights(
                &mut w,
                &mut report.lights,
                proto,
                resource_lights(&resource, ext, entry)?,
            );
            parts.push((proto, Role::Part));
        }
        report.assets = ctx.report;
        let root = w.spawn();
        w.insert(root, Asset { members: parts });
        w.insert(root, Name(stem.to_owned()));
        w.insert(root, Source(report.path.clone()));
        Ok(w)
    }

    fn map(&self, file: FileRef, report: &mut EntryReport) -> Result<World> {
        let (mut w, region) = read_map_file(self.story(), Some(self.online()), self.keys(), file)?;
        report.lights = attach_lights(&mut w, self.lookups()?, self.keys())?;
        report.region = region;
        Ok(w)
    }

    fn textures(&self, file: FileRef, report: &mut EntryReport) -> Result<World> {
        let (install, keys) = (self.install(), self.keys());
        let dictionary = TextureDictionary::read_resource(&decode(install, keys, file)?)?;
        let mut w = World::new();
        let mut ctx = Ctx::new(install, keys);
        for texture in &dictionary.textures {
            report.entries += 1;
            match texture {
                Some(t) => {
                    ctx.texture_entity(&mut w, &report.path, t)?;
                    report.textures += 1;
                }
                None => report.entries_without_texture += 1,
            }
        }
        Ok(w)
    }

    fn bounds(&self, file: FileRef, stem: &str, report: &mut EntryReport) -> Result<World> {
        let (install, keys) = (self.install(), self.keys());
        let bound = crate::rage::bounds::read_ybn(&install.resource(file, keys)?)?;
        let mut w = World::new();
        // a file in a map archive is a map bound: stored in world space, so at the origin
        place_map_bounds(
            &mut w,
            vec![(report.path.clone(), bound, (true, true))],
            None,
            &mut report.bounds,
        );
        let bound_entity: Vec<Entity> = w.entities().collect();
        for e in bound_entity {
            w.insert(e, Name(stem.to_owned()));
            w.insert(e, Source(report.path.clone()));
        }
        if let Some(names) = install_material_names(self.story().set, keys)? {
            report.bounds.material_names = names.len();
            w.insert_resource(GtaMaterials(names));
        }
        Ok(w)
    }
}

/// An archetype for a drawable no live map types file names: its asset and nothing else, in particular no texture
/// dictionary (the drawable's own textures and the resident shared dictionaries are all its materials can find).
pub(crate) fn made_archetype(asset_type: i32, asset_name: u32, dictionary: u32) -> GtaArchetype {
    GtaArchetype {
        ytyp: String::new(),
        name: asset_name,
        kind: "base",
        flags: 0,
        special_attribute: 0,
        lod_dist: 0.0,
        hd_texture_dist: 0.0,
        bb_min: [0.0; 3],
        bb_max: [0.0; 3],
        bs_centre: [0.0; 3],
        bs_radius: 0.0,
        texture_dictionary: 0,
        drawable_dictionary: if asset_type == 3 { dictionary } else { 0 },
        physics_dictionary: 0,
        clip_dictionary: 0,
        asset_type,
        asset_name,
        time_flags: None,
    }
}
