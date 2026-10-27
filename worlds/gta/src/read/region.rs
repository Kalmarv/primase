//! A region of the map into the world: every entity of the live maps whose pivot lies within the region plus a reach,
//! as a placement of its archetype's prototype, in the world frame.
//!
//! - Map state: the story-mode map is read; given GTA Online's archive set too ([`archive_sets`], [`States::All`]), and an entity that is live in one
//!   only carries a `core.condition` on the global axis `gta.map` {story, online} (default story, the game's start
//!   state). Entities are the same in both when their map file is.
//! - Time archetypes are shown in some hours only: their placements carry a `core.condition` on the global axis
//!   `gta.hour` {0..23} with the hours their timeFlags name; `resolve-state` picks the hour. The axis has no default:
//!   the game's clock is the save's or a script's.
//! - Script-managed maps (`CMapData` flags bit 0: the game streams them only when a script requests them, an IPL):
//!   each is a global bool axis `gta.ipl.<map name>` (class `ipl`), and the placements of that map, and of maps whose
//!   LOD parent chain reaches it, carry a condition on it. The axis has no default: whether the game's start scripts
//!   load it is in those scripts, not read ([`crate::components::state`]).
//! - Archetypes come from every live map types file; a name defined in more than one is counted and the first kept.
//! - Each state resolves an archetype's payload (its drawable and the texture dictionaries of its lookup) in its own
//!   archive set: a prototype is one archetype definition with one resolved payload ([`GtaResolvedIn`] names the
//!   states it holds for). A placement live in both states is placed once when both define its archetype in the same
//!   map types file and resolve its payload to the same files, and once per state otherwise.
//! - Transforms: GTA's frame is east-north-up ([`crate::components::GTA`]); an entity's orientation is its stored quaternion inverted
//!   (an interior instance's as stored), its scale `scaleXY, scaleXY, scaleZ`, computed in f64 and rounded once.
//! - Which entities a scene finally draws (LOD stand-ins, priority, size) is for passes; the reader keeps them all.
//!   An interior instance's own entities are placed by its rotation and position; its entity sets (which scripts
//!   switch) are not read yet (counted).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads map files from the install"
)]
use std::collections::BTreeMap;
use std::fs::File;

use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Instance, Name, Provenance, Source, Transform, World};

use crate::components::state::hour_axis;
use crate::components::{GTA, GtaArchetype, GtaEntity, GtaResolvedIn};
use crate::read::assets::{Lookup, Payload};
use crate::{Result, invalid, path};
use primase_foundations::parallel::Threads;
use rage_engine_formats::archive::archive_set::{ArchiveSet, FileRef, Install, MapState};
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::map::{ArchetypeDef, EntityDef, EntityKind, MapData, MapHeader, MapTypes};
use rage_engine_formats::meta::Meta;

/// Which map states to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum States {
    /// The story-mode map only (no state components).
    None,
    /// Story mode and GTA Online, with conditions on the `gta.map` axis.
    All,
}

/// A region of the map in GTA's ground coordinates (metres east, north).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// How far outside the region an entity's pivot may lie and still be read (large pieces reach in).
    pub reach: f32,
}

impl Region {
    fn holds_pivot(&self, p: [f32; 3]) -> bool {
        (0..2).all(|i| p[i] >= self.min[i] - self.reach && p[i] < self.max[i] + self.reach)
    }

    fn touches(&self, box_min: [f32; 3], box_max: [f32; 3]) -> bool {
        (0..2).all(|i| box_min[i] <= self.max[i] + self.reach && box_max[i] >= self.min[i] - self.reach)
    }
}

/// What reading counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionReport {
    pub maps: usize,
    pub placements: usize,
    pub prototypes: usize,
    /// Prototypes named by their archetype's hash: a drawable dictionary's entry (the dictionary stores entries by
    /// hash) or an asset with no live file. The others are named by their drawable's or fragment's file.
    pub prototypes_named_by_hash: usize,
    /// Placements by condition: `both`, `story`, `online`.
    pub by_state: BTreeMap<&'static str, usize>,
    /// Archetype names defined by more than one live map types file.
    pub duplicate_archetypes: usize,
    /// Entities whose archetype no live map types file of the states they are live in defines.
    pub missing_archetypes: usize,
    /// Entities live in both states whose archetype the two states define in different files or resolve to different
    /// payloads: placed once per state, each with its state's definition and payload and a `gta.map` condition.
    pub split_by_state: usize,
    /// Of those, the ones whose archetype both states define in the same file (the payload alone differs).
    pub split_by_payload: usize,
    /// Placements of time archetypes (each with an hour mask on the `gta.hour` axis).
    pub time_placements: usize,
    /// Of those, the ones whose mask names no hour: kept, with the condition that never holds.
    pub time_placements_never_shown: usize,
    /// Script-managed maps with placements here (each a `gta.ipl.<name>` axis, its start state unknown), and the
    /// placements they govern.
    pub ipl_axes: usize,
    pub ipl_placements: usize,
    /// Interior instances read, the entities placed in them, and their entity sets (not read yet).
    pub interiors: usize,
    pub interior_entities: usize,
    pub interior_entity_sets: usize,
}

/// One live map's entities near the region.
#[derive(Clone)]
struct MapRead {
    path: String,
    /// The live LOD parent map's path, when it has one.
    parent: Option<String>,
    /// The script-managed map that governs it: itself, or the nearest of its LOD parent chain that is.
    ipl: Option<String>,
    entities: Vec<(u32, EntityDef)>,
}

/// A map's name: its file name without the extension.
/// The path of the live map named by hash `parent` (a map's LOD parent); `None` for no parent (0) or one no live map
/// is.
fn parent_path(set: &ArchiveSet, parent: u32) -> Result<Option<String>> {
    match set.tables.get("ymap").and_then(|t| t.get(&parent)) {
        Some(&p) if parent != 0 => Ok(Some(path(set, p)?.to_owned())),
        _ => Ok(None),
    }
}

fn map_name(path: &str) -> String {
    let file = path.rsplit('\\').next().unwrap_or(path);
    file.strip_suffix(".ymap").unwrap_or(file).to_string()
}

fn meta(set: &ArchiveSet, keys: &Keys, f: FileRef) -> Result<Meta> {
    let a = &set.install.archives[f.archive];
    let r = a.resource(&mut File::open(&a.file)?, f.entry, keys)?;
    Meta::read(&r)
}

fn decode(set: &ArchiveSet, keys: &Keys, f: FileRef) -> Result<rage_engine_formats::meta::Struct> {
    meta(set, keys, f)?.root()
}

/// A map file's header, the whole file walked and checked ([`Meta::root_scalars`]) but its entities not kept.
fn header(set: &ArchiveSet, keys: &Keys, f: FileRef) -> Result<MapHeader> {
    MapHeader::from_root(&meta(set, keys, f)?.root_scalars()?)
}

/// One live map file as its header states it ([`list_maps`]).
#[derive(Clone, Debug, PartialEq)]
pub struct MapListing {
    /// The file's path in the install.
    pub path: String,
    pub file: FileRef,
    /// Its own fields: name, LOD parent, flags (bit 0: script-managed), content flags, streaming and entity extents.
    pub header: MapHeader,
    /// Its entities (`CMapData.entities`), counted, none read.
    pub entities: usize,
    /// The script-managed map that governs it: itself, or the nearest of its LOD parent chain that is (as
    /// [`read_region`] conditions its placements).
    pub ipl: Option<String>,
}

/// Every live map file of `set`, each as its header states it ([`MapListing`]): every file walked and checked as a
/// region read walks it ([`Meta::root_outline`]), no entity read; in the order of the set's map table (by name hash).
/// The files are read on `threads` threads, one each at a time: the same list on any count.
pub fn list_maps(set: &ArchiveSet, keys: &Keys, threads: Threads) -> Result<Vec<MapListing>> {
    let files: Vec<FileRef> = set
        .tables
        .get("ymap")
        .into_iter()
        .flat_map(|t| t.values().copied())
        .collect();
    let entities = primase_foundations::hash::joaat_exact(b"entities");
    let outlines =
        primase_foundations::parallel::map(threads, &files, |_, &f| -> Result<(MapHeader, usize)> {
            let (root, counts) = meta(set, keys, f)?.root_outline()?;
            let Some(&count) = counts.get(&entities) else {
                return Err(invalid(
                    "CMapData",
                    0,
                    format!("{}: no entities array", path(set, f)?),
                ));
            };
            Ok((MapHeader::from_root(&root)?, count))
        })
        .map_err(|e| invalid("map listing", 0, e.to_string()))?;
    let mut listed = Vec::with_capacity(files.len());
    for (&f, outline) in files.iter().zip(outlines) {
        let (header, count) = outline?;
        listed.push((f, header, count));
    }
    // every live map's chain entry, so a parent is never read twice
    let mut chain: BTreeMap<u32, (bool, u32, String)> = listed
        .iter()
        .map(|(f, h, _)| Ok((h.name, (h.flags & 1 != 0, h.parent, map_name(path(set, *f)?)))))
        .collect::<Result<_>>()?;
    listed
        .into_iter()
        .map(|(file, header, entities)| {
            Ok(MapListing {
                path: path(set, file)?.to_owned(),
                file,
                ipl: ipl_of(set, keys, &mut chain, &header),
                header,
                entities,
            })
        })
        .collect()
}

/// A map's script-managed map: itself, or the nearest of its LOD parent chain that is (its name). `chain` holds what is
/// known of every map by its name hash (whether it is script-managed, its LOD parent's hash, its name), with `map`'s own
/// entry in it; a parent not seen yet is read from its file.
fn ipl_of(
    set: &ArchiveSet,
    keys: &Keys,
    chain: &mut BTreeMap<u32, (bool, u32, String)>,
    map: &MapHeader,
) -> Option<String> {
    let (mut at, mut found, mut guard) = (map.name, None, 0);
    while let Some((scripted, parent, name)) = chain.get(&at).cloned().or_else(|| {
        // a parent not seen yet: read its header from its file
        let &pf = set.tables.get("ymap")?.get(&at)?;
        let pm = header(set, keys, pf).ok()?;
        let e = (pm.flags & 1 != 0, pm.parent, map_name(set.path(pf)?));
        chain.insert(at, e.clone());
        Some(e)
    }) {
        if scripted {
            found = Some(name);
            break;
        }
        guard += 1;
        if parent == 0 || guard > 16 {
            break;
        }
        at = parent;
    }
    found
}

/// The live maps' entities whose pivot lies within the region and its reach. Every live map's header is read (every
/// file walked and checked), its entities only where its entities' extents touch the region and its reach.
fn region_maps(set: &ArchiveSet, keys: &Keys, region: &Region) -> Result<Vec<MapRead>> {
    let mut out = Vec::new();
    // every live map's name hash: whether it is script-managed, and its LOD parent's
    let mut chain: BTreeMap<u32, (bool, u32, String)> = BTreeMap::new();
    for &f in set.tables.get("ymap").into_iter().flat_map(|t| t.values()) {
        let map = header(set, keys, f)?;
        chain.insert(
            map.name,
            (map.flags & 1 != 0, map.parent, map_name(path(set, f)?)),
        );
        let ipl = ipl_of(set, keys, &mut chain, &map);
        let [lo, hi] = map.entities_extents;
        if !region.touches(lo, hi) {
            continue;
        }
        let entities: Vec<(u32, EntityDef)> = MapData::from_root(&decode(set, keys, f)?)?
            .entities
            .into_iter()
            .enumerate()
            .filter(|(_, e)| region.holds_pivot(e.position))
            .map(|(i, e)| (i as u32, e))
            .collect();
        if !entities.is_empty() {
            out.push(MapRead {
                path: path(set, f)?.to_owned(),
                parent: parent_path(set, map.parent)?,
                ipl,
                entities,
            });
        }
    }
    Ok(out)
}

/// Every archetype of the live map types files, by name hash: the file and the definition (the first, when a name is
/// defined twice), and how many names were.
/// Archetypes by name hash: the map types file defining them and the definition.
type ArchetypeTable = BTreeMap<u32, (String, ArchetypeDef)>;

/// One definition of an archetype for a placement: the map types file defining it, the definition, the lookup its
/// payload is resolved through, that payload, and the states it holds for.
struct Definition<'a, S, P> {
    ytyp: &'a String,
    def: &'a ArchetypeDef,
    set: &'a S,
    payload: P,
    story: bool,
    online: bool,
}

/// The definitions of archetype `h` for a placement live in Story (`in_story`) and Online (`in_online`), from each
/// state's own map types, with the payload `payload` resolves for it in that state (called with whether the state is
/// Online's): one when only one state applies or both states define it in the same file and resolve the same payload,
/// two (one per state) when the file or the payload differs, none when no applicable state defines it.
fn definitions<'a, S, P: PartialEq>(
    h: u32,
    in_story: bool,
    in_online: bool,
    story: (&'a ArchetypeTable, &'a S),
    online: Option<(&'a ArchetypeTable, &'a S)>,
    payload: &mut impl FnMut(bool, &'a S, &'a ArchetypeDef) -> P,
) -> Vec<Definition<'a, S, P>> {
    let s =
        (in_story.then(|| story.0.get(&h)).flatten()).map(|d| (d, story.1, payload(false, story.1, &d.1)));
    let o = (online.filter(|_| in_online))
        .and_then(|(t, set)| t.get(&h).map(|d| (d, set)))
        .map(|(d, set)| (d, set, payload(true, set, &d.1)));
    let def =
        |((ytyp, def), set, payload): (&'a (String, ArchetypeDef), &'a S, P), story, online| Definition {
            ytyp,
            def,
            set,
            payload,
            story,
            online,
        };
    match (s, o) {
        (Some(sd), Some(od)) if sd.0.0 == od.0.0 && sd.2 == od.2 => vec![def(od, true, true)],
        (s, o) => s
            .map(|sd| def(sd, true, false))
            .into_iter()
            .chain(o.map(|od| def(od, false, true)))
            .collect(),
    }
}

fn archetypes(set: &ArchiveSet, keys: &Keys) -> Result<(ArchetypeTable, usize)> {
    let mut out = ArchetypeTable::new();
    let mut duplicates = 0;
    for &f in set.tables.get("ytyp").into_iter().flat_map(|t| t.values()) {
        let types = MapTypes::from_root(&decode(set, keys, f)?)?;
        for a in types.archetypes {
            match out.entry(a.name) {
                std::collections::btree_map::Entry::Occupied(_) => duplicates += 1,
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert((path(set, f)?.to_owned(), a));
                }
            }
        }
    }
    Ok((out, duplicates))
}

/// An entity's transform in GTA's frame: rotation by its (inverted, unless an interior instance) quaternion, scale,
/// translation.
pub fn entity_transform(e: &EntityDef) -> Transform {
    let [x, y, z, w] = e.rotation.map(f64::from);
    let n = x * x + y * y + z * z + w * w;
    // CEntityDef stores the inverse orientation for entities; an identity or zero quaternion is the identity
    let (x, y, z, w) = if n == 0.0 {
        (0.0, 0.0, 0.0, 1.0)
    } else if e.kind == EntityKind::Interior {
        (x / n.sqrt(), y / n.sqrt(), z / n.sqrt(), w / n.sqrt())
    } else {
        let s = n.sqrt();
        (-x / s, -y / s, -z / s, w / s)
    };
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let s = [f64::from(e.scale_xy), f64::from(e.scale_xy), f64::from(e.scale_z)];
    let mut cols = [[0f32; 3]; 4];
    for (c, col) in cols.iter_mut().take(3).enumerate() {
        *col = r[c].map(|v| (v * s[c]) as f32);
    }
    cols[3] = e.position;
    Transform { cols }
}

/// An entity to place: its map, its index there (for an interior's entity, the instance's index and its own),
/// its definition, its transform in GTA's frame, and the map states it is live in.
#[derive(Clone)]
struct Placed {
    ymap: String,
    /// The script-managed map governing it ([`MapRead::ipl`]).
    ipl: Option<String>,
    index: u32,
    child: Option<u32>,
    lod_parent: Option<(String, u32)>,
    def: EntityDef,
    transform: Transform,
    story: bool,
    online: bool,
}

/// The condition of a placement that exists when all of `literals` hold: none without literals, and the condition
/// that never holds when a literal names no value (core refuses an empty literal; an hour mask can be empty).
fn condition(literals: Vec<Literal>) -> Option<Condition> {
    if literals.is_empty() {
        None
    } else if literals.iter().any(|l| l.values == 0) {
        Some(Condition(Vec::new()))
    } else {
        Some(Condition(vec![Clause(literals)]))
    }
}

/// The archive sets for the map states asked for: story mode's, and GTA Online's with [`States::All`].
pub fn archive_sets(
    install: &Install,
    keys: &Keys,
    states: States,
) -> Result<(ArchiveSet, Option<ArchiveSet>)> {
    let story = ArchiveSet::build(install.clone(), keys, MapState::Story)?;
    let online = match states {
        States::All => Some(ArchiveSet::build(install.clone(), keys, MapState::Online)?),
        States::None => None,
    };
    Ok((story, online))
}

/// Reads the region into a new world: placements and their prototypes, each prototype's payload resolved through its
/// state's lookup (geometry and textures come with [`crate::read::assets::attach_assets_with`]). `online`, when given,
/// adds GTA Online's map and the `gta.map` conditions.
pub fn read_region(
    story: Lookup<'_>,
    online: Option<Lookup<'_>>,
    keys: &Keys,
    region: &Region,
) -> Result<(World, RegionReport)> {
    let mut report = RegionReport::default();
    // by (map path, entity index): the entity, its LOD parent, the states it is live in
    let mut placed: BTreeMap<(String, u32), Placed> = BTreeMap::new();
    for (set, is_story) in std::iter::once((story.set, true)).chain(online.map(|s| (s.set, false))) {
        for m in region_maps(set, keys, region)? {
            report.maps += 1;
            add_placements(&mut placed, m, is_story);
        }
    }
    let w = place_read(placed, story, online, keys, &mut report)?;
    Ok((w, report))
}

/// The entities of a map read in a state, into the placements being collected: one per (map, index), live in every
/// state that read it.
fn add_placements(placed: &mut BTreeMap<(String, u32), Placed>, m: MapRead, is_story: bool) {
    for (i, e) in m.entities {
        // the LOD parent: by index in this map, or in the parent map when flags bit 3 says so
        let lod_parent = u32::try_from(e.parent_index).ok().and_then(|pi| {
            let map = if e.flags & 8 != 0 {
                m.parent.clone()?
            } else {
                m.path.clone()
            };
            Some((map, pi))
        });
        let entry = placed.entry((m.path.clone(), i)).or_insert_with(|| Placed {
            ymap: m.path.clone(),
            ipl: m.ipl.clone(),
            index: i,
            child: None,
            lod_parent,
            transform: entity_transform(&e),
            def: e,
            story: false,
            online: false,
        });
        if is_story {
            entry.story = true;
        } else {
            entry.online = true;
        }
    }
}

/// The placements collected, with each state's archetypes from its own map types files, into a new world.
fn place_read(
    placed: BTreeMap<(String, u32), Placed>,
    story: Lookup<'_>,
    online: Option<Lookup<'_>>,
    keys: &Keys,
    report: &mut RegionReport,
) -> Result<World> {
    let (story_types, story_duplicates) = archetypes(story.set, keys)?;
    let online_types = online.map(|s| archetypes(s.set, keys)).transpose()?;
    report.duplicate_archetypes = story_duplicates + online_types.as_ref().map_or(0, |(_, d)| *d);
    let online_tables = online
        .as_ref()
        .zip(online_types.as_ref())
        .map(|(set, (t, _))| (t, set));
    place(
        placed.into_values().collect(),
        (&story_types, &story),
        online_tables,
        report,
    )
}

/// One map file read whole into a new world as [`read_region`] reads a region: every entity of the file (no region,
/// no reach), placed with the definitions of the states the file is live in, each prototype's payload resolved through
/// that state's lookup, and the script-managed map that governs it (`gta.ipl.<name>`). A file live in both states is
/// placed as a placement live in both is (`gta.map` conditions where the states define it differently); a file only
/// GTA Online has live is placed in Online; a file neither has live (an older copy of a name) is read as Story's. The
/// geometry and textures come with [`crate::read::assets::attach_assets_with`].
pub fn read_map_file(
    story: Lookup<'_>,
    online: Option<Lookup<'_>>,
    keys: &Keys,
    file: FileRef,
) -> Result<(World, RegionReport)> {
    let live = |l: Lookup<'_>, name: u32| l.set.tables.get("ymap").and_then(|t| t.get(&name)) == Some(&file);
    let head = header(story.set, keys, file)?;
    let live_story = live(story, head.name);
    let live_online = online.is_some_and(|o| live(o, head.name));
    let (in_story, in_online) = if live_story || !live_online {
        (true, live_online)
    } else {
        (false, true)
    };
    let set = if in_story {
        story.set
    } else {
        online.map_or(story.set, |o| o.set)
    };
    let mut chain = BTreeMap::new();
    chain.insert(
        head.name,
        (head.flags & 1 != 0, head.parent, map_name(path(set, file)?)),
    );
    let ipl = ipl_of(set, keys, &mut chain, &head);
    let map = MapData::from_root(&decode(set, keys, file)?)?;
    let m = MapRead {
        path: path(set, file)?.to_owned(),
        parent: parent_path(set, map.parent)?,
        ipl,
        entities: (0..).zip(map.entities).collect(),
    };
    let mut placed = BTreeMap::new();
    for (read, is_story) in [(in_story, true), (in_online, false)] {
        if read {
            add_placements(&mut placed, m.clone(), is_story);
        }
    }
    let mut report = RegionReport {
        maps: 1,
        ..RegionReport::default()
    };
    // Online's tables only when the file is live there: a file of one state carries no state of its own
    let w = place_read(placed, story, online.filter(|_| in_online), keys, &mut report)?;
    Ok((w, report))
}

/// The placements read and their prototypes into a new world ([`read_region`]): a placement takes the definitions of
/// the states it is live in ([`definitions`]), each state's payload resolved through its lookup; a placement live in
/// both whose states define its archetype in different files or resolve different payloads is placed once per state.
/// A prototype is one (archetype, map types file, payload), with the states resolving it ([`GtaResolvedIn`]).
fn place<'a, 'l>(
    placed: Vec<Placed>,
    story_tables: (&'a ArchetypeTable, &'a Lookup<'l>),
    online_tables: Option<(&'a ArchetypeTable, &'a Lookup<'l>)>,
    report: &mut RegionReport,
) -> Result<World> {
    // each state's payload of an archetype, resolved once
    let mut payloads: BTreeMap<(bool, u32), Payload> = BTreeMap::new();
    let mut payload = |is_online: bool, l: &Lookup<'l>, d: &ArchetypeDef| {
        (payloads.entry((is_online, d.name)))
            .or_insert_with(|| l.payload(d.into()))
            .clone()
    };
    let mut w = World::new();
    let axis = online_tables.map(|_| {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: "gta.map".into(),
                domain: Domain::Enum(vec!["story".into(), "online".into()]),
                default: Some(0),
                scope: Scope::Global,
                class: "map".into(),
                source: "the packs' GROUP_MAP change sets, run on entering GTA Online".into(),
            },
        );
        e
    });
    // every entity, and the entities placed in interiors
    let mut all: Vec<Placed> = Vec::new();
    for p in placed {
        let (e, ymap, ipl, index, in_story, in_online) = (
            p.def.clone(),
            p.ymap.clone(),
            p.ipl.clone(),
            p.index,
            p.story,
            p.online,
        );
        all.push(p);
        if e.kind != EntityKind::Interior {
            continue;
        }
        report.interiors += 1;
        // the instance's rotation and position, without its scale
        let frame = entity_transform(&EntityDef {
            scale_xy: 1.0,
            scale_z: 1.0,
            ..e.clone()
        });
        // its entities by each applicable state's interior definition, live in the states that definition holds for
        for d in definitions(
            e.archetype,
            in_story,
            in_online,
            story_tables,
            online_tables,
            &mut payload,
        ) {
            let Some(interior) = d.def.interior.as_ref() else {
                continue;
            };
            report.interior_entity_sets += interior.entity_sets;
            for (ci, ce) in interior.entities.iter().enumerate() {
                let t = frame.compose(&entity_transform(ce));
                all.push(Placed {
                    ymap: ymap.clone(),
                    ipl: ipl.clone(),
                    index,
                    child: Some(ci as u32),
                    lod_parent: None,
                    def: ce.clone(),
                    transform: t,
                    story: d.story,
                    online: d.online,
                });
            }
        }
    }
    let mut prototypes = BTreeMap::new();
    // the game's hours: always an axis (time archetypes and lights condition on it; the selection resolves or keeps it)
    let hour_axis = hour_axis(&mut w);
    let mut ipl_axes: BTreeMap<String, primase_core::Entity> = BTreeMap::new();
    for Placed {
        ymap,
        ipl,
        index,
        child,
        lod_parent,
        def: e,
        transform,
        story: in_story,
        online: in_online,
    } in &all
    {
        let defs = definitions(
            e.archetype,
            *in_story,
            *in_online,
            story_tables,
            online_tables,
            &mut payload,
        );
        if defs.is_empty() {
            report.missing_archetypes += 1;
            continue;
        }
        if let [s, o] = &defs[..] {
            report.split_by_state += 1;
            report.split_by_payload += usize::from(s.ytyp == o.ytyp);
        }
        for d in defs {
            let (ytyp, a, set) = (d.ytyp, d.def, d.set.set);
            let (in_story, in_online) = (&d.story, &d.online);
            let key = (e.archetype, ytyp.clone(), d.payload);
            let proto = *prototypes.entry(key).or_insert_with(|| {
                let p = w.spawn();
                w.insert(p, GtaArchetype::new(ytyp, a));
                w.insert(
                    p,
                    GtaResolvedIn {
                        story: false,
                        online: false,
                    },
                );
                w.insert(p, Source(ytyp.clone()));
                let archetype = format!("archetype {:#010x}", a.name);
                w.insert(
                    p,
                    Provenance::new(crate::read::assets::origin(ytyp, Some(&archetype))),
                );
                // its asset's file name, in the archive set its archetype is read from: a drawable's or a fragment's; a
                // drawable dictionary's entry (stored by hash) or an asset with no live file keeps the archetype's hash
                let ext = match a.asset_type {
                    1 => Some("yft"),
                    2 => Some("ydr"),
                    _ => None,
                };
                let name = ext.and_then(|ext| {
                    let path = set.path(*set.tables.get(ext)?.get(&a.asset_name)?)?;
                    let file = path.rsplit('\\').next()?;
                    Some(file.strip_suffix(ext)?.strip_suffix('.')?.to_string())
                });
                if name.is_none() {
                    report.prototypes_named_by_hash += 1;
                }
                w.insert(p, Name(name.unwrap_or_else(|| format!("{:#010x}", a.name))));
                p
            });
            // the states whose lookups resolve this payload: one prototype serves every placement it is resolved for
            if let Some(r) = w.get_mut::<GtaResolvedIn>(proto) {
                r.story |= *in_story;
                r.online |= *in_online;
            }
            let p = w.spawn();
            w.insert(p, GTA.transform(transform));
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            match child {
                None => {
                    w.insert(p, GtaEntity::new(ymap, *index, None, lod_parent.clone(), e));
                    w.insert(p, Source(format!("{ymap}#{index}")));
                    let item = format!("entity {index}");
                    w.insert(p, Provenance::new(crate::read::assets::origin(ymap, Some(&item))));
                }
                Some(c) => {
                    w.insert(p, GtaEntity::new(ymap, *c, Some(*index), None, e));
                    w.insert(p, Source(format!("{ymap}#{index}/{c}")));
                    let item = format!("entity {index} child {c}");
                    w.insert(p, Provenance::new(crate::read::assets::origin(ymap, Some(&item))));
                    report.interior_entities += 1;
                }
            }
            let state = match (in_story, in_online) {
                (true, true) => "both",
                (true, false) => "story",
                (false, true) => "online",
                (false, false) => unreachable!(),
            };
            *report.by_state.entry(state).or_default() += 1;
            let mut literals = Vec::new();
            if let (Some(axis), false) = (axis, state == "both") {
                let value = if state == "story" { 0 } else { 1 };
                literals.push(Literal {
                    axis,
                    values: 1u64 << value,
                });
            }
            // a time archetype is shown in the hours its timeFlags' low 24 bits name (none: never)
            if let Some(flags) = a.time_flags {
                let axis = hour_axis;
                let hours = u64::from(flags & 0x00FF_FFFF);
                literals.push(Literal { axis, values: hours });
                report.time_placements += 1;
                report.time_placements_never_shown += usize::from(hours == 0);
            }
            // a script-managed map's placements exist while its IPL is loaded
            if let Some(name) = ipl {
                let axis = *ipl_axes.entry(name.clone()).or_insert_with(|| {
                    report.ipl_axes += 1;
                    let e = w.spawn();
                    w.insert(
                        e,
                        StateAxis {
                            key: format!("gta.ipl.{name}"),
                            domain: Domain::Bool,
                            default: None,
                            scope: Scope::Global,
                            class: "ipl".into(),
                            source: format!(
                                "script-managed map {name} (CMapData flags bit 0); its start state is in the game's \
                                 scripts, not read: unknown"
                            ),
                        },
                    );
                    e
                });
                literals.push(Literal { axis, values: 0b10 });
                report.ipl_placements += 1;
            }
            if let Some(c) = condition(literals) {
                w.insert(p, c);
            }
            report.placements += 1;
        }
    }
    report.prototypes = prototypes.len();
    if report.placements == 0 && !all.is_empty() {
        return Err(invalid("region", 0, "no entity had an archetype"));
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archetype(name: u32, asset_name: u32) -> ArchetypeDef {
        ArchetypeDef {
            kind: rage_engine_formats::map::ArchetypeKind::Base,
            name,
            lod_dist: 100.0,
            flags: 0,
            special_attribute: 0,
            bb_min: [0.0; 3],
            bb_max: [1.0; 3],
            bs_centre: [0.5; 3],
            bs_radius: 1.0,
            hd_texture_dist: 0.0,
            texture_dictionary: 0,
            clip_dictionary: 0,
            drawable_dictionary: 0,
            physics_dictionary: 0,
            asset_type: 2,
            asset_name,
            time_flags: None,
            extensions: 0,
            interior: None,
        }
    }

    /// Each state's own definition (review B6): archetype 1 is defined by the same file in both states, 2 by a
    /// different file in each (another asset), 3 by Story only and 4 by Online only. A placement live in Story takes
    /// Story's definition, one live in both is placed once when the file is the same and once per state when it
    /// differs, and a definition only the other state has is none.
    #[test]
    fn a_placement_takes_the_definition_of_the_states_it_is_live_in() {
        let story: ArchetypeTable = BTreeMap::from([
            (1, ("common/a.ytyp".to_owned(), archetype(1, 10))),
            (2, ("story/b.ytyp".to_owned(), archetype(2, 20))),
            (3, ("story/c.ytyp".to_owned(), archetype(3, 30))),
        ]);
        let online: ArchetypeTable = BTreeMap::from([
            (1, ("common/a.ytyp".to_owned(), archetype(1, 10))),
            (2, ("online/b.ytyp".to_owned(), archetype(2, 21))),
            (4, ("online/d.ytyp".to_owned(), archetype(4, 40))),
        ]);
        let sets = ("story set", "online set");
        let tables = ((&story, &sets.0), Some((&online, &sets.1)));
        let seen = |h, in_story, in_online| -> Vec<(&str, u32, &str, bool, bool)> {
            definitions(h, in_story, in_online, tables.0, tables.1, &mut |_, _, _| ())
                .into_iter()
                .map(|d| (d.ytyp.as_str(), d.def.asset_name, *d.set, d.story, d.online))
                .collect()
        };
        // the same file in both states: one placement for both
        assert_eq!(
            seen(1, true, true),
            [("common/a.ytyp", 10, "online set", true, true)]
        );
        // different files: a Story placement takes Story's definition, a placement in both is split
        assert_eq!(
            seen(2, true, false),
            [("story/b.ytyp", 20, "story set", true, false)]
        );
        assert_eq!(
            seen(2, true, true),
            [
                ("story/b.ytyp", 20, "story set", true, false),
                ("online/b.ytyp", 21, "online set", false, true)
            ]
        );
        // a Story-only definition is kept for a Story placement; an Online-only one isn't Story's
        assert_eq!(
            seen(3, true, false),
            [("story/c.ytyp", 30, "story set", true, false)]
        );
        assert!(seen(4, true, false).is_empty());
        assert_eq!(
            seen(4, true, true),
            [("online/d.ytyp", 40, "online set", false, true)]
        );
        // no Online read: Story's table alone
        assert_eq!(
            definitions(
                2,
                true,
                false,
                tables.0,
                None::<(&ArchetypeTable, &&str)>,
                &mut |_, _, _| (),
            )
            .into_iter()
            .map(|d| d.def.asset_name)
            .collect::<Vec<_>>(),
            [20]
        );
    }

    fn entity(archetype: u32) -> EntityDef {
        EntityDef {
            kind: EntityKind::Entity,
            archetype,
            flags: 0,
            guid: 0,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale_xy: 1.0,
            scale_z: 1.0,
            parent_index: -1,
            lod_dist: 100.0,
            child_lod_dist: 0.0,
            lod_level: 0,
            num_children: 0,
            priority_level: 0,
            ambient_occlusion_multiplier: 255,
            artificial_ambient_occlusion: 255,
            tint_value: 0,
            extensions: 0,
        }
    }

    /// Review R-B6, a synthetic two-map install: one map types file, shared by both states, defines archetypes H and
    /// K. Story resolves H's drawable D to `story\d.ydr`, GTA Online to `online\d.ydr`; both resolve K's drawable and
    /// texture dictionary to the same files. A placement of each, live in both states: H is placed once per state, each
    /// placement with its state's `gta.map` condition and a prototype resolved in that state alone, whose drawable is
    /// loaded from that state's file; K stays one prototype for both. When Online's texture parents give K's dictionary
    /// a parent Story's don't, K's payload differs too and it is split the same way.
    #[test]
    fn each_state_s_resolved_payload_is_a_prototype_of_its_own() {
        use crate::read::assets::Lookups;
        use rage_engine_formats::gtxd::TextureParents;
        const H: u32 = 1;
        const K: u32 = 2;
        const D: u32 = 10;
        const E: u32 = 20;
        const T: u32 = 30;
        const P: u32 = 40;
        let paths = [
            "x\\story\\d.ydr",
            "x\\online\\d.ydr",
            "x\\common\\e.ydr",
            "x\\common\\t.ytd",
            "x\\online\\p.ytd",
        ];
        let story_set = ArchiveSet::synthetic(&paths, &[("ydr", D, 0), ("ydr", E, 2), ("ytd", T, 3)]);
        let online_set = ArchiveSet::synthetic(
            &paths,
            &[("ydr", D, 1), ("ydr", E, 2), ("ytd", T, 3), ("ytd", P, 4)],
        );
        let no_parents = TextureParents::default();
        let online_parents = TextureParents {
            parents: BTreeMap::from([(T, P)]),
            ..TextureParents::default()
        };
        let story = Lookup {
            set: &story_set,
            parents: &no_parents,
        };
        let shared = |h, asset| {
            let def = ArchetypeDef {
                texture_dictionary: T,
                ..archetype(h, asset)
            };
            ("common\\shared.ytyp".to_owned(), def)
        };
        let table: ArchetypeTable = BTreeMap::from([(H, shared(H, D)), (K, shared(K, E))]);
        let placed = |archetype, index| Placed {
            ymap: "x\\common\\m.ymap".into(),
            ipl: None,
            index,
            child: None,
            lod_parent: None,
            def: entity(archetype),
            transform: Transform::IDENTITY,
            story: true,
            online: true,
        };
        // per prototype: its archetype, the states it is resolved in, the drawable file it is loaded from, and the
        // gta.map values of its placements' conditions (none: no condition)
        type Seen = (u32, (bool, bool), String, Vec<Option<u64>>);
        let read = |online: Lookup<'_>| {
            let mut report = RegionReport::default();
            let w = place(
                vec![placed(H, 0), placed(K, 1)],
                (&table, &story),
                Some((&table, &online)),
                &mut report,
            )
            .unwrap();
            let lookups = Lookups::new(story, Some(online)).unwrap();
            let mut protos: Vec<Seen> = w
                .iter::<GtaArchetype>()
                .map(|(p, a)| {
                    let r = w.get::<GtaResolvedIn>(p).expect("resolved in");
                    let l = lookups.of(&w, p).unwrap();
                    let (_, f) = l.drawable(a.into()).expect("a live drawable");
                    let conditions = (w.iter::<Instance>())
                        .filter(|(_, i)| i.prototype == p)
                        .map(|(e, _)| w.get::<Condition>(e).map(|c| c.0[0].0[0].values))
                        .collect();
                    (
                        a.name,
                        (r.story, r.online),
                        path(l.set, f).expect("a file of the install").to_owned(),
                        conditions,
                    )
                })
                .collect();
            protos.sort();
            // an Online-only prototype needs Online's set to load
            let online_only = (w.iter::<GtaResolvedIn>())
                .find(|(_, r)| !r.story && r.online)
                .map(|(p, _)| p)
                .expect("an Online prototype");
            assert!(Lookups::one(story).of(&w, online_only).is_err());
            (protos, report)
        };

        let (protos, report) = read(Lookup {
            set: &online_set,
            parents: &no_parents,
        });
        assert_eq!(
            protos,
            [
                (H, (false, true), "x\\online\\d.ydr".into(), vec![Some(0b10)]),
                (H, (true, false), "x\\story\\d.ydr".into(), vec![Some(0b01)]),
                (K, (true, true), "x\\common\\e.ydr".into(), vec![None]),
            ]
        );
        assert_eq!((report.placements, report.prototypes), (3, 3), "{report:?}");
        assert_eq!((report.split_by_state, report.split_by_payload), (1, 1));

        // Online's gtxd parents K's dictionary T under P: its texture lookup differs, so K is split as well
        let (protos, report) = read(Lookup {
            set: &online_set,
            parents: &online_parents,
        });
        assert_eq!(
            protos.iter().map(|(h, r, _, _)| (*h, *r)).collect::<Vec<_>>(),
            [
                (H, (false, true)),
                (H, (true, false)),
                (K, (false, true)),
                (K, (true, false))
            ]
        );
        assert_eq!((report.placements, report.split_by_payload), (4, 2), "{report:?}");
    }

    #[test]
    fn map_names_are_file_stems() {
        assert_eq!(
            map_name("x\\dlc.rpf\\hei_showroom_closed.ymap"),
            "hei_showroom_closed"
        );
    }

    /// A placement on GTA Online's map shown at noon stays in Online at 12 and goes in story mode or at 13; one with
    /// an empty hour mask goes in every state; the world with them verifies.
    #[test]
    fn a_condition_keeps_its_placement_in_its_map_and_hours_only() {
        use primase_core::pass::{Pass, PassLog};
        for (assign, kept) in [
            ("gta.map:online;gta.hour:12", true),
            ("gta.map:story;gta.hour:12", false),
            ("gta.map:online;gta.hour:13", false),
        ] {
            let mut w = World::new();
            let (map, hour) = (w.spawn(), w.spawn());
            for (e, key, values) in [
                (map, "gta.map", vec!["story".to_string(), "online".to_string()]),
                (hour, "gta.hour", (0..24).map(|h| h.to_string()).collect()),
            ] {
                w.insert(
                    e,
                    StateAxis {
                        key: key.into(),
                        domain: Domain::Enum(values),
                        default: Some(0),
                        scope: Scope::Global,
                        class: "test".into(),
                        source: "test".into(),
                    },
                );
            }
            let at_noon_online = condition(vec![
                Literal {
                    axis: map,
                    values: 0b10,
                },
                Literal {
                    axis: hour,
                    values: 1 << 12,
                },
            ])
            .expect("a condition");
            let never = condition(vec![Literal {
                axis: hour,
                values: 0,
            }])
            .expect("a condition");
            let (p, q) = (w.spawn(), w.spawn());
            for (e, c) in [(p, at_noon_online), (q, never)] {
                w.insert(e, Transform::IDENTITY);
                w.insert(e, c);
            }
            assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
            let values = assign
                .split(';')
                .map(|kv| {
                    kv.split_once(':')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .unwrap()
                })
                .collect();
            primase_core::state::ResolveState {
                assign: values,
                ..Default::default()
            }
            .run(&mut w, &mut PassLog::default())
            .unwrap();
            assert_eq!((w.is_alive(p), w.is_alive(q)), (kept, false), "{assign}");
        }
    }
}
