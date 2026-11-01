//! Source catalog identities and pass materialization results, without reader I/O.
use super::outfits::CharacterOutfit;
use primase_core::Resource;
use serde::Serialize;
use serde_json::{Value, json};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Subject {
    Character,
    Object,
    Map,
    Other,
}
/// A source reader request in `open`/`details.sets` uses `reader` = area, character or parts;
/// a program maps that to its own reader adapter name and passes the remaining options through.
#[derive(Clone, Debug)]
pub struct CatalogEntry {
    pub key: String,
    pub kind: String,
    pub subject: Subject,
    pub name: String,
    pub path: String,
    pub open: Value,
    pub refusal: Option<String>,
    pub details: Value,
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
}
impl Resource for Catalog {
    const NAME: &'static str = "fromsoft.catalog";
}

/// Authored association of catalog placement records to their character, before any form is selected.
#[derive(Clone, Debug)]
pub struct CatalogInputs {
    pub player_parent: Option<String>,
    pub player_insert: usize,
    pub npc_insert: usize,
    pub npc_rows: Vec<(String, fromsoft_formats::equipment::NpcOutfit)>,
}
impl Resource for CatalogInputs {
    const NAME: &'static str = "fromsoft.catalog_inputs";
}
/// A catalog after its explicit presentation passes have materialized opening requests.
#[derive(Clone, Debug)]
pub struct CatalogReady;
impl Resource for CatalogReady {
    const NAME: &'static str = "fromsoft.catalog_ready";
}

/// The archive listing behind the catalog: the character and part binders the archive actually holds, named by a
/// caller-provided dictionary. Only BHD5 headers and BDT metadata are read; no geometry, texture or binder payload is
/// touched.
pub fn player_outfit_entries(parent: &str, choices: &[CharacterOutfit]) -> Vec<CatalogEntry> {
    choices
        .iter()
        .map(|outfit| CatalogEntry {
            key: outfit.key(),
            kind: "outfit".into(),
            subject: Subject::Character,
            name: format!("{} ({})", outfit.name, outfit.gender.key()),
            path: format!("{} outfit {}", outfit.animations_of, outfit.key()),
            refusal: (!outfit.readable()).then(|| {
                if outfit.unresolved.is_empty() {
                    "initializer resolves no drawable parts".into()
                } else {
                    outfit.unresolved.join("; ")
                }
            }),
            open: if outfit.readable() {
                json!({"reader":"character","id":outfit.animations_of,"role":"player","outfit":outfit.key()})
            } else {
                Value::Null
            },
            details: json!({"parent":parent,"model":outfit.animations_of,"outfit":outfit,
            "label":"installed initializer variant; a presentation chosen of it, not the game's default"}),
        })
        .collect()
}
