//! Installed protector selection census, moved unchanged from the format crate beside its registered pass.
mod common;
mod inputs;
use fromsoft_formats::{
    Game, Install, Keys,
    dcx::Decoder,
    equipment::OutfitLayout,
    game_params::read_param,
    param::{Param, Row},
};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::World;
use primase_fromsoft::sekiro::outfit_data::InitializerEntry;
use primase_fromsoft::sekiro::{CharacterOutfit, InitializerOutfits, OutfitVariants};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn configured(game: Game) -> Option<(Install, inputs::OodleLibrary)> {
    let data = match game {
        Game::Sekiro => Data::Sekiro,
        Game::EldenRing => Data::Elden,
    };
    let (root, folder, dll) = (install(data)?, keys(data)?, oodle(Tool::Oodle6)?);
    let keys = Keys::from_folder(folder, game, None).ok()?;
    Some((Install::new(root, game, keys), dll))
}

fn resolve_outfit(
    game: Game,
    initializers: &Param,
    protectors: &Param,
    id: i32,
    has: impl Fn(&str) -> bool,
) -> std::result::Result<CharacterOutfit, String> {
    let candidates = fromsoft_formats::equipment::outfit_candidates(game, initializers, protectors, id, has)
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
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let pipeline = registry
        .parse(&format!(
            "select-fromsoft-initializer-outfits{{initializer={id},presentation=authored,level=prefer-low}}"
        ))
        .map_err(|why| why.to_string())?;
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

/// A copy of the initializer row `template` with its slot `slot` (0 head .. 3 legs) naming `protector`, the others -1.
fn initializer_naming(template: &Row, slot: usize, protector: i32) -> Row {
    let mut data = template.data.clone();
    for at in [32, 36, 40, 44] {
        data[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
    }
    data[32 + 4 * slot..36 + 4 * slot].copy_from_slice(&protector.to_le_bytes());
    Row {
        id: template.id,
        name: None,
        data,
    }
}

#[test]
fn elden_rings_protectors_find_their_part_binders() -> Result<()> {
    let game = Game::EldenRing;
    let Some((install, dll)) = configured(game) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let archive = install.open()?;
    let initializers = read_param(&install, &archive, &decoder, "CharaInitParam.param")?;
    let protectors = read_param(&install, &archive, &decoder, "EquipParamProtector.param")?;
    let layout = OutfitLayout::of(game);
    assert_eq!(
        (
            (initializers.data_version, initializers.row_size),
            (protectors.data_version, protectors.row_size)
        ),
        (layout.initializer, layout.protector)
    );
    assert_eq!((initializers.rows.len(), protectors.rows.len()), (3273, 838));
    let has = |name: &str| archive.contains(&format!("/parts/{name}.partsbnd.dcx"));

    // the initializers: 13,092 slots, 1,147 unset, 65 naming no protector row, 11,880 resolved to one of the four
    // categories their slot takes; each resolves or says why, and the unset slots say nothing
    let by_id: std::collections::BTreeMap<i32, &Row> = protectors.rows.iter().map(|r| (r.id, r)).collect();
    let (mut unset, mut dangling, mut named) = (0, 0, 0);
    let mut slot_categories = [[0usize; 8]; 4];
    for init in &initializers.rows {
        for (slot, at) in [32, 36, 40, 44].into_iter().enumerate() {
            let id = i32::from_le_bytes(init.data[at..at + 4].try_into().unwrap());
            match (id, by_id.get(&id)) {
                (id, _) if id < 0 => unset += 1,
                (_, None) => dangling += 1,
                (_, Some(p)) => {
                    named += 1;
                    slot_categories[slot][usize::from(p.data[layout.category])] += 1;
                }
            }
        }
    }
    assert_eq!((unset, dangling, named), (1147, 65, 11880));
    assert_eq!(
        slot_categories.map(|c| {
            c.iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(category, n)| (category, *n))
                .collect::<Vec<_>>()
        }),
        [vec![(5, 2898)], vec![(2, 3041)], vec![(1, 2919)], vec![(6, 3022)],]
    );
    let mut dangling_reported = 0;
    for init in &initializers.rows {
        let parts = resolve_outfit(game, &initializers, &protectors, init.id, has)?;
        dangling_reported += parts
            .unresolved
            .iter()
            .filter(|w| w.contains("has no such row"))
            .count();
        assert!(
            parts.unresolved.iter().all(|w| !w.contains("is not a ")),
            "init {}: a protector of another category in a slot: {:?}",
            init.id,
            parts.unresolved
        );
    }
    assert_eq!(dangling_reported, 65);

    // the protectors one by one: each of the 793 rows with a model id and a category of head, body, arms or legs
    // (1, 2, 5, 6) is put alone in its category's slot of a copy of an initializer row, and the part is resolved
    let template = &initializers.rows[0];
    let slot_of = |category: u8| match category {
        5 => Some(0),
        2 => Some(1),
        1 => Some(2),
        6 => Some(3),
        _ => None,
    };
    let (mut rows, mut resolved, mut ambiguous, mut absent) = (0, 0, 0, 0);
    let mut by_gender = std::collections::BTreeMap::<u8, [usize; 3]>::new();
    for p in &protectors.rows {
        let model = u16::from_le_bytes(p.data[layout.model..layout.model + 2].try_into().unwrap());
        let (category, gender) = (p.data[layout.category], p.data[layout.gender]);
        let Some(slot) = slot_of(category).filter(|_| model != 0) else {
            continue;
        };
        rows += 1;
        let one = Param {
            rows: vec![initializer_naming(template, slot, p.id)],
            ..initializers.clone()
        };
        let parts = resolve_outfit(game, &one, &protectors, template.id, has)?;
        // the independent expectation: which letters hold a binder for the category's prefix
        let prefix = ["hd", "bd", "am", "lg"][slot];
        let held: Vec<char> = ['m', 'f', 'a']
            .into_iter()
            .filter(|l| {
                has(&format!("{prefix}_{l}_{model:04}_l")) || has(&format!("{prefix}_{l}_{model:04}"))
            })
            .collect();
        assert!(!held.contains(&'a'), "protector {}: an `a` binder", p.id);
        let tally = by_gender.entry(gender).or_default();
        if !parts.parts.is_empty() {
            resolved += 1;
            tally[0] += 1;
            assert_eq!(parts.parts.len(), 1, "protector {}", p.id);
            let letter = parts.parts[0].split('_').nth(1).unwrap().chars().next().unwrap();
            assert!(held.contains(&letter), "protector {}: {}", p.id, parts.parts[0]);
            // gender 4 (none) and 1 (male) are `m`, 2 (female) `f`
            match gender {
                1 | 4 => assert_eq!(letter, 'm', "protector {}", p.id),
                2 => assert_eq!(letter, 'f', "protector {}", p.id),
                _ => {}
            }
        } else if parts.unresolved.iter().any(|w| w.contains("ambiguous")) {
            ambiguous += 1;
            tally[1] += 1;
            assert_eq!((gender, held.len()), (3, 2), "protector {}: ambiguous", p.id);
        } else {
            absent += 1;
            tally[2] += 1;
            assert!(
                parts.unresolved.iter().any(|w| w.contains("part binder")),
                "{:?}",
                parts.unresolved
            );
        }
    }
    println!("by gender [resolved, ambiguous, no binder]: {by_gender:?}");
    // 793 rows; 789 find the binder of their category's prefix, 4 find none (the `_l` and plain spellings agree in all)
    assert_eq!(rows, 793);
    assert_eq!((resolved + ambiguous, absent), (789, 4));
    Ok(())
}
