//! The game's NPC names ([`fromsoft_formats::messages`]) as the `fromsoft.npc_names` resource.
use crate::invalid;
use fromsoft_formats::messages::npc_names;
use fromsoft_formats::{Archive, Install, NameDictionary, Result, dcx::Decoder};

const FORMAT: &str = "FromSoftware messages";

/// Read the authored NPC-row/FMG join into the World for name-fromsoft-entities.
/// This is explicit reader work: any error leaves the old table unchanged, and a
/// later pure naming pass neither reopens an install nor picks a different language.
pub fn npc_names_into_world(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    dictionary: &NameDictionary,
    language: &str,
    world: &mut primase_core::World,
) -> Result<usize> {
    let read = npc_names(install, archive, decoder, dictionary, language)?;
    let count = read.names.len();
    let names = crate::sekiro::character_data::NpcNames {
        game: install.game,
        language: language.into(),
        names: read.names,
        report: read.report,
    };
    primase_core::Resource::verify(&names, world).map_err(|reason| invalid(FORMAT, 0, reason))?;
    world.insert_resource(names);
    Ok(count)
}
