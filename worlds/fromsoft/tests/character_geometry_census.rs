//! Every dictionary-named character binder: drawable source mesh count, or an explicit reason for zero.
//! Reads FLVERs only, never textures, shader bakes or animation tracks. This is source geometry coverage,
//! not a claim that every material will lower or a rendered draw count. Patch/archive lookup uses Install's policy.
use std::collections::BTreeSet;
use primase_core::convert::Reader;
use primase_fromsoft::{Game, Install, Keys, NameDictionary, dcx::Decoder, sekiro};
use primase_testdata::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use serde_json::json;
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn every_character_has_source_geometry_or_a_named_empty_reason() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else { return Ok(()); };
    let codec = common::oodle(library)?;
    let decoder = Decoder { oodle: Some(&codec), max_output: 1 << 30 };
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(names)) = (install(data), oracle(OracleSet::Uxm, data, "dictionary.txt")) else { continue; };
        let key = match game {
            Game::Sekiro => Keys::published(game).ok_or("published keys")?,
            Game::EldenRing => { let Some(folder) = keys(data) else { continue; }; Keys::from_folder(folder, game)? }
        };
        let install = Install::new(root, game, key);
        let archive = std::sync::Arc::new(install.open()?);
        let dictionary = NameDictionary::read_for(names, game)?;
        let placements = sekiro::npc_placements(&archive, &decoder, &dictionary)?;
        let models: BTreeSet<_> = dictionary.names.values().flatten().filter_map(|name| {
            let id = name.strip_prefix("/chr/")?.strip_suffix(".chrbnd.dcx")?;
            (id.len() == 5 && id.starts_with('c') && id[1..].bytes().all(|b| b.is_ascii_digit())
                && archive.contains(name)).then(|| id.to_owned())
        }).collect();
        let mut empty = 0usize;
        for model in &models {
            let mut reader=sekiro::CharacterReader::animations_in(install.clone(),model,decoder,archive.clone())?;
            let (plans,_)=reader.read().map_err(|error|error.to_string())?;
            let clips:Vec<_>=plans.iter::<sekiro::ClipSource>().map(|(_,source)|source).collect();
            if archive.contains(&format!("/chr/{model}.anibnd.dcx")) {
                println!("FS_CHARACTER_CLIPS={}", json!({"game": game.name(), "character": model,
                    "listed": clips.len(), "decoded": 0, "duration_known": clips.iter().filter(|c| c.duration.is_some()).count(),
                    "animation_binders": common::clip_binders(&plans),
                    "first_keys": clips.iter().take(3).map(|c| &c.key).collect::<Vec<_>>()}));
            } else {
                println!("FS_CHARACTER_CLIPS={}", json!({"game": game.name(), "character": model,
                    "listed": 0, "decoded": 0, "reason": "no character animation binder"}));
            }
            let geometry = sekiro::character_mesh_count(&archive, &decoder, model);
            let mut row = json!({"game": game.name(), "character": model, "scope": "source meshes with vertices and triangles"});
            match geometry {
                Ok(count) if count > 0 => row["drawable_source_meshes"] = json!(count),
                result => {
                    empty += 1;
                    let measured = result.is_ok();
                    row["drawable_source_meshes"] = if measured { json!(0) } else { serde_json::Value::Null };
                    row["geometry_measured"] = json!(measured);
                    row["reason"] = json!(match result { Ok(_) => "FLVER is a rig/stub with no drawable source mesh".to_owned(),
                        Err(why) => why.to_string() });
                    row["placements"] = json!(placements.iter().filter(|p| &p.model == model).collect::<Vec<_>>());
                    row["classification"] = json!(if (measured || row["reason"].as_str().is_some_and(|s| s.contains("has no FLVER;")))
                        && matches!((game, model.as_str()), (Game::Sekiro, "c1000" | "c1001")) {
                        "placeholder" } else { "real-miss" });
                    match sekiro::npc_outfits(&install, &archive, &decoder, &dictionary, model, None) {
                        Ok(outfits) => {
                            row["authored_readable_outfits"] = json!(outfits.iter().filter(|o| !o.parts.is_empty()).count());
                            if outfits.iter().any(|o| !o.parts.is_empty()) { row["classification"] = json!("part-built"); }
                            row["outfits"] = json!(outfits);
                            if row["authored_readable_outfits"] == 0 {
                                row["equipment_reason"] = json!("no map enemy initializer resolves to readable protector parts; no fallback model was invented");
                                if measured || row["reason"].as_str().is_some_and(|s| s.contains("has no FLVER;")) {
                                    row["reason"] = json!(sekiro::empty_character_reason(game, model));
                                }
                            }
                        }
                        Err(why) => row["equipment_reason"] = json!(why.to_string()),
                    }
                    assert!(row["reason"].as_str().is_some_and(|s| !s.is_empty()));
                }
            }
            println!("FS_CHARACTER_GEOMETRY={row}");
        }
        println!("FS_CHARACTER_GEOMETRY_TOTAL={}", json!({"game": game.name(), "characters": models.len(), "zero_or_unread_source_geometry": empty,
            "scope": "all dictionary-named character binders held by the archive; unknown hashed paths remain unmeasured"}));
    }
    Ok(())
}
