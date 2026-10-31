//! Authored FromSoft catalog reading: indexes, binder forms, parameter links, names and lazy clip plans.
#![allow(clippy::disallowed_methods, reason = "reader I/O: installed catalog metadata and an explicitly supplied cache directory")]
use std::path::{Path, PathBuf};
use fromsoft_formats::{Archive, Install, Game as FromSoftGame, dcx::Decoder, listing};
use primase_foundations::parallel::{self, Threads};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use crate::sekiro::{self, CharacterReader, CharacterOutfit};
use primase_core::convert::Reader;
type Result<T> = fromsoft_formats::Result<T>;
macro_rules! anyhow { ($($arg:tt)*) => { crate::invalid("FromSoft catalog", 0, format!($($arg)*)) }; }

/// Caller-stated presentation settings. The reader supplies no language or cache location default.
#[derive(Clone,Debug)]
pub struct Options { pub language:String, pub language_source:String, pub cache_dir:Option<PathBuf> }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
pub enum Subject { Character,Object,Map,Other }
/// A source reader request in `open`/`details.sets` uses `reader` = area, character or parts;
/// a program maps that to its own reader adapter name and passes the remaining options through.
#[derive(Clone,Debug)]
pub struct CatalogEntry {
    pub key:String,pub kind:String,pub subject:Subject,pub name:String,pub path:String,
    pub open:Value,pub refusal:Option<String>,pub details:Value,
}
#[derive(Clone,Debug,Default)]
pub struct Catalog {pub entries:Vec<CatalogEntry>}

/// Source-stated report fields and notices, rendered generically by a program.
pub struct DescriptiveReport {
    pub fields: serde_json::Map<String,Value>,
    pub notices: Vec<primase_core::convert::Notice>,
}

/// The selected row's descriptive data and display-only card; selecting the row is a separate operation.
pub fn npc_description(install:&Install,archive:&Archive,decoder:&Decoder<'_>,row:i32)->DescriptiveReport {
    let mut fields=serde_json::Map::new();
    let mut notices=Vec::new();
    match fromsoft_formats::game_params::read_npc_stats(install,archive,decoder) {
        Ok(stats)=>if let Some(stats)=stats.iter().find(|stats|stats.id==row) {
            let values=stats.fields();
            fields.insert("npc_row".into(),json!({"row":row,"param":"NPC_PARAM_ST",
                "fields":values.iter().map(|(name,value,unit)|json!({"name":name,"value":value,"unit":unit})).collect::<Vec<_>>()}));
            let card=primase_core::report_card::ReportCard {
                title:format!("NPC param row {row}"),
                description:"Descriptive fields of the row (NPC_PARAM_ST), by the definition's names; the row's other fields are not read.".into(),
                fields:values.iter().map(|(name,value,unit)|primase_core::report_card::ReportField {
                    name:(*name).into(),value:json!(value),unit:Some((*unit).into()),
                }).collect(),
            };
            fields.insert("cards".into(),json!([card]));
        },
        Err(error)=>notices.push(primase_core::convert::Notice::new("fromsoft",primase_core::convert::NoticeKind::Unsupported,
            format!("NPC row {row} descriptive fields unread: {error}"),1)),
    }
    DescriptiveReport {fields,notices}
}
mod names {
    use serde_json::{json,Value};
    pub const COMMUNITY:&str="community";
    pub const GAME:&str="game";
    pub fn alias(name:&str,source:&str)->Value {json!({"name":name,"source":source})}
}
pub fn listed_details(entry: &listing::ListingEntry) -> Value {
    let mut details = json!({
        "hash": entry.hash,
        "shard": entry.shard,
        "padded_size": entry.padded_size,
    });
    if entry.logical_size > 0 {
        details["size"] = json!(entry.logical_size);
        details["logical_size"] = json!(entry.logical_size);
    }
    details
}

/// Read the install's authored catalog, with every presentation choice stated by the program.
pub fn read(install: &Install, decoder: &Decoder<'_>, names: Option<&Path>, options: &Options) -> Result<Catalog> {
    let dictionary = names.ok_or_else(|| anyhow!("no FromSoft name dictionary"))?;
    let dictionary_data = fromsoft_formats::NameDictionary::read_for(dictionary, install.game)?;
    let archive = std::sync::Arc::new(install.open()?);
    let listing = listing::Listing::from_entries(install.root.clone(), Some(dictionary.to_path_buf()), &archive.entries, &dictionary_data);
    let npc_rows = npc_entries(&install, &archive, &decoder, names.as_deref())?;
    let rows: Vec<CatalogEntry> = listing
        .entries
        .iter()
        .map(|entry| {
            let mut refusal = None;
            let (kind, subject, open) = match entry.kind {
                listing::AssetKind::Area => {
                    // the dictionary names areas whose map file the archives do not hold (Sekiro's m89_00_00_99):
                    // listed, refused with that reason, never an open that fails
                    let msb = format!("/map/mapstudio/{}.msb.dcx", entry.name);
                    let held = archive.contains(&msb);
                    if !held {
                        refusal = Some(format!(
                            "{} has no map file in the archives ({msb}), so the area does not read",
                            entry.name
                        ));
                    }
                    (
                        "area",
                        Subject::Map,
                        if held {
                            json!({ "reader": "area", "area": entry.name })
                        } else {
                            Value::Null
                        },
                    )
                }
                listing::AssetKind::Character => (
                    "character",
                    Subject::Character,
                    json!({ "reader": "character", "id": entry.name }),
                ),
                listing::AssetKind::Part => (
                    "part",
                    Subject::Object,
                    json!({ "reader": "parts", "parts": [entry.name] }),
                ),
            };
            CatalogEntry {
                key: entry.key.clone(),
                kind: kind.into(),
                subject,
                refusal,
                name: entry.name.clone(),
                path: entry.path.clone(),
                open,
                details: {
                    let mut details = listed_details(entry);
                    if entry.kind == listing::AssetKind::Character {
                        details["animations"] = json!(animation_binder_held(&archive, &entry.name));
                    }
                    details
                },
            }
        })
        .collect();
    // each part binder's forms, read from its members (not its models), on several threads: a binder is decoded
    // whole to list them, and the archive opens its files for each read
    let parts: Vec<&str> = rows
        .iter()
        .filter(|row| row.kind == "part")
        .map(|row| row.name.as_str())
        .collect();
    let threads = std::thread::available_parallelism()
        .ok()
        .and_then(|n| Threads::new(n.get().min(8)))
        .unwrap_or(Threads::ONE);
    let mut binders = parallel::map(threads, &parts, |_, part| {
        sekiro::part_forms(&archive, &decoder, part).map_err(|error| error.to_string())
    })
    .map_err(|error| anyhow!("reading the part binders' forms: {error}"))?
    .into_iter();
    let mut entries: Vec<CatalogEntry> = Vec::new();
    for row in rows {
        if row.kind != "part" {
            entries.push(row);
            continue;
        }
        // a binder of no FLVER is refused, saying what it holds
        let read = binders
            .next()
            .ok_or_else(|| anyhow!("a part binder's forms were not read"))?;
        match read {
            Err(error) => match sekiro::part_textures(&archive, &decoder, &row.name) {
                // a binder of no model that holds textures (a face's) is its textures: it opens as a sheet of them
                Ok(textures) if !textures.is_empty() => {
                    let mut row = row;
                    row.details["textures"] = json!(textures.len());
                    row.details["label"] = json!(format!("{} textures, no model", textures.len()));
                    entries.push(row);
                }
                _ => entries.push(CatalogEntry {
                    open: Value::Null,
                    refusal: Some(error),
                    ..row
                }),
            },
            Ok(forms) => {
                let mut row = row;
                row.details["forms"] = json!(forms);
                let several: Vec<CatalogEntry> = if forms.len() > 1 {
                    forms
                        .iter()
                        .enumerate()
                        .map(|(i, form)| form_row(&row, form, i, forms.len()))
                        .collect()
                } else {
                    Vec::new()
                };
                entries.push(row);
                entries.extend(several);
            }
        }
    }
    // an NPC row is a variant of the character whose model it names
    let characters: std::collections::BTreeMap<String, String> = entries
        .iter()
        .filter(|entry| entry.kind == "character")
        .map(|entry| (entry.name.clone(), entry.key.clone()))
        .collect();
    if let Some(parent) = characters.get("c0000") {
        match sekiro::character_outfits(&install, &archive, &decoder) {
            Ok(outfits) => entries.extend(player_outfit_entries(parent, &outfits)),
            Err(error) => entries.push(CatalogEntry {
                key: "initializer/unread".into(), kind: "outfit".into(), subject: Subject::Character,
                name: "Player outfits unread".into(), path: "CharaInitParam".into(),
                refusal: Some(format!("installed player outfits did not read: {error}")), open: Value::Null,
                details: json!({"parent":parent,"model":"c0000"}),
            }),
        }
    }
    for mut row in npc_rows {
        if let Some(parent) = row.details["model"]
            .as_str()
            .and_then(|model| characters.get(model))
        {
            row.details["parent"] = json!(parent);
        }
        entries.push(row);
    }
    // each draw-param file of an area's map is a variant of the area: it opens the area with that environment, and is
    // a value of the `environment` option (listed for the `area` given: the full name opens the row, the short name
    // `m11_01` an area `m11_01_00_00` states too is a value of it)
    // Elden Ring's open-world tiles (m60, m61) are left out here: thousands of tiles share one map's files, and a row
    // per tile and file would outnumber every other row of the catalog; their files are the shared rows below
    let areas: Vec<(String, String)> = entries
        .iter()
        .filter(|entry| entry.kind == "area" && !entry.open.is_null())
        .filter(|entry| {
            !(install.game == FromSoftGame::EldenRing
                && (entry.name.starts_with("m60_") || entry.name.starts_with("m61_")))
        })
        .map(|entry| (entry.key.clone(), entry.name.clone()))
        .collect();
    for (parent, area) in areas {
        for stem in fromsoft_formats::area::draw_params(&archive, install.game, &area) {
            entries.push(CatalogEntry {
                key: format!("environment/{area}/{stem}"),
                kind: "environment".into(),
                subject: Subject::Map,
                refusal: None,
                name: stem.clone(),
                path: format!("{area} environment {stem}"),
                open: json!({ "reader": "area", "area": area, "environment": stem }),
                details: json!({ "parent": parent, "label": "draw-param file" }),
            });
            if let Some(short) = area.strip_suffix("_00_00") {
                entries.push(CatalogEntry {
                    key: format!("environment/{short}/{stem}"),
                    kind: "environment".into(),
                    subject: Subject::Map,
                    refusal: None,
                    name: stem.clone(),
                    path: format!("{short} environment {stem}"),
                    open: Value::Null,
                    details: json!({
                        "parent": parent,
                        "label": "draw-param file",
                        "sets": { "reader": "area", "area": short, "environment": stem },
                    }),
                });
            }
        }
    }
    // Elden Ring's open-world tiles share the files of one map (`m60_00_*`, `m61_00_*`): one value-only row per file,
    // stating the file for every tile of that map the catalog lists (a row of its own per tile and file would outnumber
    // every other row); a tile's `environment` choice is these
    if install.game == FromSoftGame::EldenRing {
        let mut tiles: std::collections::BTreeMap<&str, Vec<String>> = std::collections::BTreeMap::new();
        for entry in &entries {
            if entry.kind == "area" && !entry.open.is_null()
                && let Some(map) = ["m60", "m61"].into_iter().find(|map| entry.name.starts_with(&format!("{map}_"))) {
                tiles.entry(map).or_default().push(entry.name.clone());
            }
        }
        let mut rows = Vec::new();
        for (map, names) in &tiles {
            for stem in fromsoft_formats::area::draw_params(&archive, install.game, &names[0]) {
                rows.push(CatalogEntry {
                    key: format!("environment/{map}/{stem}"),
                    kind: "environment".into(),
                    subject: Subject::Map,
                    refusal: None,
                    name: stem.clone(),
                    path: format!("{map} tiles environment {stem}"),
                    open: Value::Null,
                    details: json!({
                        "label": "draw-param file",
                        "tiles": names.len(),
                        "sets": { "reader": "area", "area": names, "environment": stem },
                    }),
                });
            }
        }
        entries.extend(rows);
    }
    // each animation of a character is a variant of it: it opens the character with that clip lowered onto it
    let animated: Vec<String> = entries
        .iter()
        .filter(|entry| entry.kind == "character" && entry.details["animations"] == true)
        .map(|entry| entry.name.clone())
        .collect();
    let clips = clip_lists(&install, &archive, &decoder, &animated, names.as_deref(), options.cache_dir.as_deref());
    for (model, clips) in clips {
        let Some(parent) = characters.get(&model) else {
            continue;
        };
        let clips=match clips {
            Ok(clips)=>clips,
            Err(error)=>{
                entries.push(CatalogEntry{key:format!("clip/{model}/unread"),kind:"clip".into(),subject:Subject::Character,
                    refusal:Some(format!("{model}: clip planning failed: {error}")),name:format!("{model} clips unread"),
                    path:format!("{model} animation binders"),open:Value::Null,
                    details:json!({"parent":parent,"model":model})});
                continue;
            }
        };
        for clip in clips {
            entries.push(CatalogEntry {
                key: format!("clip/{model}/{}",clip.key),
                kind: "clip".into(),
                subject: Subject::Character,
                refusal: None,
                name: clip.name.clone(),
                path: format!("{model} clip {}",clip.name),
                open: json!({
                    "reader": "character", "id": model, "animations": true, "clips": clip.key,
                }),
                details: json!({ "parent": parent, "model": model, "label": clip.name }),
            });
        }
    }
    // the game's own text first: an NPC row's `nameId` in the item message binder's NPC-name FMG
    let game_text=names.as_deref().ok_or_else(||"NPC names need the install's name dictionary to enumerate languages".to_owned())
        .and_then(|path|fromsoft_formats::NameDictionary::read_for(path,install.game).map_err(|e|e.to_string()))
        .and_then(|dictionary|fromsoft_formats::messages::npc_names(&install,&archive,&decoder,&dictionary,options.language.as_str()).map_err(|e|e.to_string()));
    name_entries(&mut entries,install.game,game_text.as_ref().map(|read|&read.names).map_err(String::as_str));
    for entry in entries.iter_mut().filter(|entry|entry.kind=="npc") {
        entry.details["name_language"]=json!({"value":options.language.as_str(),"reason":options.language_source.as_str()});
        if let Ok(read)=&game_text {entry.details["name_notices"]=json!(read.report);}
    }
    // each weapon row whose model the archives hold: a value of weapons_right and weapons_left
    entries.extend(weapon_entries(&install, &archive, &decoder));
    Ok(Catalog { entries })
}

/// The catalog's weapons: each `EquipParamWeapon` row with a model whose part binder the archives hold, opening as its
/// models (named in the right hand, held by no one). The rows without a model (the game's unarmed rows) or whose model no
/// binder holds are left out; params that do not read are one refused row saying why.
pub fn weapon_entries(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Vec<CatalogEntry> {
    let rows = match sekiro::WeaponParams::read(install, archive, decoder).and_then(|params| params.weapons()) {
        Ok(rows) => rows,
        Err(error) => {
            return vec![CatalogEntry {
                key: "weapon/unread".into(),
                kind: "weapon".into(),
                subject: Subject::Object,
                refusal: Some(format!("the weapon params did not read: {error}")),
                name: "weapons unread".into(),
                path: "EquipParamWeapon".into(),
                open: Value::Null,
                details: json!({}),
            }];
        }
    };
    let mut binders: std::collections::BTreeMap<String, Option<String>> = std::collections::BTreeMap::new();
    rows.into_iter()
        .filter(|weapon| weapon.model > 0)
        .filter_map(|weapon| {
            let base = weapon.part();
            let part = binders
                .entry(base.clone())
                .or_insert_with(|| weapon_binder(archive, &base))
                .clone()?;
            Some(CatalogEntry {
                key: format!("weapon/{}", weapon.row),
                kind: "weapon".into(),
                subject: Subject::Object,
                refusal: None,
                name: weapon.row.to_string(),
                path: format!("EquipParamWeapon {} {part}", weapon.row),
                open: json!({ "reader": "parts", "weapons_right": [weapon.row.to_string()] }),
                details: json!({ "part": part, "label": format!("{part} (EquipParamWeapon {})", weapon.row) }),
            })
        })
        .collect()
}

pub fn name_entries(
    entries: &mut [CatalogEntry],
    game: FromSoftGame,
    game_text: std::result::Result<&std::collections::BTreeMap<i32, String>, &str>,
) {
    use crate::entity_names as source_names;
    for entry in entries.iter_mut() {
        let community = |name: Option<&str>| name.map(|name| names::alias(name, names::COMMUNITY));
        let alias = match entry.kind.as_str() {
            "area" => community(source_names::map(game, &entry.name)),
            "character" => community(source_names::character(game, &entry.name)),
            "part" => community(source_names::part(game, &entry.name)),
            "clip" => {
                let model = entry.details["model"].as_str().unwrap_or_default();
                if let Some(act) = source_names::time_act(game, model, &entry.name) {
                    entry.details["time_act"] = json!(act);
                    entry.details["label"] = json!(format!("{} · time act {act} (community)", entry.name));
                }
                community(source_names::animation(game, &entry.name))
            }
            "npc" => {
                let from_game = entry
                    .name
                    .parse::<i32>()
                    .ok()
                    .and_then(|row| game_text.ok().and_then(|text| text.get(&row)));
                if let Err(why) = &game_text {
                    entry.details["name_unread"] = json!(why);
                }
                match from_game {
                    Some(name) => Some(names::alias(name, names::GAME)),
                    None => community(
                        entry.details["model"].as_str().and_then(|model| source_names::character(game, model)),
                    ),
                }
            }
            _ => None,
        };
        if let Some(alias) = alias {
            entry.details["alias"] = alias;
        }
    }
}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct CatalogClip {pub key:String,pub name:String}
pub type ClipLists = std::collections::BTreeMap<String, std::result::Result<Vec<CatalogClip>,String>>;

/// The animations each of `models` has (`a000_003000`): the member names of its animation binders, read on several
/// threads (a binder is decoded whole to list them) and kept between runs. A model whose binders cannot be read lists
/// a named refusal. Duplicate names retain their distinct reader keys.
pub fn clip_lists(
    install: &Install,
    archive:&std::sync::Arc<Archive>,
    decoder: &Decoder<'_>,
    models: &[String],
    names: Option<&Path>,
    cache_dir: Option<&Path>,
) -> ClipLists {
    let file = cache_file("clip-names", install, names, cache_dir);
    if let Some(known) = file
        .as_ref()
        .and_then(|file| serde_json::from_slice::<ClipLists>(&std::fs::read(file).ok()?).ok())
    {
        return known;
    }
    let threads = std::thread::available_parallelism()
        .ok()
        .and_then(|n| Threads::new(n.get().min(8)))
        .unwrap_or(Threads::ONE);
    let lists = parallel::map(threads, models, |_, model| -> std::result::Result<Vec<CatalogClip>,String> {
        let mut reader=CharacterReader::animations_in(install.clone(),model,*decoder,archive.clone()).map_err(|error|error.to_string())?;
        let (plans,_)=reader.read().map_err(|error|error.to_string())?;
        Ok(plans.iter::<primase_core::ClipKey>().filter_map(|(entity,key)|
            plans.get::<primase_core::Name>(entity).map(|name|CatalogClip{key:key.0.clone(),name:name.0.clone()})).collect())
    }).unwrap_or_else(|error|models.iter().map(|_|Err(format!("parallel clip planning failed: {error}"))).collect());
    let lists: ClipLists = models
        .iter()
        .cloned()
        .zip(lists)
        .collect();
    if let Some(file) = &file {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, serde_json::to_vec(&lists).unwrap_or_default());
    }
    lists
}

/// One form of a part binder that holds several (`form`, the `index`th of `count`): a variant of the part's row, kind
/// `part-form`, opening the part with that form kept.
pub fn form_row(part: &CatalogEntry, form: &str, index: usize, count: usize) -> CatalogEntry {
    CatalogEntry {
        key: format!("{}#{form}", part.key),
        kind: "part-form".into(),
        subject: Subject::Object,
        name: form.to_owned(),
        path: part.path.clone(),
        open: json!({ "reader": "parts", "parts": [part.name], "form": form }),
        refusal: None,
        details: json!({ "parent": part.key, "label": format!("form {} of {count}", index + 1) }),
    }
}

/// The NPC param rows that name a model in a map's parts: one entry per (model, row), kind `npc`, its `open` the
/// character with that row (the option `npc` takes the row's id). Which rows point at which model is the map files'
/// statement (an Enemy or DummyEnemy part's model and `NPCParamID`), over the maps the names dictionary lists; a part
/// that names no row (0 or -1) lists none. The row is in the NPC param or the reading is refused. A row whose model has
/// no character binder in the archives (a model that lives elsewhere, or none) is listed, refused, with that reason:
/// the archive's hash lookup says so without reading the binder.
pub fn npc_entries(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    names: Option<&Path>,
) -> Result<Vec<CatalogEntry>> {
    let names = names.ok_or_else(|| anyhow!("no FromSoft name dictionary"))?;
    let dictionary = fromsoft_formats::NameDictionary::read_for(names, install.game)
        .map_err(|error| anyhow!("dictionary {}: {error}", names.display()))?;
    let placements = fromsoft_formats::game_params::npc_placements(archive, decoder, &dictionary)
        .map_err(|error| anyhow!("reading the maps' enemy parts: {error}"))?;
    // per (model, row): the maps and the parts that name it, in map then file order
    let mut rows: std::collections::BTreeMap<(String, i32), (Vec<String>, usize, usize)> =
        std::collections::BTreeMap::new();
    for p in placements
        .iter()
        .filter(|p| p.row > 0 && !p.model.is_empty())
    {
        let (maps, parts, dummies) = rows.entry((p.model.clone(), p.row)).or_default();
        if !maps.contains(&p.map) {
            maps.push(p.map.clone());
        }
        *parts += 1;
        *dummies += usize::from(p.dummy);
    }
    let mut held: std::collections::BTreeMap<String, (bool, bool)> =
        std::collections::BTreeMap::new();
    let mut entries: Vec<_> = rows
        .into_iter()
        .map(|((model, row), (maps, parts, dummies))| {
            let (binder, animations) = *held.entry(model.clone()).or_insert_with(|| {
                (
                    archive.contains(&fromsoft_formats::paths::character(&model)),
                    animation_binder_held(archive, &model),
                )
            });
            npc_entry(&model, row, &maps, parts, dummies, binder, animations)
        })
        .collect();
    if install.game == FromSoftGame::Sekiro {
        let models: std::collections::BTreeSet<_> = placements.iter()
            .filter(|p| p.character_init.is_some()).map(|p| p.model.as_str()).collect();
        for model in models {
            for outfit in fromsoft_formats::equipment::npc_outfits(install, archive, decoder, &dictionary, model, None)? {
                // a placement whose MSB stores a negative initializer names none
                let init = outfit.init.map_or_else(|| "none".to_owned(), |i| i.to_string());
                entries.push(CatalogEntry {
                    key: format!("outfit/{model}/{init}/{}/{}", outfit.map, outfit.placement),
                    kind: "outfit".into(), subject: Subject::Character,
                    name: format!("{init} · {} / {}", outfit.map, outfit.placement),
                    path: format!("{model} NPC {} outfit {init}", outfit.npc),
                    refusal: outfit.parts.is_empty().then(|| outfit.unresolved.join("; ")),
                    open: if outfit.parts.is_empty() { Value::Null } else {
                        json!({"reader": "character", "id": model, "npc": outfit.npc.to_string(), "init": outfit.init.map(|i| i.to_string())}) },
                    details: json!({"model": model, "outfit": outfit}),
                });
            }
        }
    }
    Ok(entries)
}

/// Whether the archives hold the animation binder of character `model` (`/chr/<model>.anibnd.dcx`, the file the
/// character reader maps its clips from): the archive's hash lookup says so without reading the binder.
pub fn animation_binder_held(archive: &Archive, model: &str) -> bool {
    archive.contains(&fromsoft_formats::paths::character_animation(&model))
}

/// The entry of one NPC row: it opens the character with that row, or, when the archives hold no character binder for
/// its model (`binder` false), is listed refused and says why. `animations` is whether the archives hold the model's
/// animation binder.
pub fn npc_entry(
    model: &str,
    row: i32,
    maps: &[String],
    parts: usize,
    dummies: usize,
    binder: bool,
    animations: bool,
) -> CatalogEntry {
    CatalogEntry {
        key: format!("npc/{model}/{row}"),
        kind: "npc".into(),
        subject: Subject::Character,
        refusal: (!binder).then(|| {
            format!("{model} has no character binder in the archives (/chr/{model}.chrbnd.dcx), so no row of it reads")
        }),
        name: row.to_string(),
        path: format!("{model} NPC row {row}"),
        open: if binder {
            json!({ "reader": "character", "id": model, "npc": row.to_string() })
        } else {
            Value::Null
        },
        details: json!({
            "model": model,
            "parts": parts,
            "dummy_parts": dummies,
            "animations": animations,
            "label": maps_label(maps),
            "maps": maps,
        }),
    }
}

/// The maps an NPC row is placed in, as the one line the Browse list shows beside it: the first three, then how many more.
pub fn maps_label(maps: &[String]) -> String {
    let shown = maps
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match maps.len().saturating_sub(3) {
        0 => shown,
        more => format!("{shown} +{more}"),
    }
}

/// The archive listing behind the catalog: the character and part binders the archive actually holds, named by a
/// caller-provided dictionary. Only BHD5 headers and BDT metadata are read; no geometry, texture or binder payload is
/// touched.
pub fn player_outfit_entries(
    parent: &str,
    choices: &[CharacterOutfit],
) -> Vec<CatalogEntry> {
    choices.iter().map(|outfit| CatalogEntry {
        key: outfit.key(), kind: "outfit".into(), subject: Subject::Character,
        name: format!("{} ({})", outfit.name, outfit.gender.key()),
        path: format!("{} outfit {}", outfit.animations_of, outfit.key()),
        refusal: (!outfit.readable()).then(|| if outfit.unresolved.is_empty() {
            "initializer resolves no drawable parts".into()
        } else { outfit.unresolved.join("; ") }),
        open: if outfit.readable() {
            json!({"reader":"character","id":outfit.animations_of,"outfit":outfit.key()})
        } else { Value::Null },
        details: json!({"parent":parent,"model":outfit.animations_of,"outfit":outfit,
            "label":"installed initializer variant; the viewer chooses a presentation, not the game's default"}),
    }).collect()
}

fn weapon_binder(archive: &fromsoft_formats::Archive, base: &str) -> Option<String> {
    [format!("{base}_l"), base.to_owned(), format!("{base}_m")]
        .into_iter()
        .find(|p| archive.contains(&fromsoft_formats::paths::part(&p)))
}

/// The explicit cache directory and input identity determine the metadata cache key.
fn cache_file(kind: &str, install: &Install, names: Option<&Path>, cache_dir: Option<&Path>) -> Option<PathBuf> {
    let mut key = format!("{:?}|{}|", install.game, install.root.display());
    if let Some(names) = names {
        let meta = std::fs::metadata(names).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        key.push_str(&format!("{}|{}|{modified}", names.display(), meta.len()));
    }
    // FNV-1a: stable between runs, which std's hashers do not promise
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(
        cache_dir?.to_path_buf()
            .join("cache")
            .join(format!("{kind}-{hash:016x}.json")),
    )
}


pub type FirstRows = std::collections::BTreeMap<String, Vec<i32>>;

/// The NPC rows (ascending) that a map's part names for each model, over the maps the dictionary lists, by install
/// root: read once per process (reading the maps' parts takes seconds).
pub fn first_rows(install: &Install, decoder: &Decoder<'_>, names: Option<&Path>, cache_dir: Option<&Path>) -> Option<FirstRows> {
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<std::collections::BTreeMap<PathBuf, Option<FirstRows>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().ok()?.get(&install.root) {
        return known.clone();
    }
    let file = cache_file("npc-rows", install, names, cache_dir);
    let rows = file
        .as_ref()
        .and_then(|file| serde_json::from_slice::<FirstRows>(&std::fs::read(file).ok()?).ok())
        .or_else(|| {
            let rows = first_rows_read(install, decoder, names)?;
            if let Some(file) = &file {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(file, serde_json::to_vec(&rows).unwrap_or_default());
            }
            Some(rows)
        });
    cache
        .lock()
        .ok()?
        .insert(install.root.clone(), rows.clone());
    rows
}


fn first_rows_read(
    install: &Install,
    decoder: &Decoder<'_>,
    names: Option<&Path>,
) -> Option<FirstRows> {
    (|| {
        let archive = install.open().ok()?;
        let entries = npc_entries(install, &archive, decoder, names).ok()?;
        let mut first = FirstRows::new();
        for entry in entries.iter().filter(|e| !e.open.is_null()) {
            let (Some(model), Some(row)) = (
                entry.details["model"].as_str(),
                entry.name.parse::<i32>().ok(),
            ) else {
                continue;
            };
            first.entry(model.to_owned()).or_default().push(row);
        }
        for rows in first.values_mut() {
            rows.sort_unstable();
            rows.dedup();
        }
        Some(first)
    })()
}


/// Attach the caller's language-specific NPC names and report the explicit language with no fallback.
pub fn read_display_names(install:&Install,decoder:&Decoder<'_>,names:Option<&Path>,world:&mut primase_core::World,options:&Options)->DescriptiveReport {
    use primase_core::convert::{Notice,NoticeKind};
    let mut fields=serde_json::Map::new();
    fields.insert("name_language".into(),json!({"value":options.language,"reason":format!("{}; no reader language fallback",options.language_source)}));
    let read=(||->Result<usize>{
        let names=names.ok_or_else(||anyhow!("game NPC names need the archive's name dictionary to enumerate msg/<lang> folders"))?;
        let dictionary=fromsoft_formats::NameDictionary::read_for(names,install.game)?;
        let archive=install.open()?;
        crate::sekiro::npc_names_into_world(install,&archive,decoder,&dictionary,&options.language,world)
    })();
    let mut notices=Vec::new();
    match read {
        Ok(count)=>{fields.insert("npc_name_rows".into(),json!(count));},
        Err(error)=>notices.push(Notice::new("fromsoft",NoticeKind::Unsupported,
            format!("game NPC names unavailable for explicit lab language {}: {error}",options.language),1)),
    }
    DescriptiveReport {fields,notices}
}
