//! What the area and character readers make of the part-built NPCs' outfits ([`fromsoft_formats::equipment`]): the
//! choices the outfits made by presence as the read's notices, why a character has no model, and the skeletons of a
//! character's models.
use crate::invalid;
use fromsoft_formats::equipment::{NO_INITIALIZER, NpcOutfit};
use fromsoft_formats::flver::Model;
use fromsoft_formats::{Archive, Binder, Game, Result, dcx::Decoder};
use primase_core::convert::{Notice, NoticeKind};
use std::collections::BTreeMap;

/// What the outfits' choices by presence are, as the read's notices: each reason once, counted over the outfits, with a
/// few examples (the placement and the part). A program that wants the outfits to be exact sees here which are not.
pub fn outfit_notices(stage: &str, outfits: &[NpcOutfit]) -> Vec<Notice> {
    let mut by_reason: BTreeMap<&'static str, (u64, Vec<String>)> = BTreeMap::new();
    let mut unnamed: Option<(u64, Vec<String>)> = None;
    for outfit in outfits {
        if outfit.init.is_none() {
            let entry = unnamed.get_or_insert((0, Vec::new()));
            entry.0 += 1;
            entry.1.push(format!(
                "{} {} (NPC row {})",
                outfit.map, outfit.placement, outfit.npc
            ));
        }
        for choice in &outfit.chosen_by_presence {
            let entry = by_reason.entry(choice.reason).or_default();
            entry.0 += 1;
            entry
                .1
                .push(format!("{} {}: {}", outfit.map, outfit.placement, choice.example));
        }
    }
    by_reason
        .into_iter()
        .map(|(reason, found)| (reason, found, NoticeKind::Selection))
        .chain(unnamed.map(|found| (NO_INITIALIZER, found, NoticeKind::Unsupported)))
        .map(|(reason, (count, examples), kind)| {
            examples
                .into_iter()
                .fold(Notice::new(stage, kind, reason, count), |n, e| n.example(e))
        })
        .collect()
}

/// Characters that SDT's character alias list names "Invisible" placeholders (`interactable_placeholder`), by game:
/// a named table of ids to a purpose, the only source of the label. It never overrides an actual FLVER or an
/// MSB-authored outfit, and an id not in it is reported as the general miss.
const PLACEHOLDER_CHARACTERS: &[(Game, &str)] = &[(Game::Sekiro, "c1000"), (Game::Sekiro, "c1001")];

/// Why a binder already observed to have no drawable geometry and no resolved parts has none: a placeholder the
/// table above names, else the general miss (source geometry remains an unresolved miss).
pub fn empty_character_reason(game: Game, id: &str) -> &'static str {
    if PLACEHOLDER_CHARACTERS.contains(&(game, id)) {
        "an invisible placeholder character: no model (no authored equipment parts resolved)"
    } else {
        "no drawable character model or resolved part outfit; source geometry remains an unresolved miss"
    }
}

/// Model skeletons only, for pair validation before a program decides whether to read a character's geometry.
/// Names identify the FLVER member; several members stay separate. No material, texture or animation is read.
pub fn character_skeletons(
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
) -> Result<Vec<(String, primase_core::Skeleton)>> {
    if id.len() != 5 || !id.starts_with('c') || !id[1..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("character skeleton", 0, "expected c and four digits"));
    }
    let path = format!("/chr/{id}.chrbnd.dcx");
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let mut out = Vec::new();
    for file in &binder.files {
        if let Some(name) = file
            .name
            .as_deref()
            .filter(|n| n.to_ascii_lowercase().ends_with(".flver"))
        {
            let model = Model::read(&decoder.decode_if_dcx(&file.bytes)?)?;
            out.push((name.to_owned(), crate::pose::skeleton(&model)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::equipment::{GENDER_BY_PRESENCE, PresenceChoice};

    /// Each reason an outfit chose a part by presence is one notice, counted over the outfits.
    #[test]
    fn presence_choices_are_one_notice_a_reason_counted_over_the_outfits() {
        let outfit = NpcOutfit {
            npc: 1,
            init: Some(15030),
            map: "m".into(),
            placement: "p".into(),
            parts: vec!["bd_m_9040_l".into()],
            unresolved: Vec::new(),
            chosen_by_presence: vec![PresenceChoice {
                reason: GENDER_BY_PRESENCE,
                example: "protector 101000: bd_m_9040_l".into(),
            }],
        };
        let named = outfit_notices("stage", &[outfit.clone(), outfit]);
        assert_eq!(named.len(), 1);
        assert_eq!(
            (named[0].reason.as_str(), named[0].count),
            (GENDER_BY_PRESENCE, 2)
        );
    }
}
