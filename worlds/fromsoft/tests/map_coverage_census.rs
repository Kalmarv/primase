//! A coverage audit of FromSoftware maps, for three Sekiro areas and three Elden Ring areas (an open-world tile, a legacy
//! dungeon, a catacomb): what the install stores for the area, and for each kind of it whether a primase reader keeps it.
//! It prints; it asserts only that the reads that start succeed. Statuses:
//!
//! - `read`: a reader keeps it and a pass or the lab shows it;
//! - `read-header`: the header and shape are read, the type's own data is kept as bytes and not decoded;
//! - `dropped`: the MSB reader reads it and nothing puts it in the world (without `Options::map_records`);
//! - `unread`: no reader opens it.
//!
//! The lines (a game is `Sekiro` or `Elden Ring`; `area` the map name):
//!
//! - `AREA <game> <area> <label>`: which area stands for which label; `NO-AREA` when the install lacks every candidate;
//! - `DICTIONARY <game> entries <n> unnamed <m>`: archive entries, and those the name dictionary does not name. An area's
//!   file that is not in the dictionary is invisible to the `FILES` lines: `unnamed` says how many such entries there are;
//! - `FILES <game> <area> <pattern> files <n> bytes <b> status <s> owner <o>`: every archive path the dictionary names that
//!   holds the area's number (or, for draw-param files, its block's), by pattern (the area's number written `<area>`, every
//!   other digit `N`), with the sum of their stored sizes. This is the list of files the area has; a pattern's status says
//!   whether the area read opens it. It decides which unread pattern is next (the largest first);
//! - `MEMBERS <game> <area> <pattern> ext <e> members <n> in <k> files`: the members of the binders among those files
//!   (navmesh, light-probe, texture and collision binders; at most three files per pattern, five for map pieces), by
//!   extension. It decides what is inside an unread binder before a reader is written for it. `MEMBERERR` names a binder
//!   that did not read;
//! - `MODELS <game> <area> kind <k> code <c> n <n>`: the MSB's model table;
//! - `PARTS <game> <area> kind <k> msb <n> status <s> world <m>`: parts by kind, and how many of them the world holds
//!   (placements, collision files, or markers of `fromsoft.unplaced_part` after `lower-fromsoft-map-records`);
//! - `REGIONS <game> <area> type <t> code <c> msb <n> shapes [..] data_lengths [..] entity_ids <k> status <s> world <m>`: every
//!   region type with its shapes and the lengths of its type data. Equal lengths within a type mean a fixed record, which a
//!   decoder can be written for; `example` gives the first non-empty data as hexadecimal. It decides which type's data is
//!   decoded next;
//! - `EVENTS <game> <area> type <t> code <c> msb <n> data_lengths [..] parts <p> regions <r> status <s> world <m>`: the same
//!   for events, with how many name a part and a region;
//! - `WORLD <game> <area> component <name> entities <n>` and `RESOURCE <game> <area> <name>`: everything the read puts
//!   in the world (only the records are read here: the area's models, textures and collision are other censuses'). What the
//!   MSB counts say is stored and this says is kept are compared line by line above.
//!
//! The area read itself is not run: it needs every model of the area, and the censuses of `sekiro_scene` and
//! `elden_area` account for them. The records are read through the same function the read calls
//! (`sekiro::read_map_records`) and lowered by the same pass the lab runs.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
use primase_core::World;
use primase_fromsoft::msb::{Event, MapLayout, Region};
use primase_fromsoft::sekiro::{ExtrasReport, read_map_records};
use primase_fromsoft::{Archive, Binder, Game, Install, Keys, NameDictionary, dcx::Decoder};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The areas standing for each label, in the order they are tried: the first the install holds is the one used.
fn candidates(game: Game) -> [(&'static str, &'static [&'static str]); 3] {
    match game {
        Game::Sekiro => [
            ("castle", &["m11_01_00_00"]),
            ("estate", &["m20_00_00_00"]),
            (
                "outskirts",
                &["m10_00_00_00", "m11_00_00_00", "m24_00_00_00", "m25_00_00_00"],
            ),
        ],
        Game::EldenRing => [
            ("m60 tile", &["m60_53_37_00", "m60_42_36_00"]),
            ("legacy dungeon", &["m14_00_00_00", "m10_00_00_00"]),
            (
                "catacomb",
                &["m30_00_00_00", "m30_01_00_00", "m30_02_00_00", "m30_03_00_00"],
            ),
        ],
    }
}

/// What the area read does with an archive path of the area, from the code (`read/area.rs`, `read/collision.rs`,
/// `read/extras.rs`, `read/draw_param.rs`, `read/states.rs`): the status and who owns the next step.
fn status(game: Game, path: &str) -> (&'static str, &'static str) {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let ends = |suffix: &str| leaf.ends_with(suffix);
    if ends(".msb.dcx") {
        ("read", "primase msb.rs; lab parts, markers")
    } else if ends(".mapbnd.dcx") {
        ("read", "primase read/area.rs; lab Objects")
    } else if ends(".hkxbhd") || ends(".hkxbdt") {
        match leaf.chars().next() {
            Some('h') => ("read", "primase read/collision.rs; lab Collision"),
            Some('l') => (
                "unread",
                "primase: low-resolution map collision, no option selects it; lab none",
            ),
            _ => ("unread", "primase: no reader; lab none"),
        }
    } else if ends("_0000.btl.dcx") {
        ("read", "primase read/extras.rs bank 0; lab Lights")
    } else if ends(".btl.dcx") {
        ("unread", "primase: only bank 0 is read; lab none")
    } else if ends(".edgebhd") || ends(".edgebdt") {
        ("read", "primase read/extras.rs; lab Curves")
    } else if ends(".tpfbhd") || ends(".tpfbdt") {
        match game {
            Game::Sekiro => ("read", "primase read/textures.rs catalog; lab Textures"),
            Game::EldenRing => (
                "unread",
                "primase: Elden Ring finds textures by material, not here; lab none",
            ),
        }
    } else if ends(".emevd.dcx") {
        match game {
            Game::Sekiro => ("read", "primase read/states.rs with states=event; lab State axes"),
            Game::EldenRing => ("unread", "primase: Elden Ring's states are refused; lab none"),
        }
    } else if path.starts_with("/param/drawparam/") {
        (
            "read",
            "primase read/draw_param.rs when the caller names the file; lab Properties",
        )
    } else {
        ("unread", "primase: no reader; lab none")
    }
}

/// A path as a pattern: the area's number written `<area>`, every other digit `N`.
fn pattern(path: &str, number: &str) -> String {
    path.replace(number, "<area>")
        .chars()
        .map(|c| if c.is_ascii_digit() { 'N' } else { c })
        .collect()
}

fn extension(name: &str) -> String {
    let leaf = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    let leaf = leaf.strip_suffix(".dcx").unwrap_or(&leaf);
    leaf.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_owned())
}

/// What the files of one group are: the path pattern, the status of its reader and who owns it.
type FileGroup = (String, &'static str, &'static str);

/// How many files a group holds, their stored bytes, and their paths.
#[derive(Default)]
struct FileTotals {
    count: usize,
    bytes: u64,
    paths: Vec<String>,
}

/// The dictionary's paths of an area that the archive holds, grouped by pattern, status and owner.
fn files(
    game: Game,
    archive: &Archive,
    names: &NameDictionary,
    area: &str,
) -> BTreeMap<FileGroup, FileTotals> {
    let number = &area[1..];
    let block = format!("/{}_", &area[..6]);
    let mut out: BTreeMap<FileGroup, FileTotals> = BTreeMap::new();
    for (hash, paths) in &names.names {
        let Some(entry) = archive.entries.get(hash) else {
            continue;
        };
        for path in paths {
            let drawparam = path.starts_with("/param/drawparam/") && path.contains(&block);
            if !path.contains(number) && !drawparam {
                continue;
            }
            let (status, owner) = status(game, path);
            let slot = out.entry((pattern(path, number), status, owner)).or_default();
            slot.count += 1;
            slot.bytes += entry.entry.padded_size as u64;
            slot.paths.push(path.clone());
        }
    }
    out
}

/// The members of one binder file by extension: a `.bhd` is read with its `.bdt`, anything else as a binder.
fn members(archive: &Archive, decoder: &Decoder<'_>, path: &str) -> Result<BTreeMap<String, usize>> {
    let binder = if let Some(stem) = path.strip_suffix("bhd") {
        Binder::read_split(
            &archive.read(path, decoder)?,
            &archive.read(&format!("{stem}bdt"), decoder)?,
            decoder,
        )?
    } else {
        Binder::read(&archive.read(path, decoder)?, decoder)?
    };
    let mut out = BTreeMap::new();
    for f in &binder.files {
        *out.entry(extension(f.name.as_deref().unwrap_or(""))).or_default() += 1;
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn area_census(
    game: Game,
    archive: &Archive,
    decoder: &Decoder<'_>,
    names: Option<&NameDictionary>,
    area: &str,
    label: &str,
) -> Result<()> {
    let name = game.name();
    println!("AREA {name} {area} {label}");
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;

    // the files the area has
    if let Some(names) = names {
        let listed = files(game, archive, names, area);
        for ((pattern, status, owner), FileTotals { count, bytes, paths }) in &listed {
            println!(
                "FILES {name} {area} {pattern} files {count} bytes {bytes} status {status} owner {owner}"
            );
            let binder = ["bnd.dcx", "bnd", "bhd"].iter().any(|s| pattern.ends_with(s));
            if !binder {
                continue;
            }
            let sample = if pattern.ends_with(".mapbnd.dcx") { 5 } else { 3 };
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            let mut taken = 0;
            for path in paths.iter().take(sample) {
                match members(archive, decoder, path) {
                    Ok(found) => {
                        taken += 1;
                        for (ext, n) in found {
                            *seen.entry(ext).or_default() += n;
                        }
                    }
                    Err(e) => println!("MEMBERERR {name} {area} {path} {e}"),
                }
            }
            for (ext, n) in seen {
                println!("MEMBERS {name} {area} {pattern} ext {ext:?} members {n} in {taken} files");
            }
        }
    }

    // the MSB: models, parts
    let mut models: BTreeMap<u32, usize> = BTreeMap::new();
    for m in &layout.models {
        *models.entry(m.kind).or_default() += 1;
    }
    for (code, n) in &models {
        println!("MODELS {name} {area} code {code} n {n}");
    }
    let mut parts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for p in &layout.parts {
        *parts.entry(p.kind.word()).or_default() += 1;
    }

    // what the records read puts in the world, lowered as the lab lowers it
    let mut world = World::default();
    let mut extras = ExtrasReport::default();
    read_map_records(&layout, None, &mut extras, &mut world)?;
    common::run(&mut world, "lower-fromsoft-map-records{leave-out=}")?;
    let word = |named: Option<&str>, code: u32| named.map_or_else(|| format!("type_{code}"), str::to_owned);
    let mut part_records: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, part) in world.iter::<primase_fromsoft::msb::UnplacedPart>() {
        *part_records.entry(part.kind).or_default() += 1;
    }
    for (kind, n) in &parts {
        let (status, in_world) = match *kind {
            "map_piece" | "object" | "asset" => ("read", "placements (the area read)".to_owned()),
            "collision" => ("read", "collision files (the area read)".to_owned()),
            _ => (
                "read (dropped before the world without Options::map_records)",
                format!("{} records", part_records.get(kind).copied().unwrap_or(0)),
            ),
        };
        println!("PARTS {name} {area} kind {kind} msb {n} status {status} world {in_world}");
    }

    // regions and events, by type
    let mut region_records: BTreeMap<String, usize> = BTreeMap::new();
    for (_, r) in world.iter::<Region>() {
        *region_records.entry(word(r.type_name, r.type_code)).or_default() += 1;
    }
    let mut event_records: BTreeMap<String, usize> = BTreeMap::new();
    for (_, e) in world.iter::<Event>() {
        *event_records.entry(word(e.type_name, e.type_code)).or_default() += 1;
    }
    let mut regions: BTreeMap<(String, u32), Vec<&Region>> = BTreeMap::new();
    let all_regions = layout.map_regions()?;
    for r in &all_regions {
        regions
            .entry((word(r.type_name, r.type_code), r.type_code))
            .or_default()
            .push(r);
    }
    for ((kind, code), list) in &regions {
        let shapes: BTreeSet<String> = list
            .iter()
            .map(|r| {
                serde_json::to_value(r)
                    .ok()
                    .and_then(|v| v["shape"].as_str().map(str::to_owned))
                    .unwrap_or_default()
            })
            .collect();
        let lengths: BTreeSet<usize> = list.iter().map(|r| r.type_data.len()).collect();
        let example = list
            .iter()
            .find(|r| !r.type_data.is_empty())
            .map(|r| hex(&r.type_data));
        let entity_ids = list.iter().filter(|r| r.entity_id > 0).count();
        // a spawn region is the extras' travel marker, not a record of the records pass
        let (status, lowered) = if layout.generation.spawn_region == Some(*code) {
            (
                "read (lower-fromsoft-extras: travel markers)",
                format!(
                    "0 records, {} read as spawn points",
                    extras.region_types.get(kind).copied().unwrap_or(0)
                ),
            )
        } else {
            (
                "read-header",
                format!("{} records", region_records.get(kind).copied().unwrap_or(0)),
            )
        };
        println!(
            "REGIONS {name} {area} type {kind} code {code} msb {} shapes {shapes:?} data_lengths {lengths:?} \
             entity_ids {entity_ids} status {status} world {lowered}{}",
            list.len(),
            example.map_or(String::new(), |e| format!(" example {e}")),
        );
    }
    let all_events = layout.map_events()?;
    let mut events: BTreeMap<(String, u32), Vec<&Event>> = BTreeMap::new();
    for e in &all_events {
        events
            .entry((word(e.type_name, e.type_code), e.type_code))
            .or_default()
            .push(e);
    }
    for ((kind, code), list) in &events {
        let lengths: BTreeSet<usize> = list.iter().map(|e| e.type_data.len()).collect();
        let example = list
            .iter()
            .find(|e| !e.type_data.is_empty())
            .map(|e| hex(&e.type_data));
        let parts_named = list.iter().filter(|e| e.part.is_some()).count();
        let regions_named = list.iter().filter(|e| e.region.is_some()).count();
        let status = if layout.generation.map_offset_event == Some(*code) {
            "read-header (and the map offset, which the area read uses)"
        } else {
            "read-header"
        };
        println!(
            "EVENTS {name} {area} type {kind} code {code} msb {} data_lengths {lengths:?} parts {parts_named} \
             regions {regions_named} status {status} world {} records{}",
            list.len(),
            event_records.get(kind).copied().unwrap_or(0),
            example.map_or(String::new(), |e| format!(" example {e}")),
        );
    }

    // everything the read put in the world
    for component in world.component_names() {
        println!(
            "WORLD {name} {area} component {component} entities {}",
            world.entities_named(component).len()
        );
    }
    for resource in world.resource_names() {
        println!("RESOURCE {name} {area} {resource}");
    }
    Ok(())
}

fn census(
    game: Game,
    install: &Install,
    names: Option<&NameDictionary>,
    decoder: &Decoder<'_>,
) -> Result<()> {
    let archive = install.open()?;
    if let Some(names) = names {
        let unnamed = archive
            .entries
            .keys()
            .filter(|h| !names.names.contains_key(h))
            .count();
        println!(
            "DICTIONARY {} entries {} unnamed {unnamed}",
            game.name(),
            archive.entries.len()
        );
    } else {
        println!("DICTIONARY {} none: the FILES lines are not printed", game.name());
    }
    for (label, options) in candidates(game) {
        match options
            .iter()
            .find(|a| archive.contains(&format!("/map/mapstudio/{a}.msb.dcx")))
        {
            Some(area) => area_census(game, &archive, decoder, names, area, label)?,
            None => println!("NO-AREA {} {label} {options:?}", game.name()),
        }
    }
    Ok(())
}

#[test]
fn what_the_maps_store_and_what_primase_keeps() -> Result<()> {
    let Some(dll) = primase_testdata::oodle(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data) in [
        (Game::Sekiro, primase_testdata::Game::Sekiro),
        (Game::EldenRing, primase_testdata::Game::Elden),
    ] {
        let (Some(root), keys) = (primase_testdata::install(data), primase_testdata::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (Game::Sekiro, _) => Keys::published(Game::Sekiro).ok_or("Sekiro's keys")?,
            (_, Some(folder)) => Keys::from_folder(folder, game)?,
            (_, None) => continue,
        };
        let names = primase_testdata::oracle(primase_testdata::OracleSet::Uxm, data, "dictionary.txt")
            .map(|path| NameDictionary::read_for(path, game))
            .transpose()?;
        census(game, &Install::new(root, game, keys), names.as_ref(), &decoder)?;
    }
    Ok(())
}
