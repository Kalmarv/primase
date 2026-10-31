//! Wearable character presentations resolved from the installed initializer and protector params.
//!
//! These are catalog choices, not a claim about which initializer a game selects at startup. A caller may choose
//! the first readable variant as a viewer policy and must describe that choice as its own.
use fromsoft_formats::equipment::{PresenceChoice, resolve_outfit};
use fromsoft_formats::game_params::param_binder;
use fromsoft_formats::param::Param;
use fromsoft_formats::{Archive, Game, Install, Result, dcx::Decoder};

use crate::invalid;

/// Which spelling the viewer permits where an initializer's protector permits both player presentations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum OutfitGender {
    Male,
    Female,
}

impl OutfitGender {
    pub fn key(self) -> &'static str {
        match self {
            Self::Male => "m",
            Self::Female => "f",
        }
    }
}

/// One installed `CharaInitParam` row, resolved through its `EquipParamProtector` rows and archive membership.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CharacterOutfit {
    pub initializer: i32,
    /// The stored param-row name, otherwise the param and row number. Community labels are not game data.
    pub name: String,
    pub gender: OutfitGender,
    /// The shared player animation rig of these games; no particular animation is chosen or decoded here.
    pub animations_of: String,
    pub parts: Vec<String>,
    pub unresolved: Vec<String>,
    pub chosen_by_presence: Vec<PresenceChoice>,
}

impl CharacterOutfit {
    pub fn key(&self) -> String {
        format!("initializer/{}/{}", self.initializer, self.gender.key())
    }

    pub fn readable(&self) -> bool {
        !self.parts.is_empty() && self.unresolved.is_empty()
    }
}

/// Installed initializer outfits of either game, in param-row order. Rows that do not resolve retain a reason;
/// readable male/female variants are distinct only where the archive actually supplies different parts.
pub fn character_outfits(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> Result<Vec<CharacterOutfit>> {
    let binder = param_binder(install, archive, decoder)?;
    let param = |name: &str| {
        let member = binder
            .member(name)
            .ok_or_else(|| invalid("character outfits", 0, format!("param binder has no {name}")))?;
        Param::read(&member.bytes)
    };
    let initializers = param("CharaInitParam.param")?;
    let protectors = param("EquipParamProtector.param")?;
    Ok(outfits_from_params(
        install.game,
        &initializers,
        &protectors,
        |part| archive.contains(&format!("/parts/{part}.partsbnd.dcx")),
    ))
}

/// Resolve stored param rows without choosing a game's default. Archive membership, including the explicitly
/// permitted player presentation, is supplied by `has`; field layouts and slot semantics stay in the format crate.
pub fn outfits_from_params(
    game: Game,
    initializers: &Param,
    protectors: &Param,
    has: impl Fn(&str) -> bool,
) -> Vec<CharacterOutfit> {
    let mut outfits = Vec::new();
    for row in &initializers.rows {
        let name = row
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("CharaInitParam {}", row.id));
        let mut variants: Vec<CharacterOutfit> = Vec::new();
        // ER's format resolver fixes the part letter for protector modes 1, 2 and 4. A viewer's presentation
        // choice must not reject those candidates (mode 4's shared parts are named `m`, including on a female
        // presentation). Only a mode that permits several spellings needs the additional presentation filter.
        let fixed_spelling: Vec<&str> = if game == Game::EldenRing {
            let layout = fromsoft_formats::equipment::OutfitLayout::of(game);
            [("hd", 32), ("bd", 36), ("am", 40), ("lg", 44)]
                .into_iter()
                .filter_map(|(prefix, at)| {
                    let bytes: [u8; 4] = row.data.get(at..at + 4)?.try_into().ok()?;
                    let equipped = i32::from_le_bytes(bytes);
                    let protector = protectors.rows.iter().find(|p| p.id == equipped)?;
                    matches!(protector.data.get(layout.gender), Some(1 | 2 | 4)).then_some(prefix)
                })
                .collect()
        } else {
            Vec::new()
        };
        for gender in [OutfitGender::Male, OutfitGender::Female] {
            let resolved = resolve_outfit(game, initializers, protectors, row.id, |part| {
                let spelling = part.split('_').nth(1);
                (fixed_spelling.contains(&part.split('_').next().unwrap_or(""))
                    || matches!(spelling, Some("a"))
                    || spelling == Some(gender.key()))
                    && has(part)
            });
            let (parts, unresolved, chosen_by_presence) = match resolved {
                Ok(parts) => (parts.parts, parts.unresolved, parts.chosen_by_presence),
                Err(error) => (Vec::new(), vec![error.to_string()], Vec::new()),
            };
            if variants
                .iter()
                .any(|existing| existing.parts == parts && existing.unresolved == unresolved)
            {
                continue;
            }
            variants.push(CharacterOutfit {
                initializer: row.id,
                name: name.clone(),
                gender,
                animations_of: "c0000".into(),
                parts,
                unresolved,
                chosen_by_presence,
            });
        }
        outfits.extend(variants);
    }
    outfits
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::equipment::OutfitLayout;
    use fromsoft_formats::param::Row;

    fn params(game: Game) -> (Param, Param) {
        let layout = OutfitLayout::of(game);
        let mut init = vec![0; layout.initializer.1];
        for at in [32, 36, 40, 44] {
            init[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
        }
        init[36..40].copy_from_slice(&79113i32.to_le_bytes());
        let mut protector = vec![0; layout.protector.1];
        protector[layout.model..layout.model + 2].copy_from_slice(&1379u16.to_le_bytes());
        protector[layout.category] = 2;
        protector[layout.gender] = if game == Game::Sekiro { 0 } else { 3 };
        let param = |name: &str, layout: (u16, usize), id, data| Param {
            param_type: name.into(),
            data_version: layout.0,
            unk06: 0,
            row_size: layout.1,
            rows: vec![Row {
                id,
                name: Some(String::new()),
                data,
            }],
        };
        (
            param("CHARACTER_INIT_PARAM", layout.initializer, 48112, init),
            param("EQUIP_PARAM_PROTECTOR_ST", layout.protector, 79113, protector),
        )
    }

    #[test]
    fn both_games_catalog_the_installed_rows_without_a_fixed_player_outfit() {
        for game in [Game::Sekiro, Game::EldenRing] {
            let (initializers, protectors) = params(game);
            let outfits = outfits_from_params(game, &initializers, &protectors, |part| {
                matches!(part, "bd_m_1379_l" | "bd_f_1379_l")
            });
            assert_eq!(outfits.len(), 2);
            assert!(outfits.iter().all(CharacterOutfit::readable));
            assert_eq!(outfits[0].initializer, 48112);
            assert_eq!(outfits[0].name, "CharaInitParam 48112");
            assert_eq!(outfits[0].parts, ["bd_m_1379_l"]);
            assert_eq!(outfits[1].parts, ["bd_f_1379_l"]);
            assert_eq!(outfits[0].animations_of, "c0000");
            assert_ne!(outfits[0].key(), outfits[1].key());
        }
    }

    #[test]
    fn missing_protectors_remain_named_refusals_and_do_not_create_parts() {
        let (initializers, mut protectors) = params(Game::Sekiro);
        protectors.rows.clear();
        let outfits = outfits_from_params(Game::Sekiro, &initializers, &protectors, |_| true);
        assert_eq!(outfits.len(), 1);
        assert!(!outfits[0].readable());
        assert!(outfits[0].parts.is_empty());
        assert!(
            outfits[0]
                .unresolved
                .iter()
                .any(|reason| reason.contains("79113"))
        );
    }

    #[test]
    fn a_split_presentation_keeps_the_protectors_shared_head_part() {
        let (mut initializers, mut protectors) = params(Game::EldenRing);
        let layout = OutfitLayout::of(Game::EldenRing);
        let mut head = protectors.rows[0].clone();
        head.id = 79110;
        head.data[layout.model..layout.model + 2].copy_from_slice(&2468u16.to_le_bytes());
        head.data[layout.category] = 5;
        head.data[layout.gender] = 4;
        protectors.rows.push(head);
        initializers.rows[0].data[32..36].copy_from_slice(&79110i32.to_le_bytes());
        let outfits = outfits_from_params(Game::EldenRing, &initializers, &protectors, |part| {
            matches!(part, "hd_m_2468_l" | "bd_m_1379_l" | "bd_f_1379_l")
        });
        assert_eq!(outfits.len(), 2);
        assert!(outfits.iter().all(CharacterOutfit::readable));
        assert_eq!(outfits[0].parts, ["hd_m_2468_l", "bd_m_1379_l"]);
        assert_eq!(outfits[1].parts, ["hd_m_2468_l", "bd_f_1379_l"]);
    }
}
