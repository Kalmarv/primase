//! The game's rule for the clip a selector of the player's graph plays (`sekiro::attack_clip`), on Elden Ring's data and one
//! save (`oracles.saves.elden/ER0000.sl2`, a copy of a save whose character holds the Uchigatana in both hands in the
//! right): the save's state, the selectors of `c0000.hkb` by offset type, and the clips the rule chooses. The two-handed
//! first light attack (`AttackBothLight1_CMSG`, offset type 16, animId 32000) is `a029_032000`, the clip a run of the game
//! played (the model-space joint positions of every attack frame match it, at 1.0 to 3.2 cm mean, against 13 cm for the
//! one-handed `a029_030000`). The one-handed request is refused for this hold; for the same weapons held in one hand it is
//! `a029_030000`.
//!
//! Needs `installs.elden`, `keys.elden`, `tools.oodle6` and `oracles.saves`; a run without one skips.
mod common;
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use fromsoft_formats::save::{ArmStyle, Save};
use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_core::{ClipKey, Name, convert::Reader};
use primase_fromsoft::sekiro::attack_clip::{self, Hand, PlayerState, Selector};
use primase_fromsoft::sekiro::passes::weapons::Grip;
use primase_fromsoft::sekiro::{CharacterReader, WeaponParams};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn the_rule_chooses_the_clip_the_game_played_for_the_saved_hold() -> Result<()> {
    let (Some(root), Some(folder), Some(library), Some(save)) = (
        install(Data::Elden),
        keys(Data::Elden),
        oodle(Tool::Oodle6),
        oracle(OracleSet::Saves, Data::Elden, "ER0000.sl2"),
    ) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let installed = Install::new(root, Game::EldenRing, Keys::from_folder(folder, Game::EldenRing, None)?);
    let archive = installed.open()?;

    // the save's state: arm style 3, the right hand's slot 0 active and holding the Uchigatana
    let save = Save::read(&std::fs::read(save)?)?;
    let player = PlayerState::from_save(save.slot(0).ok_or("no slot 0")?)?;
    assert_eq!(player.arm_style, ArmStyle::RightBoth);
    assert_eq!(player.grip(), Grip::TwoHandedRight);
    assert_eq!(
        (player.active_weapon(Hand::Right), player.active_weapon(Hand::Left)),
        (Some(9_000_000), Some(41_000_000))
    );

    // the selectors of the player's graph, by offset type: 478 of the 4,051 are of the three types the rule reads
    let selectors = attack_clip::read_selectors(&archive, &decoder, "c0000")?;
    let mut by_type: BTreeMap<(i64, Option<&str>), usize> = BTreeMap::new();
    for s in &selectors {
        *by_type.entry((s.offset_type, s.offset_name)).or_default() += 1;
    }
    assert_eq!(
        by_type,
        BTreeMap::from([
            ((0, Some("OffsetNone")), 34),
            ((11, Some("IdleCategory")), 2874),
            ((13, Some("WeaponCategoryRight")), 183),
            ((14, Some("WeaponCategoryLeft")), 62),
            ((15, Some("AnimIdOffset")), 58),
            ((16, Some("WeaponCategoryHandStyle")), 233),
            ((17, Some("MagicCategory")), 194),
            ((18, Some("SwordArtsCategory")), 413),
        ])
    );
    let both = Selector::named(selectors.clone(), "AttackBothLight1_CMSG")?;
    let right = Selector::named(selectors, "AttackRightLight1_CMSG")?;
    assert_eq!(
        (both.offset_type, both.offset_name, both.anim_id),
        (16, Some("WeaponCategoryHandStyle"), 32000)
    );
    assert_eq!(
        (right.offset_type, right.offset_name, right.anim_id),
        (13, Some("WeaponCategoryRight"), 30000)
    );

    // the character's animations, as its binders list them
    let mut reader = CharacterReader::animations(installed.clone(), "c0000", decoder);
    let (plan, _) = reader.read().map_err(|error| error.to_string())?;
    let animations: BTreeSet<String> = plan
        .iter::<ClipKey>()
        .filter_map(|(e, _)| plan.get::<Name>(e).map(|n| n.0.clone()))
        .collect();
    let params = WeaponParams::read(&installed, &archive, &decoder)?;

    attack_clip::check_request(&both.name, player.arm_style)?;
    let choice = attack_clip::choose(&both, &player, &params, &animations)?;
    println!("{}", choice.steps.join("\n"));
    assert_eq!(
        (choice.hand, choice.row, choice.category, choice.sp_attack, choice.id),
        (Hand::Right, 9_000_000, 29, 0, 29_032_000)
    );
    assert_eq!(choice.animation, "a029_032000");
    assert_eq!(both.children.get(choice.child).cloned().flatten().as_deref(), Some("a029_032000"));

    // the one-handed request contradicts this hold; for the same weapons in one hand it plays a029_030000
    assert!(attack_clip::check_request(&right.name, player.arm_style).is_err());
    let one_handed = PlayerState {
        arm_style: ArmStyle::Right,
        ..player
    };
    assert_eq!(attack_clip::choose(&right, &one_handed, &params, &animations)?.animation, "a029_030000");
    Ok(())
}
