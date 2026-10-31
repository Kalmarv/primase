//! Presentation and level choices over the reader's complete initializer/protector candidate inventory.
use crate::sekiro::components::outfits::{CharacterOutfit, InitializerOutfits, OutfitGender, OutfitVariants};
use crate::sekiro::components::outfits::{NpcOutfitRows, NpcOutfitVariants, SelectedNpcOutfit};
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

/// Apply a stated level policy to map-authored NPC candidates; source modes determine the permitted gender spellings.
#[derive(Clone, Copy, Debug)]
pub struct SelectFromSoftNpcOutfits(pub PartLevel);
pub const SELECT_FROMSOFT_NPC_OUTFITS: PassSpec = PassSpec {
    name: "select-fromsoft-npc-outfits",
    options: &[OptionSpec::required(
        "level",
        "prefer-low",
        "low, full or explicit prefer-low fallback policy",
    )],
    make: |g| {
        let level = match g.required("level") {
            "prefer-low" => PartLevel::PreferLow,
            "low" => PartLevel::Low,
            "full" => PartLevel::Full,
            _ => return Err("level is not prefer-low, low or full".into()),
        };
        Ok(Box::new(SelectFromSoftNpcOutfits(level)))
    },
};
impl Pass for SelectFromSoftNpcOutfits {
    fn name(&self) -> &'static str {
        "select-fromsoft-npc-outfits"
    }
    fn options(&self) -> Options {
        Options::from([("level".into(), self.0.text().into())])
    }
    fn requires(&self) -> &[&'static str] {
        &[NpcOutfitRows::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[NpcOutfitVariants::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[NpcOutfitVariants::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let source = world
            .resource::<NpcOutfitRows>()
            .ok_or("no NPC outfit candidates")?;
        let variants = source
            .rows
            .iter()
            .map(|row| {
                let mut chosen = row
                    .candidates
                    .as_ref()
                    .map(|raw| select_parts(source.game, raw, None, self.0))
                    .unwrap_or_default();
                chosen.unresolved.extend(row.unresolved.iter().cloned());
                for choice in &chosen.chosen_by_presence {
                    log.approximate(choice.reason, 1);
                }
                SelectedNpcOutfit {
                    npc: row.npc,
                    init: row.init,
                    map: row.map.clone(),
                    placement: row.placement.clone(),
                    parts: chosen.parts,
                    unresolved: chosen.unresolved,
                    chosen_by_presence: chosen.chosen_by_presence,
                }
            })
            .collect::<Vec<_>>();
        log.count("NPC outfit presentations selected", variants.len() as u64);
        world.insert_resource(NpcOutfitVariants(variants));
        Ok(())
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
        &[
            OutfitVariants::NAME,
            primase_core::opening::OpeningSuggestions::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            OutfitVariants::NAME,
            primase_core::opening::OpeningSuggestions::NAME,
        ]
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
        let suggestions = variants
            .iter()
            .filter(|outfit| outfit.readable())
            .map(|outfit| primase_core::opening::OpeningSuggestion {
                label: format!("Open {} with its parts", outfit.name),
                reasons: vec!["The character's drawable body is an installed initializer outfit.".into()],
                reader: "parts".into(),
                options: serde_json::json!({"parts":outfit.parts,"animations_of":outfit.animations_of}),
                inherit: [
                    "game",
                    "install",
                    "keys",
                    "names",
                    "bake_layers",
                    "preview_layers",
                    "game_shaders",
                    "clips",
                    "rate",
                    "absent",
                    "root_motion",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            })
            .collect();
        world.insert_resource(primase_core::opening::OpeningSuggestions(suggestions));
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

#[cfg(test)]
mod legacy_format_selection_tests {
    use super::*;
    use crate::sekiro::components::outfits::InitializerEntry;
    use fromsoft_formats::{
        equipment::OutfitLayout,
        param::{Param, Row},
    };
    const SEX: usize = 228;

    type OutfitParts = CharacterOutfit;
    fn resolve_outfit(
        game: Game,
        initializers: &Param,
        protectors: &Param,
        id: i32,
        has: impl Fn(&str) -> bool,
    ) -> Result<CharacterOutfit, String> {
        let candidates =
            fromsoft_formats::equipment::outfit_candidates(game, initializers, protectors, id, has)
                .map_err(|why| why.to_string())?;
        let mut world = World::new();
        world.insert_resource(InitializerOutfits {
            game,
            rows: vec![InitializerEntry {
                id,
                name: format!("CharaInitParam {id}"),
                candidates: Ok(candidates),
            }],
        });
        let mut registry = primase_core::pass::PassRegistry::new();
        crate::sekiro::passes::register(&mut registry);
        let pipeline = registry.parse(&format!("select-fromsoft-initializer-outfits{{initializer={id},presentation=authored,level=prefer-low}}" )).map_err(|why| why.to_string())?;
        primase_core::pass::PassManager::new(registry)
            .run(
                &mut world,
                &pipeline,
                &mut primase_core::pass::RunReport::default(),
            )
            .map_err(|why| why.to_string())?;
        world
            .resource::<OutfitVariants>()
            .and_then(|variants| variants.0.first())
            .cloned()
            .ok_or_else(|| "no selected outfit".into())
    }
    /// A one-row param of `game`'s layout: `CHARACTER_INIT_PARAM` or `EQUIP_PARAM_PROTECTOR_ST`.
    fn param(game: Game, init: bool, id: i32, data: impl FnOnce(&mut Vec<u8>)) -> Param {
        let l = OutfitLayout::of(game);
        let ((data_version, row_size), param_type) = if init {
            (l.initializer, "CHARACTER_INIT_PARAM")
        } else {
            (l.protector, "EQUIP_PARAM_PROTECTOR_ST")
        };
        let mut bytes = vec![0; row_size];
        data(&mut bytes);
        Param {
            param_type: param_type.into(),
            data_version,
            unk06: 1,
            row_size,
            rows: vec![Row {
                id,
                name: None,
                data: bytes,
            }],
        }
    }

    /// An initializer with `slot` (0 head .. 3 legs) naming `protector` and the other slots unset.
    fn initializer(game: Game, id: i32, sex: u8, slot: usize, protector: i32) -> Param {
        param(game, true, id, |d| {
            for at in [32, 36, 40, 44] {
                d[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
            }
            d[32 + 4 * slot..36 + 4 * slot].copy_from_slice(&protector.to_le_bytes());
            d[SEX] = sex;
        })
    }

    /// A protector of `model`, `category` and `gender` in `game`'s layout.
    fn protector(game: Game, id: i32, model: u16, category: u8, gender: u8) -> Param {
        let l = OutfitLayout::of(game);
        param(game, false, id, |d| {
            d[l.model..l.model + 2].copy_from_slice(&model.to_le_bytes());
            d[l.category] = category;
            d[l.gender] = gender;
        })
    }

    fn reasons(parts: &OutfitParts) -> Vec<&'static str> {
        parts.chosen_by_presence.iter().map(|c| c.reason).collect()
    }

    #[test]
    fn initializer_equipment_follows_protector_model_not_npc_or_equipment_id() {
        let g = Game::Sekiro;
        let initializers = initializer(g, 9000, 0, 0, 100000);
        // the Wolf's head: category 3 is the face, gender mode 3 takes the initializer's sex
        let protectors = protector(g, 100000, 200, 3, 3);
        let r = resolve_outfit(g, &initializers, &protectors, 9000, |p| p == "fc_m_0200_l").expect("outfit");
        assert_eq!(r.parts, ["fc_m_0200_l"]);
        assert!(r.unresolved.is_empty());
        assert!(reasons(&r).is_empty());
        assert!(resolve_outfit(g, &initializers, &protectors, 10002020, |_| true).is_err());
        // a head part (category 5) is `hd_`, not the face, even where the face binder exists
        let head = protector(g, 100000, 200, 5, 3);
        let r = resolve_outfit(g, &initializers, &head, 9000, |p| {
            matches!(p, "fc_m_0200_l" | "hd_m_0200_l")
        })
        .expect("outfit");
        assert_eq!(r.parts, ["hd_m_0200_l"]);
    }

    #[test]
    fn ambiguous_or_missing_part_bindings_are_explained_without_inventing_a_model() {
        let g = Game::Sekiro;
        let initializers = initializer(g, 7, 0, 1, 1);
        let protectors = protector(g, 1, 0, 2, 0);
        let r = resolve_outfit(g, &initializers, &protectors, 7, |_| true).expect("fields");
        assert!(r.parts.is_empty());
        assert!(r.unresolved.iter().any(|w| w.contains("ambiguous")));
        let r = resolve_outfit(g, &initializers, &protectors, 7, |_| false).expect("fields");
        assert!(r.parts.is_empty());
        assert!(r.unresolved.iter().any(|w| w.contains("no bd part")));
    }

    #[test]
    fn unspecified_sekiro_gender_follows_unique_held_binder_not_npc_player_sex() {
        let g = Game::Sekiro;
        let initializers = initializer(g, 15030, 1, 1, 101000);
        let protectors = protector(g, 101000, 9040, 2, 1);
        let r = resolve_outfit(g, &initializers, &protectors, 15030, |name| name == "bd_m_9040_l")
            .expect("fields");
        assert_eq!(r.parts, ["bd_m_9040_l"]);
        assert!(r.unresolved.is_empty());
        assert_eq!(reasons(&r), [GENDER_BY_PRESENCE]);
        let r = resolve_outfit(g, &initializers, &protectors, 15030, |name| {
            matches!(name, "bd_m_9040_l" | "bd_f_9040_l")
        })
        .expect("fields");
        assert!(r.parts.is_empty());
        assert!(r.unresolved.iter().any(|s| s.contains("ambiguous")));
    }

    #[test]
    fn a_protector_in_a_slot_of_another_category_is_refused() {
        for (g, wrong) in [
            (Game::Sekiro, 0),
            (Game::Sekiro, 5),
            (Game::EldenRing, 4),
            (Game::EldenRing, 5),
        ] {
            // a body slot (1) naming a protector of another category: no part, the reason named
            let initializers = initializer(g, 1, 0, 1, 10);
            let protectors = protector(g, 10, 100, wrong, 4);
            let r = resolve_outfit(g, &initializers, &protectors, 1, |_| true).expect("fields");
            assert!(r.parts.is_empty(), "{g:?} {wrong}");
            assert!(
                r.unresolved
                    .iter()
                    .any(|w| w.contains(&format!("equipModelCategory {wrong} is not a bd part"))),
                "{:?}",
                r.unresolved
            );
        }
    }

    #[test]
    fn elden_rings_part_letter_follows_the_gender_mode_and_never_the_shared_a() {
        let g = Game::EldenRing;
        // head slot, category 5 -> hd; gender 4 (none) and 1 (male) are `m`, 2 (female) is `f`
        for (gender, letter) in [(4u8, "m"), (1, "m"), (2, "f")] {
            let (init, prot) = (initializer(g, 1, 1, 0, 10), protector(g, 10, 7, 5, gender));
            let name = format!("hd_{letter}_0007_l");
            let r = resolve_outfit(g, &init, &prot, 1, |n| n == name || n == "hd_a_0007_l").expect("fields");
            assert_eq!(r.parts, [name], "gender {gender}");
            assert!(reasons(&r).is_empty());
        }
        // the plain name is taken when the `_l` sibling is not held, and said
        let (init, prot) = (initializer(g, 1, 0, 3, 10), protector(g, 10, 7, 6, 4));
        let r = resolve_outfit(g, &init, &prot, 1, |n| n == "lg_m_0007").expect("fields");
        assert_eq!(r.parts, ["lg_m_0007"]);
        assert_eq!(reasons(&r), [LEVEL_SUFFIX_BY_PRESENCE]);
        // gender 3 splits by a sex whose letter is not known: the one held binder, or no choice
        let (init, prot) = (initializer(g, 1, 0, 2, 10), protector(g, 10, 7, 1, 3));
        let r = resolve_outfit(g, &init, &prot, 1, |n| n == "am_f_0007_l").expect("fields");
        assert_eq!(r.parts, ["am_f_0007_l"]);
        assert_eq!(reasons(&r), [GENDER_BY_PRESENCE]);
        let r = resolve_outfit(g, &init, &prot, 1, |n| matches!(n, "am_f_0007_l" | "am_m_0007_l"))
            .expect("fields");
        assert!(r.parts.is_empty() && r.unresolved.iter().any(|w| w.contains("ambiguous")));
        // an unknown gender mode is reported, not guessed
        let (init, prot) = (initializer(g, 1, 0, 0, 10), protector(g, 10, 7, 5, 9));
        let r = resolve_outfit(g, &init, &prot, 1, |_| true).expect("fields");
        assert!(
            r.parts.is_empty()
                && r.unresolved
                    .iter()
                    .any(|w| w.contains("unknown equipModelGender 9"))
        );
    }

    #[test]
    fn one_games_params_are_not_read_with_the_others_offsets() {
        let (init, prot) = (
            initializer(Game::Sekiro, 1, 0, 0, 10),
            protector(Game::Sekiro, 10, 7, 5, 4),
        );
        let e = resolve_outfit(Game::EldenRing, &init, &prot, 1, |_| true)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Elden Ring"), "{e}");
    }
}
