//! Materialize an authored catalog after registered presentation selectors have made their choices.
use primase_core::{Resource,World};
use primase_core::pass::{Pass,PassLog,PassSpec};
use serde_json::{json,Value};
use crate::sekiro::{catalog::{Catalog,CatalogInputs,CatalogReady,CatalogEntry,Subject,player_outfit_entries},
    outfit_data::{OutfitVariants,NpcOutfitVariants}};

#[derive(Clone,Copy,Debug)]
pub struct MaterializeFromSoftCatalog;
pub const MATERIALIZE_FROMSOFT_CATALOG:PassSpec=PassSpec {
    name:"materialize-fromsoft-catalog",options:&[],make:|_|Ok(Box::new(MaterializeFromSoftCatalog)),
};
impl Pass for MaterializeFromSoftCatalog {
    fn name(&self)->&'static str {"materialize-fromsoft-catalog"}
    fn requires(&self)->&[&'static str] {&[Catalog::NAME,CatalogInputs::NAME,OutfitVariants::NAME,NpcOutfitVariants::NAME]}
    fn modifies(&self)->&[&'static str] {&[Catalog::NAME]}
    fn provides(&self)->&[&'static str] {&[CatalogReady::NAME]}
    fn run(&self,world:&mut World,log:&mut PassLog)->Result<(),String> {
        let input=world.resource::<CatalogInputs>().ok_or("catalog inputs absent")?.clone();
        let players=world.resource::<OutfitVariants>().ok_or("initializer presentations absent")?.0.clone();
        let npcs=world.resource::<NpcOutfitVariants>().ok_or("NPC presentations absent")?.0.clone();
        if input.npc_rows.len()!=npcs.len() {return Err("selected NPC rows do not preserve catalog identities".into());}
        let mut catalog=world.resource::<Catalog>().ok_or("catalog absent")?.clone();
        let mut npc_entries=Vec::new();
        for ((model,raw),outfit) in input.npc_rows.iter().zip(npcs) {
            if (raw.npc,raw.init,&raw.map,&raw.placement)!=(outfit.npc,outfit.init,&outfit.map,&outfit.placement) {
                return Err("selected NPC row identity changed before catalog materialization".into());
            }
            let init=outfit.init.map_or_else(||"none".into(),|value|value.to_string());
            let parent=catalog.entries.iter().find(|entry|entry.kind=="character"&&entry.name==*model).map(|entry|entry.key.clone());
            let mut details=json!({"model":model,"outfit":outfit});
            if let Some(parent)=parent {details["parent"]=json!(parent);}
            npc_entries.push(CatalogEntry {
                key:format!("outfit/{model}/{init}/{}/{}",outfit.map,outfit.placement),
                kind:"outfit".into(),subject:Subject::Character,
                name:format!("{init} · {} / {}",outfit.map,outfit.placement),
                path:format!("{model} NPC {} outfit {init}",outfit.npc),
                refusal:outfit.parts.is_empty().then(||outfit.unresolved.join("; ")),
                open:if outfit.parts.is_empty() {Value::Null} else {json!({"reader":"character","id":model,"npc":outfit.npc.to_string(),"init":outfit.init.map(|value|value.to_string())})},
                details,
            });
        }
        if input.npc_insert>catalog.entries.len() || input.player_insert>input.npc_insert {return Err("catalog insertion identities changed".into());}
        catalog.entries.splice(input.npc_insert..input.npc_insert,npc_entries);
        if let Some(parent)=&input.player_parent {
            catalog.entries.splice(input.player_insert..input.player_insert,player_outfit_entries(parent,&players));
        }
        log.count("catalog entries materialized",catalog.entries.len() as u64);
        world.insert_resource(catalog); world.insert_resource(CatalogReady);
        Ok(())
    }
}
