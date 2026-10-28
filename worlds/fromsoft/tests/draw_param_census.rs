//! A census of the draw-param fields the install's maps store: for each map's draw-param files, every group read at the
//! value set the area's parts name, and every field in it with its key and value as stored, one line per field:
//! `game file group id key value`. It is what `lower-fromsoft-environment` needs to lower the fog and the exposure
//! to `core.atmosphere`: which keys carry them, and in what range the values lie (a fog distance in metres or a
//! density, an exposure as a multiplier or as stops). It asserts only what is stated (a file read has groups, a group
//! its fields) and prints the rest.
//!
//! Each file is read as the area reader reads it with a region no placement reaches
//! ([`sekiro::read_area_draw_param`]: against the collision parts' graphics configs), without reading the area's models.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
mod inputs;

use fromsoft_formats::{Archive, Game, Keys, dcx::Decoder};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{self, DrawParams};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

fn census(game: Game, archive: &Archive, decoder: &Decoder<'_>, map: &str) -> Result<()> {
    // the first read names the files the archive holds; each is then read in turn
    let mut files = {
        let first = DrawParams::File(format!("{}_0000", &map[..6]));
        sekiro::read_area_draw_param(archive, decoder, map, &first)?
            .1
            .available
    };
    if files.is_empty() {
        println!("{} {map}: no draw-param file", game.name());
        return Ok(());
    }
    files.truncate(8);
    // Inspect original time tracks before the area reader selects one value. Known weather/event
    // layer names are observed only, never silently merged into the active environment.
    let mut time_files: std::collections::BTreeSet<_> = files.iter().cloned().collect();
    for prefix in std::iter::once(map[..6].to_owned()).chain(files.iter().cloned()) {
        for layer in ["weatherbase", "weatheroutdoor", "commonevent", "capturemap"] {
            let stem = format!("{prefix}_{layer}");
            let present = archive.contains(&format!("/param/drawparam/{stem}.gparam.dcx"));
            println!("GPARAM_LAYER {} {stem} present={present}", game.name());
            if present {
                time_files.insert(stem);
            }
        }
    }
    for stem in time_files {
        let bytes = archive.read(&format!("/param/drawparam/{stem}.gparam.dcx"), decoder)?;
        let data = fromsoft_formats::gparam::Gparam::read(&bytes)?;
        let mut tracks = 0;
        for group in &data.groups {
            for field in &group.fields {
                for id in field
                    .ids
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    let indices: Vec<_> = field.indices_of(id).collect();
                    if indices.len() < 2 {
                        continue;
                    }
                    tracks += 1;
                    let samples: Vec<_> = indices
                        .iter()
                        .map(|&index| {
                            serde_json::json!({
                        "hour":field.times.get(index),"value":field.values.get(index)})
                        })
                        .collect();
                    println!(
                        "GPARAM_HOUR={}",
                        serde_json::json!({"game":game.name(),"file":stem,
                        "group":group.key,"field":field.key,"id":id,"samples":samples})
                    );
                }
            }
        }
        println!(
            "GPARAM_HOUR_FILE {} {stem} version={} multi_sample_tracks={tracks}",
            game.name(),
            data.version
        );
    }
    for file in files {
        let (draw, report) =
            sekiro::read_area_draw_param(archive, decoder, map, &DrawParams::File(file.clone()))?;
        // what the reader found and left of the file: the groups it could not choose a value set for, the fields it
        // read at no set, the other times of day, the blocks it does not interpret. It says whether an empty file is
        // empty or unread.
        let r = &report;
        println!(
            "{} {file} REPORT version {} groups {} fields {} unchosen_groups {:?} unchosen_fields {} fields_without_set {} \
             time_variants {} zero_id_groups {:?} unread {:?}",
            game.name(),
            r.version,
            r.groups,
            r.fields,
            r.unchosen_groups,
            r.unchosen_fields,
            r.fields_without_set,
            r.time_variants,
            r.zero_id_groups,
            r.unread,
        );
        // what the rule left, against the IDs it left: a group with fields and no set chosen, with the IDs it stores;
        // and how much the file holds at all (a file whose groups are all empty stores nothing)
        println!(
            "{} {file} SETS bytes {} empty_groups {} other_ids {:?} absent_ids {:?}",
            game.name(),
            r.bytes,
            r.empty_groups,
            r.other_ids,
            r.absent_ids,
        );
        // what the parts name in each slot (id as stored, negative for none), and their scene configs: the evidence for
        // what selects a group's value set
        println!(
            "{} {file} PARTSLOTS {:?} SCENES {:?}",
            game.name(),
            r.slots,
            r.scenes
        );
        for (group, (ids, fields)) in &r.unchosen_sets {
            println!(
                "{} {file} UNCHOSEN [{group}] fields {fields} ids {ids:?}",
                game.name()
            );
        }
        let draw = draw.ok_or_else(|| format!("{file}: no draw param read"))?;
        // a census states what it finds: a file with no group, or a group with no field, is a line, not a failure
        if draw.groups.is_empty() {
            println!("{} {file} NO-GROUPS", game.name());
        }
        for group in &draw.groups {
            if group.fields.is_empty() && group.chosen != sekiro::source::SetChosen::ZeroId {
                println!(
                    "{} {file} [{}] NO-FIELDS {:?}",
                    game.name(),
                    group.key,
                    group.chosen
                );
            }
            for field in &group.fields {
                println!(
                    "{} {file} [{}] {} {} {:?}",
                    game.name(),
                    group.key,
                    group.id,
                    field.key,
                    field.value
                );
            }
        }
    }
    Ok(())
}

#[test]
fn the_fields_the_maps_store() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data, maps) in [
        (
            Game::Sekiro,
            inputs::Game::Sekiro,
            &["m11_01_00_00", "m20_00_00_00"][..],
        ),
        (
            Game::EldenRing,
            inputs::Game::Elden,
            &["m10_00_00_00", "m12_02_00_00"][..],
        ),
    ] {
        let (Some(root), keys) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let keys = match (game, keys) {
            (_, Some(folder)) => Keys::from_folder(folder, game, game.param_key_sha256())?,
            (_, None) => continue,
        };
        // one archive for the game's maps, its indexes unwrapped on the configured threads
        let archive = common::install(root, game, keys).open()?;
        for map in maps {
            census(game, &archive, &decoder, map)?;
        }
    }
    Ok(())
}
