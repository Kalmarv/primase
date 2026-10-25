//! Source visibility coverage for every dictionary-named character in both games. No HKX tracks,
//! meshes, textures or shaders are decoded. A zero counts events, not a claim about bone-scale animation.
use std::collections::BTreeSet;
use primase_fromsoft::{Binder, Game, Install, Keys, NameDictionary, dcx::Decoder, sekiro, tae::{Tae,clip_animation_id,visibility::ClipVisibility}};
use primase_testdata::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use serde_json::json;
mod common;
type Result<T> = std::result::Result<T,Box<dyn std::error::Error>>;

#[test]
fn every_sekiro_and_elden_character_reports_its_clip_visibility_event_coverage()->Result<()> {
    let Some(library)=oodle(Tool::Oodle6) else {return Ok(());};
    let codec=common::oodle(library)?;let decoder=Decoder {oodle:Some(&codec),max_output:1<<30};
    for (game,data) in [(Game::Sekiro,Data::Sekiro),(Game::EldenRing,Data::Elden)] {
        let (Some(root),Some(names))=(install(data),oracle(OracleSet::Uxm,data,"dictionary.txt")) else {continue;};
        let key=match game {Game::Sekiro=>Keys::published(game).ok_or("published keys absent")?,
            Game::EldenRing=>{let Some(folder)=keys(data) else {continue;};Keys::from_folder(folder,game)?}};
        let install=Install::new(root,game,key);let archive=install.open()?;let dictionary=NameDictionary::read_for(names,game)?;
        let characters:BTreeSet<_>=dictionary.names.values().flatten().filter_map(|name| {
            let id=name.strip_prefix("/chr/")?.strip_suffix(".chrbnd.dcx")?;
            (id.len()==5 && id.starts_with('c') && id[1..].bytes().all(|b|b.is_ascii_digit()) && archive.contains(name)).then(||id.to_owned())
        }).collect();
        for character in &characters {
            let Some(listing)=sekiro::list_clips(&archive,&decoder,character)? else {
                println!("FS_CHARACTER_VISIBILITY={}",json!({"game":game.name(),"character":character,"clips":0,"visibility_clips":0,"reason":"no animation binder"}));continue;
            };
            let mut records=std::collections::BTreeMap::new();
            for set in std::iter::once(character).chain(listing.companions.iter()) {
                let binder=Binder::read(&archive.read(&format!("/chr/{set}.anibnd.dcx"),&decoder)?,&decoder)?;
                for file in &binder.files {
                    let name=file.name.as_deref().unwrap_or("");
                    if !name.to_ascii_lowercase().ends_with(".tae") {continue;}
                    let tae=Tae::read(&decoder.decode_if_dcx(&file.bytes)?)?;
                    for animation in &tae.animations {
                        if let Some(inherited)=tae.resolved_animation(animation.id)? {
                            records.entry(animation.id).or_insert_with(||ClipVisibility::decode(character,&inherited.events));
                        }
                    }
                }
            }
            let mut matched=0;let mut visibility=0;let mut events=0;let mut refused=0;let mut kinds=std::collections::BTreeMap::new();
            for clip in &listing.clips {
                let Some(timeline)=clip_animation_id(&clip.name).and_then(|id|records.get(&id)) else {continue;};
                matched+=1;
                if !timeline.changes.is_empty() || !timeline.refusals.is_empty() {visibility+=1;}
                events+=timeline.changes.len();refused+=timeline.refusals.len();
                for change in &timeline.changes {*kinds.entry(change.kind).or_insert(0usize)+=1;}
            }
            println!("FS_CHARACTER_VISIBILITY={}",json!({"game":game.name(),"character":character,"clips":listing.clips.len(),
                "clips_with_tae":matched,"visibility_clips":visibility,"visibility_events":events,"by_kind":kinds,"refused_records":refused,
                "scope":"TAE233 latched assignments and711/713 intervals; bone-scale motion not decoded in this census"}));
        }
    }
    Ok(())
}
