//! Source visibility coverage for every dictionary-named character in both games. No HKX tracks,
//! meshes, textures or shaders are decoded. A zero counts events, not a claim about bone-scale animation.
mod inputs;

use fromsoft_formats::{
    Binder, Game, Keys, NameDictionary,
    dcx::Decoder,
    tae::{self, Tae, clip_animation_id},
};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_core::convert::Reader;
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use serde_json::json;
use std::collections::BTreeSet;
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn every_sekiro_and_elden_character_reports_its_clip_visibility_event_coverage() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(names)) = (install(data), oracle(OracleSet::Uxm, data, "dictionary.txt"))
        else {
            continue;
        };
        let Some(folder) = keys(data) else {
            continue;
        };
        let key = Keys::from_folder(folder, game, game.param_key_sha256())?;
        let install = common::install(root, game, key);
        let archive = std::sync::Arc::new(install.open()?);
        let dictionary = NameDictionary::read_for(names, game)?;
        let characters: BTreeSet<_> = dictionary
            .names
            .values()
            .flatten()
            .filter_map(|name| {
                let id = name.strip_prefix("/chr/")?.strip_suffix(".chrbnd.dcx")?;
                (id.len() == 5
                    && id.starts_with('c')
                    && id[1..].bytes().all(|b| b.is_ascii_digit())
                    && archive.contains(name))
                .then(|| id.to_owned())
            })
            .collect();
        for character in &characters {
            let mut reader =
                sekiro::CharacterReader::animations_in(install.clone(), character, decoder, archive.clone())?;
            let (plans, _) = reader.read().map_err(|error| error.to_string())?;
            let clips: Vec<_> = plans
                .iter::<sekiro::ClipSource>()
                .map(|(_, source)| source)
                .collect();
            if !archive.contains(&format!("/chr/{character}.anibnd.dcx")) {
                println!(
                    "FS_CHARACTER_VISIBILITY={}",
                    json!({"game":game.name(),"character":character,"clips":0,"visibility_clips":0,"reason":"no animation binder"})
                );
                continue;
            }
            let mut records = std::collections::BTreeMap::new();
            // imports naming an animation no event file of the binder holds: their records have no events to read
            let mut unresolved = Vec::new();
            for binder in common::clip_binders(&plans) {
                let binder = Binder::read(&archive.read(&binder, &decoder)?, &decoder)?;
                let mut taes = Vec::new();
                for file in &binder.files {
                    let name = file.name.as_deref().unwrap_or("");
                    if name.to_ascii_lowercase().ends_with(".tae") {
                        taes.push(Tae::read_member(&binder, file)?);
                    }
                }
                // an import names an animation of any event file of the binder
                for tae in &taes {
                    for animation in &tae.animations {
                        let number = tae.number(animation);
                        match tae::resolve(&taes, number) {
                            Ok(Some((_, inherited))) => {
                                records.entry(number).or_insert_with(|| {
                                    sekiro::decode_visibility(character, &inherited.events)
                                });
                            }
                            Ok(None) => return Err("a TAE record disappeared".into()),
                            Err(why) => unresolved.push(why.to_string()),
                        }
                    }
                }
            }
            let mut matched = 0;
            let mut visibility = 0;
            let mut events = 0;
            let mut refused = 0;
            let mut kinds = std::collections::BTreeMap::new();
            for clip in &clips {
                let Some(timeline) = clip_animation_id(&clip.name).and_then(|id| records.get(&id)) else {
                    continue;
                };
                matched += 1;
                if !timeline.changes.is_empty() || !timeline.refusals.is_empty() {
                    visibility += 1;
                }
                events += timeline.changes.len();
                refused += timeline.refusals.len();
                for change in &timeline.changes {
                    *kinds.entry(change.kind).or_insert(0usize) += 1;
                }
            }
            println!(
                "FS_CHARACTER_VISIBILITY={}",
                json!({"game":game.name(),"character":character,"clips":clips.len(),
                "clips_with_tae":matched,"visibility_clips":visibility,"visibility_events":events,"by_kind":kinds,"refused_records":refused,"unresolved_imports":unresolved,
                "scope":"TAE233 latched assignments and711/713 intervals; bone-scale motion not decoded in this census"})
            );
        }
    }
    Ok(())
}
