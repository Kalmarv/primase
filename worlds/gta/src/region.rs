//! A region of the map into the world: every entity of the live maps whose pivot lies within the region plus a reach,
//! as a placement of its archetype's prototype, in the world frame.
//!
//! - Map state: the story-mode map is read; given GTA Online's archive set too ([`archive_sets`], [`States::All`]), and an entity that is live in one
//!   only carries a `core.condition` on the global axis `gta.map` {story, online} (default story, the game's start
//!   state). Entities are the same in both when their map file is.
//! - Time archetypes are shown in some hours only: their placements carry a `core.condition` on the global axis
//!   `gta.hour` {0..23} (default 12) with the hours their timeFlags name; `resolve-state` picks the hour.
//! - Script-managed maps (`CMapData` flags bit 0: the game streams them only when a script requests them, an IPL):
//!   each is a global bool axis `gta.ipl.<map name>` (class `ipl`), and the placements of that map, and of maps whose
//!   LOD parent chain reaches it, carry a condition on it. The default (whether the game's start scripts load it) is
//!   [`crate::ipl::ipl_default`].
//! - Archetypes come from every live map types file; a name defined in more than one is counted and the first kept.
//! - Transforms: GTA's frame is east-north-up ([`GTA`]); an entity's orientation is its stored quaternion inverted
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

use primase_core::frame::SourceFrame;
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Instance, Name, Source, Transform, World};

use crate::archive_set::{ArchiveSet, FileRef, Install, MapState};
use crate::components::{GtaArchetype, GtaEntity};
use crate::keys::Keys;
use crate::map::{ArchetypeDef, EntityDef, EntityKind, MapData, MapTypes};
use crate::meta::Meta;
use crate::{Result, invalid};

/// GTA V's frame: x east, y north, z up, metres.
pub const GTA: SourceFrame = SourceFrame::ENU;

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
    /// Placements by condition: `both`, `story`, `online`.
    pub by_state: BTreeMap<&'static str, usize>,
    /// Archetype names defined by more than one live map types file.
    pub duplicate_archetypes: usize,
    /// Entities whose archetype no live map types file defines.
    pub missing_archetypes: usize,
    /// Placements of time archetypes (each with an hour mask on the `gta.hour` axis).
    pub time_placements: usize,
    /// Of those, the ones whose mask names no hour: kept, with the condition that never holds.
    pub time_placements_never_shown: usize,
    /// Script-managed maps with placements here (each a `gta.ipl.<name>` axis), those on by default, and the
    /// placements they govern.
    pub ipl_axes: usize,
    pub ipl_axes_on: usize,
    pub ipl_placements: usize,
    /// Interior instances read, the entities placed in them, and their entity sets (not read yet).
    pub interiors: usize,
    pub interior_entities: usize,
    pub interior_entity_sets: usize,
}

/// One live map's entities near the region.
struct MapRead {
    path: String,
    /// The live LOD parent map's path, when it has one.
    parent: Option<String>,
    /// The script-managed map that governs it: itself, or the nearest of its LOD parent chain that is.
    ipl: Option<String>,
    entities: Vec<(u32, EntityDef)>,
}

/// A map's name: its file name without the extension.
fn map_name(path: &str) -> String {
    let file = path.rsplit('\\').next().unwrap_or(path);
    file.strip_suffix(".ymap").unwrap_or(file).to_string()
}

fn decode(set: &ArchiveSet, keys: &Keys, f: FileRef) -> Result<crate::meta::Struct> {
    let a = &set.install.archives[f.archive];
    let r = a.resource(&mut File::open(&a.file)?, f.entry, keys)?;
    Meta::read(&r)?.root()
}

/// The live maps' entities whose pivot lies within the region and its reach.
fn region_maps(set: &ArchiveSet, keys: &Keys, region: &Region) -> Result<Vec<MapRead>> {
    let mut out = Vec::new();
    // every live map's name hash: whether it is script-managed, and its LOD parent's
    let mut chain: BTreeMap<u32, (bool, u32, String)> = BTreeMap::new();
    for &f in set.tables.get("ymap").into_iter().flat_map(|t| t.values()) {
        let map = MapData::from_root(&decode(set, keys, f)?)?;
        chain.insert(map.name, (map.flags & 1 != 0, map.parent, map_name(set.path(f))));
        let ipl = {
            let (mut at, mut found, mut guard) = (map.name, None, 0);
            while let Some((scripted, parent, name)) = chain.get(&at).cloned().or_else(|| {
                // a parent not seen yet: read its header from its file
                let &pf = set.tables.get("ymap")?.get(&at)?;
                let pm = MapData::from_root(&decode(set, keys, pf).ok()?).ok()?;
                let e = (pm.flags & 1 != 0, pm.parent, map_name(set.path(pf)));
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
        };
        let [lo, hi] = map.entities_extents;
        if map.entities.is_empty() || !region.touches(lo, hi) {
            continue;
        }
        let entities: Vec<(u32, EntityDef)> = map
            .entities
            .into_iter()
            .enumerate()
            .filter(|(_, e)| region.holds_pivot(e.position))
            .map(|(i, e)| (i as u32, e))
            .collect();
        if !entities.is_empty() {
            out.push(MapRead {
                path: set.path(f).to_string(),
                parent: (map.parent != 0)
                    .then(|| {
                        set.tables
                            .get("ymap")
                            .and_then(|t| t.get(&map.parent))
                            .map(|&p| set.path(p).to_string())
                    })
                    .flatten(),
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

fn archetypes(set: &ArchiveSet, keys: &Keys) -> Result<(ArchetypeTable, usize)> {
    let mut out = ArchetypeTable::new();
    let mut duplicates = 0;
    for &f in set.tables.get("ytyp").into_iter().flat_map(|t| t.values()) {
        let types = MapTypes::from_root(&decode(set, keys, f)?)?;
        for a in types.archetypes {
            match out.entry(a.name) {
                std::collections::btree_map::Entry::Occupied(_) => duplicates += 1,
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert((set.path(f).to_string(), a));
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

/// `a` then `b`: `a * b`, in f64, rounded once.
fn compose(a: &Transform, b: &Transform) -> Transform {
    let lin = |m: &Transform, v: [f32; 3]| -> [f64; 3] {
        std::array::from_fn(|i| (0..3).map(|j| f64::from(m.cols[j][i]) * f64::from(v[j])).sum())
    };
    let mut cols = [[0f32; 3]; 4];
    for (c, col) in cols.iter_mut().take(3).enumerate() {
        *col = lin(a, b.cols[c]).map(|v| v as f32);
    }
    let t = lin(a, b.cols[3]);
    cols[3] = std::array::from_fn(|i| (t[i] + f64::from(a.cols[3][i])) as f32);
    Transform { cols }
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

/// Reads the region into a new world: placements and their prototypes (geometry and textures come with
/// [`crate::assets::attach_assets`]). `online`, when given, adds GTA Online's map and the `gta.map` conditions.
pub fn read_region(
    story: &ArchiveSet,
    online: Option<&ArchiveSet>,
    keys: &Keys,
    region: &Region,
) -> Result<(World, RegionReport)> {
    let mut report = RegionReport::default();
    // by (map path, entity index): the entity, its LOD parent, the states it is live in
    let mut placed: BTreeMap<(String, u32), Placed> = BTreeMap::new();
    for (set, is_story) in std::iter::once((story, true)).chain(online.map(|s| (s, false))) {
        for m in region_maps(set, keys, region)? {
            report.maps += 1;
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
    }
    let (types, duplicates) = archetypes(online.unwrap_or(story), keys)?;
    report.duplicate_archetypes = duplicates;

    let mut w = World::new();
    let axis = online.map(|_| {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: "gta.map".into(),
                domain: Domain::Enum(vec!["story".into(), "online".into()]),
                default: 0,
                scope: Scope::Global,
                class: "map".into(),
                source: "the packs' GROUP_MAP change sets, run on entering GTA Online".into(),
            },
        );
        e
    });
    // every entity, and the entities placed in interiors
    let mut all: Vec<Placed> = Vec::new();
    for p in placed.into_values() {
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
        let Some(interior) = types.get(&e.archetype).and_then(|(_, a)| a.interior.as_ref()) else {
            continue;
        };
        report.interior_entity_sets += interior.entity_sets;
        // the instance's rotation and position, without its scale
        let frame = entity_transform(&EntityDef {
            scale_xy: 1.0,
            scale_z: 1.0,
            ..e.clone()
        });
        for (ci, ce) in interior.entities.iter().enumerate() {
            let t = compose(&frame, &entity_transform(ce));
            all.push(Placed {
                ymap: ymap.clone(),
                ipl: ipl.clone(),
                index,
                child: Some(ci as u32),
                lod_parent: None,
                def: ce.clone(),
                transform: t,
                story: in_story,
                online: in_online,
            });
        }
    }
    let mut prototypes = BTreeMap::new();
    let mut hour_axis: Option<primase_core::Entity> = None;
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
        let Some((ytyp, a)) = types.get(&e.archetype) else {
            report.missing_archetypes += 1;
            continue;
        };
        let proto = *prototypes.entry(e.archetype).or_insert_with(|| {
            let p = w.spawn();
            w.insert(p, GtaArchetype::new(ytyp, a));
            w.insert(p, Source(ytyp.clone()));
            let name = story
                .tables
                .get("ydr")
                .and_then(|t| t.get(&a.asset_name))
                .map(|&f| story.path(f))
                .and_then(|path| path.rsplit('\\').next())
                .and_then(|n| n.strip_suffix(".ydr"))
                .map_or_else(|| format!("{:#010x}", a.name), str::to_string);
            w.insert(p, Name(name));
            p
        });
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
            }
            Some(c) => {
                w.insert(p, GtaEntity::new(ymap, *c, Some(*index), None, e));
                w.insert(p, Source(format!("{ymap}#{index}/{c}")));
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
            let axis = *hour_axis.get_or_insert_with(|| {
                let e = w.spawn();
                w.insert(
                    e,
                    StateAxis {
                        key: "gta.hour".into(),
                        domain: Domain::Enum((0..24).map(|h| h.to_string()).collect()),
                        default: 12,
                        scope: Scope::Global,
                        class: "time".into(),
                        source: "time archetypes' timeFlags (CTimeArchetypeDef): the hours each is shown"
                            .into(),
                    },
                );
                e
            });
            let hours = u64::from(flags & 0x00FF_FFFF);
            literals.push(Literal { axis, values: hours });
            report.time_placements += 1;
            report.time_placements_never_shown += usize::from(hours == 0);
        }
        // a script-managed map's placements exist while its IPL is loaded
        if let Some(name) = ipl {
            let axis = *ipl_axes.entry(name.clone()).or_insert_with(|| {
                let (on, why) = crate::ipl::ipl_default(name);
                report.ipl_axes += 1;
                report.ipl_axes_on += usize::from(on);
                let e = w.spawn();
                w.insert(
                    e,
                    StateAxis {
                        key: format!("gta.ipl.{name}"),
                        domain: Domain::Bool,
                        default: u32::from(on),
                        scope: Scope::Global,
                        class: "ipl".into(),
                        source: format!("script-managed map {name} (CMapData flags bit 0); default: {why}"),
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
    report.prototypes = prototypes.len();
    if report.placements == 0 && !all.is_empty() {
        return Err(invalid("region", 0, "no entity had an archetype"));
    }
    Ok((w, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_names_are_file_stems() {
        assert_eq!(
            map_name("x\\dlc.rpf\\hei_showroom_closed.ymap"),
            "hei_showroom_closed"
        );
    }

    #[test]
    fn an_empty_hour_mask_never_holds_and_the_world_verifies() {
        let mut w = World::new();
        let (map, hour) = (w.spawn(), w.spawn());
        for (e, key, n) in [(map, "gta.map", 2), (hour, "gta.hour", 24)] {
            w.insert(
                e,
                StateAxis {
                    key: key.into(),
                    domain: Domain::Enum((0..n).map(|v: i32| v.to_string()).collect()),
                    default: 0,
                    scope: Scope::Global,
                    class: "test".into(),
                    source: "test".into(),
                },
            );
        }
        let online = Literal { axis: map, values: 2 };
        assert_eq!(condition(Vec::new()), None);
        assert_eq!(
            condition(vec![
                online,
                Literal {
                    axis: hour,
                    values: 1 << 12
                }
            ]),
            Some(Condition(vec![Clause(vec![
                online,
                Literal {
                    axis: hour,
                    values: 1 << 12
                }
            ])]))
        );
        let never = condition(vec![
            online,
            Literal {
                axis: hour,
                values: 0,
            },
        ])
        .expect("a condition");
        assert_eq!(never, Condition(Vec::new()));
        let p = w.spawn();
        w.insert(p, Transform::IDENTITY);
        w.insert(p, never);
        assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    }
}
