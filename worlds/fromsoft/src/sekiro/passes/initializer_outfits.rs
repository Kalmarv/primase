//! Presentation and level choices over the reader's complete initializer/protector candidate inventory.
use crate::sekiro::components::outfits::{CharacterOutfit, InitializerOutfits, OutfitGender, OutfitVariants};
use fromsoft_formats::Game;
use fromsoft_formats::equipment::{
    GENDER_BY_PRESENCE, LEVEL_SUFFIX_BY_PRESENCE, OutfitCandidates, PresenceChoice,
};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Resource, World};

/// Required level policy; `PreferLow` preserves the previous caller's stated preference and reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartLevel {
    PreferLow,
    Low,
    Full,
}
impl PartLevel {
    fn text(self) -> &'static str {
        match self {
            Self::PreferLow => "prefer-low",
            Self::Low => "low",
            Self::Full => "full",
        }
    }
}
/// Required presentation choice. `Authored` makes no additional sex/presentation filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Both,
    Male,
    Female,
    Authored,
}
impl Presentation {
    fn text(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Male => "m",
            Self::Female => "f",
            Self::Authored => "authored",
        }
    }
}
#[derive(Clone, Debug)]
pub struct SelectFromSoftInitializerOutfits {
    initializer: String,
    presentation: Presentation,
    level: PartLevel,
}
impl SelectFromSoftInitializerOutfits {
    pub fn new(initializer: &str, presentation: Presentation, level: PartLevel) -> Result<Self, String> {
        if !matches!(initializer, "all" | "first-readable") && initializer.parse::<i32>().is_err() {
            return Err("initializer is not a row id, all, or first-readable".into());
        }
        Ok(Self {
            initializer: initializer.to_owned(),
            presentation,
            level,
        })
    }
    fn from_given(g: &Given) -> Result<Self, String> {
        let presentation = match g.required("presentation") {
            "both" => Presentation::Both,
            "m" => Presentation::Male,
            "f" => Presentation::Female,
            "authored" => Presentation::Authored,
            _ => return Err("presentation is not both, m, f or authored".into()),
        };
        let level = match g.required("level") {
            "prefer-low" => PartLevel::PreferLow,
            "low" => PartLevel::Low,
            "full" => PartLevel::Full,
            _ => return Err("level is not prefer-low, low or full".into()),
        };
        Self::new(g.required("initializer"), presentation, level)
    }
}
pub const SELECT_FROMSOFT_INITIALIZER_OUTFITS: PassSpec = PassSpec {
    name: "select-fromsoft-initializer-outfits",
    options: &[
        OptionSpec::required(
            "initializer",
            "9000",
            "the stated initializer, all, or an explicit first-readable viewer choice",
        ),
        OptionSpec::required(
            "presentation",
            "both",
            "both, m, f or authored (no additional presentation filter)",
        ),
        OptionSpec::required(
            "level",
            "prefer-low",
            "low, full or explicit prefer-low fallback policy",
        ),
    ],
    make: |g| Ok(Box::new(SelectFromSoftInitializerOutfits::from_given(g)?)),
};

/// Result of the slot-selection mechanism, used only inside registered passes.
#[derive(Clone, Debug, Default)]
struct SelectedParts {
    parts: Vec<String>,
    unresolved: Vec<String>,
    chosen_by_presence: Vec<PresenceChoice>,
}
fn select_parts(
    game: Game,
    raw: &OutfitCandidates,
    presentation: Option<OutfitGender>,
    level: PartLevel,
) -> SelectedParts {
    let mut out = SelectedParts {
        unresolved: raw.unresolved.clone(),
        ..Default::default()
    };
    for slot in &raw.slots {
        let mut chosen = Vec::new();
        let mut suffix_choices = Vec::new();
        for gender in ["m", "f", "a"] {
            let fixed = game == Game::EldenRing && matches!(slot.gender_mode, 1 | 2 | 4);
            if !fixed && gender != "a" && presentation.is_some_and(|p| p.key() != gender) {
                continue;
            }
            let held: Vec<_> = slot
                .candidates
                .iter()
                .filter(|c| c.gender == gender && c.present)
                .collect();
            let picked = match level {
                PartLevel::PreferLow => held.iter().find(|c| c.low_suffix).or_else(|| held.first()),
                PartLevel::Low => held.iter().find(|c| c.low_suffix),
                PartLevel::Full => held.iter().find(|c| !c.low_suffix),
            };
            if let Some(picked) = picked {
                if held.len() > 1 || !picked.low_suffix {
                    suffix_choices.push(PresenceChoice {
                        reason: LEVEL_SUFFIX_BY_PRESENCE,
                        example: format!(
                            "protector {}: {} of {}",
                            slot.protector,
                            picked.name,
                            held.iter()
                                .map(|p| p.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
                chosen.push(picked.name.clone());
            }
        }
        chosen.sort();
        chosen.dedup();
        match chosen.as_slice() {
            [name] => {
                out.chosen_by_presence.extend(
                    suffix_choices
                        .into_iter()
                        .filter(|c| c.example.contains(&format!(": {name} of "))),
                );
                if slot.gender_by_presence {
                    out.chosen_by_presence.push(PresenceChoice {
                        reason: GENDER_BY_PRESENCE,
                        example: format!("protector {}: {name}", slot.protector),
                    });
                }
                if !out.parts.contains(name) {
                    out.parts.push(name.clone());
                }
            }
            [] => out.unresolved.push(format!(
                "protector {}: no {} part binder for model {}, gender mode {}",
                slot.protector, slot.prefix, slot.model, slot.gender_mode
            )),
            _ => out.unresolved.push(format!(
                "protector {}: ambiguous part binders {}",
                slot.protector,
                chosen.join(", ")
            )),
        }
    }
    out
}
impl Pass for SelectFromSoftInitializerOutfits {
    fn name(&self) -> &'static str {
        "select-fromsoft-initializer-outfits"
    }
    fn options(&self) -> Options {
        Options::from([
            ("initializer".into(), self.initializer.clone()),
            ("presentation".into(), self.presentation.text().into()),
            ("level".into(), self.level.text().into()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[InitializerOutfits::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[OutfitVariants::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[OutfitVariants::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let source = world
            .resource::<InitializerOutfits>()
            .ok_or("no initializer candidate inventory")?;
        let selected = self.initializer.parse::<i32>().ok();
        let mut variants: Vec<CharacterOutfit> = Vec::new();
        for row in &source.rows {
            if selected.is_some_and(|id| id != row.id) {
                continue;
            }
            let presentations: &[Option<OutfitGender>] = match self.presentation {
                Presentation::Both => &[Some(OutfitGender::Male), Some(OutfitGender::Female)],
                Presentation::Male => &[Some(OutfitGender::Male)],
                Presentation::Female => &[Some(OutfitGender::Female)],
                Presentation::Authored => &[None],
            };
            let mut row_variants: Vec<CharacterOutfit> = Vec::new();
            for &presentation in presentations {
                let mut picked = match &row.candidates {
                    Ok(raw) => select_parts(source.game, raw, presentation, self.level),
                    Err(why) => SelectedParts {
                        unresolved: vec![why.clone()],
                        ..Default::default()
                    },
                };
                if source.game == Game::EldenRing && picked.parts.is_empty() && picked.unresolved.is_empty() {
                    picked.unresolved.push(
                        "player body and face not drawn (character-creator meshes not in scope)".into(),
                    );
                }
                if row_variants
                    .iter()
                    .any(|v| v.parts == picked.parts && v.unresolved == picked.unresolved)
                {
                    continue;
                }
                row_variants.push(CharacterOutfit {
                    initializer: row.id,
                    name: row.name.clone(),
                    gender: presentation.unwrap_or(OutfitGender::Male),
                    animations_of: "c0000".into(),
                    parts: picked.parts,
                    unresolved: picked.unresolved,
                    chosen_by_presence: picked.chosen_by_presence,
                });
            }
            variants.extend(row_variants);
        }
        if selected.is_some() && variants.is_empty() {
            return Err(format!("CharaInitParam has no row {}", self.initializer));
        }
        if self.initializer == "first-readable" {
            let chosen = variants
                .into_iter()
                .find(CharacterOutfit::readable)
                .ok_or("no readable initializer outfit")?;
            log.count(
                "first readable initializer selected by the program (viewer choice, not a game default)",
                1,
            );
            variants = vec![chosen];
        }
        for variant in &variants {
            for choice in &variant.chosen_by_presence {
                log.approximate(choice.reason, 1);
            }
        }
        log.count("initializer presentations selected", variants.len() as u64);
        world.insert_resource(OutfitVariants(variants));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::components::outfits::InitializerEntry;
    use fromsoft_formats::param::Param;
    fn outfits_from_params(
        game: Game,
        initializers: &Param,
        protectors: &Param,
        has: impl Fn(&str) -> bool,
    ) -> Vec<CharacterOutfit> {
        let mut world = World::new();
        let source = InitializerOutfits {
            game,
            rows: initializers
                .rows
                .iter()
                .map(|row| InitializerEntry {
                    id: row.id,
                    name: row
                        .name
                        .clone()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| format!("CharaInitParam {}", row.id)),
                    candidates: fromsoft_formats::equipment::outfit_candidates(
                        game,
                        initializers,
                        protectors,
                        row.id,
                        &has,
                    )
                    .map_err(|why| why.to_string()),
                })
                .collect(),
        };
        world.insert_resource(source);
        let mut registry = primase_core::pass::PassRegistry::new();
        super::super::register(&mut registry);
        let pipeline = registry
            .parse("select-fromsoft-initializer-outfits{initializer=all,presentation=both,level=prefer-low}")
            .unwrap();
        primase_core::pass::PassManager::new(registry)
            .run(
                &mut world,
                &pipeline,
                &mut primase_core::pass::RunReport::default(),
            )
            .unwrap();
        world.resource::<OutfitVariants>().unwrap().0.clone()
    }

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
