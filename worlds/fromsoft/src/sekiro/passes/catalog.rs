//! Materialize an authored catalog after registered presentation selectors have made their choices.
use crate::sekiro::{
    components::catalog::{
        Catalog, CatalogEntry, CatalogInputs, CatalogReady, Subject, player_outfit_entries,
    },
    components::standalone::{SelectedWeaponForms, WeaponInventory},
    outfit_data::{NpcOutfitVariants, OutfitVariants},
};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Resource, World};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug)]
pub struct MaterializeFromSoftCatalog;
pub const MATERIALIZE_FROMSOFT_CATALOG: PassSpec = PassSpec {
    name: "materialize-fromsoft-catalog",
    options: &[],
    make: |_| Ok(Box::new(MaterializeFromSoftCatalog)),
};
impl Pass for MaterializeFromSoftCatalog {
    fn name(&self) -> &'static str {
        "materialize-fromsoft-catalog"
    }
    fn requires(&self) -> &[&'static str] {
        &[
            Catalog::NAME,
            CatalogInputs::NAME,
            OutfitVariants::NAME,
            NpcOutfitVariants::NAME,
            WeaponInventory::NAME,
            SelectedWeaponForms::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Catalog::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[CatalogReady::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let input = world
            .resource::<CatalogInputs>()
            .ok_or("catalog inputs absent")?
            .clone();
        let players = world
            .resource::<OutfitVariants>()
            .ok_or("initializer presentations absent")?
            .0
            .clone();
        let npcs = world
            .resource::<NpcOutfitVariants>()
            .ok_or("NPC presentations absent")?
            .0
            .clone();
        if input.npc_rows.len() != npcs.len() {
            return Err("selected NPC rows do not preserve catalog identities".into());
        }
        let mut catalog = world.resource::<Catalog>().ok_or("catalog absent")?.clone();
        let mut npc_entries = Vec::new();
        for ((model, raw), outfit) in input.npc_rows.iter().zip(npcs) {
            if (raw.npc, raw.init, &raw.map, &raw.placement)
                != (outfit.npc, outfit.init, &outfit.map, &outfit.placement)
            {
                return Err("selected NPC row identity changed before catalog materialization".into());
            }
            let init = outfit
                .init
                .map_or_else(|| "none".into(), |value| value.to_string());
            let parent = catalog
                .entries
                .iter()
                .find(|entry| entry.kind == "character" && entry.name == *model)
                .map(|entry| entry.key.clone());
            let mut details = json!({"model":model,"outfit":outfit});
            if let Some(parent) = parent {
                details["parent"] = json!(parent);
            }
            npc_entries.push(CatalogEntry {
                key:format!("outfit/{model}/{init}/{}/{}",outfit.map,outfit.placement),
                kind:"outfit".into(),subject:Subject::Character,
                name:format!("{init} · {} / {}",outfit.map,outfit.placement),
                path:format!("{model} NPC {} outfit {init}",outfit.npc),
                refusal:outfit.parts.is_empty().then(||outfit.unresolved.join("; ")),
                open:if outfit.parts.is_empty() {Value::Null} else {json!({"reader":"character","id":model,"role":"npc","npc":outfit.npc.to_string(),"init":outfit.init.map(|value|value.to_string())})},
                details,
            });
        }
        if input.npc_insert > catalog.entries.len() || input.player_insert > input.npc_insert {
            return Err("catalog insertion identities changed".into());
        }
        catalog
            .entries
            .splice(input.npc_insert..input.npc_insert, npc_entries);
        if let Some(parent) = &input.player_parent {
            catalog.entries.splice(
                input.player_insert..input.player_insert,
                player_outfit_entries(parent, &players),
            );
        }
        let inventory = world
            .resource::<WeaponInventory>()
            .ok_or("weapon inventory absent")?;
        let forms = world
            .resource::<SelectedWeaponForms>()
            .ok_or("weapon forms not selected")?;
        if inventory.rows.len() != forms.0.len() {
            return Err("selected weapon rows changed catalog identity".into());
        }
        for (raw, (id, part)) in inventory.rows.iter().zip(&forms.0) {
            if raw.row.row != *id {
                return Err("selected weapon row order changed".into());
            }
            if raw.row.model == 0 {
                continue;
            }
            let Some(part) = part else { continue };
            catalog.entries.push(CatalogEntry {
                key: format!("weapon/{id}"),
                kind: "weapon".into(),
                subject: Subject::Object,
                refusal: None,
                name: id.to_string(),
                path: format!("EquipParamWeapon {id} {part}"),
                open: json!({"reader":"parts","weapons_right":[id.to_string()]}),
                details: json!({"part":part,"label":format!("{part} (EquipParamWeapon {id})")}),
            });
        }
        log.count("catalog entries materialized", catalog.entries.len() as u64);
        world.insert_resource(catalog);
        world.insert_resource(CatalogReady);
        Ok(())
    }
}
