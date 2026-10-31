//! Which animation an Elden Ring `CustomManualSelectorGenerator` of the player's behaviour graph plays, by the game's
//! rule, from the player's state ([`PlayerState`]: the weapons in each hand's slots, each hand's active slot and the arm
//! style, as a save states them) and the install's data (the selector's `offsetType`, `animId` and children, the held
//! weapon's `EquipParamWeapon` row, the character's animations).
//!
//! The game (2.7.2.0's executable) computes an animation id `offset + animId` and plays the selector's child whose
//! animation is named `a<id / 1000000, 3 digits>_<id % 1000000, 6 digits>`; with no such child, the one named
//! `a000_<animId>`; with neither, child 0. For the three offset types this module reads, the offset comes from one hand's
//! weapon:
//!
//! - `WeaponCategoryRight` (13): the right hand; `WeaponCategoryLeft` (14): the left hand;
//! - `WeaponCategoryHandStyle` (16): the left hand when the arm style is `LeftBoth`, else the right.
//!
//! The hand's weapon is the item in its active slot; its row is the item id less its upgrade level (the last two
//! digits), as the game's own lookup drops them. With `category` the row's `wepmotionCategory` and `sp` its
//! `spAtkcategory` (the u16 at 238 of a 664-byte row, the offset the game reads), the id is the first of these the
//! character has an animation for:
//!
//! 1. `sp x 1000000 + animId`, where `sp` is not 0;
//! 2. `category x 1000000 + animId`;
//! 3. `23000000 + animId` for the right hand, `48000000 + animId` for the left;
//!
//! and `category x 1000000 + animId` where it has none of them. The game asks its own table of the character's
//! animations whether one exists; the read asks the animations the character's binders list, which is what that table is
//! taken to hold. That the category the game reads is the hand's weapon's `wepmotionCategory` is its writer's reading:
//! the array the choice reads it from is filled from that field per hand, and the code that loads the array is not
//! followed. The other offset types (`OffsetNone` 0, `IdleCategory` 11, `AnimIdOffset` 15, `MagicCategory` 17,
//! `SwordArtsCategory` 18 and any unnamed value) read other state: `select-fromsoft-attack` refuses them by name.
//!
//! Which selector plays is the player's request (an attack button and the arm style, in the player's behaviour script's
//! `GetAttackRequest`, which is not read): it is stated, and `select-fromsoft-attack` refuses one the arm style contradicts.
//! A one-handed light attack is under `AttackRight`, a two-handed one under `AttackBoth`.
use crate::sekiro::components::attack_clip::{
    AttackInputs, AttackSelection, Choice, Hand, PlayerState, Selector,
};
use crate::sekiro::effect_data::EquippedWeapons;
use fromsoft_formats::save::ArmStyle;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{ClipKey, Component, Name, Resource, World};
use std::collections::BTreeSet;

/// A program's selector request and explicit hold. No weapon, active slot or arm style is implicit.
#[derive(Clone, Debug)]
pub struct SelectFromSoftAttack {
    selector: String,
    player: PlayerState,
}
impl SelectFromSoftAttack {
    pub fn new(selector: &str, player: PlayerState) -> Self {
        Self {
            selector: selector.to_owned(),
            player,
        }
    }
    fn from_given(g: &Given) -> Result<Self, String> {
        let slots = |text: &str| -> Result<[Option<i32>; 4], String> {
            let values: Vec<_> = text
                .split('+')
                .map(|part| match part {
                    "none" => Ok(None),
                    _ => part
                        .parse::<i32>()
                        .map(Some)
                        .map_err(|_| format!("weapon slot {part:?} is not a row or none")),
                })
                .collect::<Result<_, _>>()?;
            values
                .try_into()
                .map_err(|_| "a hand states exactly four slots, separated by +".to_owned())
        };
        let active = |key| {
            g.required(key)
                .parse::<usize>()
                .map_err(|_| format!("{key} is not a slot index"))
        };
        let style = g
            .required("arm-style")
            .parse::<u32>()
            .ok()
            .and_then(ArmStyle::from_word)
            .ok_or("arm-style is not a stored style word 0..3")?;
        let player = PlayerState {
            weapons: EquippedWeapons {
                right: slots(g.required("right"))?,
                left: slots(g.required("left"))?,
            },
            active_right: active("active-right")?,
            active_left: active("active-left")?,
            arm_style: style,
        };
        Ok(Self::new(g.required("selector"), player))
    }
}

/// The program states all request and hold choices; the pass follows the stored selector and weapon records.
pub const SELECT_FROMSOFT_ATTACK: PassSpec = PassSpec {
    name: "select-fromsoft-attack",
    options: &[
        OptionSpec::required("selector", "AttackBothLight1_CMSG", "the requested selector name"),
        OptionSpec::required(
            "right",
            "9000000+none+none+none",
            "four right-hand weapon rows or none, in slot order",
        ),
        OptionSpec::required(
            "left",
            "41000000+none+none+none",
            "four left-hand weapon rows or none, in slot order",
        ),
        OptionSpec::required("active-right", "0", "the active right-hand slot"),
        OptionSpec::required("active-left", "0", "the active left-hand slot"),
        OptionSpec::required("arm-style", "3", "the stored arm-style word (0..3)"),
    ],
    make: |g| Ok(Box::new(SelectFromSoftAttack::from_given(g)?)),
};
impl Pass for SelectFromSoftAttack {
    fn name(&self) -> &'static str {
        "select-fromsoft-attack"
    }
    fn options(&self) -> Options {
        let slots = |s: &[Option<i32>; 4]| {
            s.iter()
                .map(|v| v.map_or_else(|| "none".to_owned(), |v| v.to_string()))
                .collect::<Vec<_>>()
                .join("+")
        };
        Options::from([
            ("selector".into(), self.selector.clone()),
            ("right".into(), slots(&self.player.weapons.right)),
            ("left".into(), slots(&self.player.weapons.left)),
            ("active-right".into(), self.player.active_right.to_string()),
            ("active-left".into(), self.player.active_left.to_string()),
            ("arm-style".into(), self.player.arm_style.word().to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AttackInputs::NAME, ClipKey::NAME, Name::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[AttackSelection::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[AttackSelection::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let inputs = world
            .resource::<AttackInputs>()
            .ok_or("no attack tables in the world")?;
        let selector = Selector::named(inputs.selectors.clone(), &self.selector)?;
        check_request(&self.selector, self.player.arm_style)?;
        let plans: Vec<_> = world
            .iter::<ClipKey>()
            .map(|(e, _)| (e, world.get::<Name>(e).map(|n| n.0.clone())))
            .collect();
        let names = plans.iter().filter_map(|(_, name)| name.clone()).collect();
        let choice = choose(&selector, &self.player, &inputs.weapons, &names)?;
        let matches: Vec<_> = plans
            .iter()
            .filter(|(_, name)| name.as_deref() == Some(&choice.animation))
            .map(|(e, _)| *e)
            .collect();
        let selected = match matches.as_slice() {
            [selected] => *selected,
            [] => return Err(format!("the character has no clip plan for {}", choice.animation)),
            _ => return Err(format!("ambiguous clip plan for {}", choice.animation)),
        };
        log.count(
            &format!("attack selector {} -> {}", self.selector, choice.animation),
            1,
        );
        log.count("attack clip plans selected", 1);
        log.count("attack clip plans excluded", (plans.len() - 1) as u64);
        for (entity, _) in plans {
            if entity != selected {
                primase_core::ledger::despawn(world, entity, log, "not the selected attack clip");
            }
        }
        world.insert_resource(AttackSelection { selector, choice });
        Ok(())
    }
}

/// The animation name of an id, in the game's stored naming convention.
fn animation_name(id: i64) -> String {
    format!("a{:03}_{:06}", id / 1_000_000, id % 1_000_000)
}
/// The child `selector` plays for `player` (see the module documentation); `animations` are the names of the character's
/// animations.
fn choose(
    selector: &Selector,
    player: &PlayerState,
    weapons: &[fromsoft_formats::weapon_params::WeaponRow],
    animations: &BTreeSet<String>,
) -> std::result::Result<Choice, String> {
    let hand = match selector.offset_type {
        13 => Hand::Right,
        14 => Hand::Left,
        16 if player.arm_style == ArmStyle::LeftBoth => Hand::Left,
        16 => Hand::Right,
        other => {
            return Err(format!(
                "{}: offset type {other} ({}): its rule reads other state and is not read",
                selector.name,
                selector.offset_name.unwrap_or("unnamed")
            ));
        }
    };
    let mut steps = vec![format!(
        "{} offset type {} ({}), animId {}: the {} hand's weapon",
        selector.name,
        selector.offset_type,
        selector.offset_name.unwrap_or("unnamed"),
        selector.anim_id,
        if hand == Hand::Right { "right" } else { "left" }
    )];
    let row = player
        .active_weapon(hand)
        .ok_or_else(|| format!("{}: the hand's active slot holds no weapon", selector.name))?;
    let mut found = weapons.iter().filter(|weapon| weapon.row == row);
    let weapon = match (found.next(), found.next()) {
        (Some(weapon), None) => weapon,
        (None, _) => return Err(format!("EquipParamWeapon has no row {row}")),
        (Some(_), Some(_)) => return Err(format!("EquipParamWeapon has row {row} twice")),
    };
    let sp = weapon.sp_attack_category.ok_or_else(|| {
        format!(
            "{}: the install's weapon rows have no spAtkcategory to read",
            selector.name
        )
    })?;
    let anim = i64::from(selector.anim_id);
    let category = i64::from(weapon.motion_category);
    steps.push(format!(
        "weapon row {row}: wepmotionCategory {}, spAtkcategory {sp}",
        weapon.motion_category
    ));
    let has = |id: i64| animations.contains(&animation_name(id));
    let mut candidates = Vec::new();
    if sp != 0 {
        candidates.push(i64::from(sp) * 1_000_000 + anim);
    }
    candidates.push(category * 1_000_000 + anim);
    candidates.push(
        if hand == Hand::Right {
            23_000_000
        } else {
            48_000_000
        } + anim,
    );
    let id = match candidates.iter().find(|id| has(**id)) {
        Some(id) => *id,
        None => category * 1_000_000 + anim,
    };
    for c in &candidates {
        steps.push(format!(
            "{}: {}",
            animation_name(*c),
            if has(*c) {
                "the character has it"
            } else {
                "the character has none"
            }
        ));
        if *c == id {
            break;
        }
    }
    let named = animation_name(id);
    let fallback = animation_name(anim);
    let child = match selector
        .children
        .iter()
        .position(|c| c.as_deref() == Some(named.as_str()))
    {
        Some(i) => i,
        None => match selector
            .children
            .iter()
            .position(|c| c.as_deref() == Some(fallback.as_str()))
        {
            Some(i) => {
                steps.push(format!("no child plays {named}: {fallback}"));
                i
            }
            None => {
                steps.push(format!("no child plays {named} or {fallback}: child 0"));
                0
            }
        },
    };
    let animation = selector
        .children
        .get(child)
        .cloned()
        .flatten()
        .ok_or_else(|| format!("{}: child {child} is not a clip generator", selector.name))?;
    steps.push(format!("child {child}: {animation}"));
    Ok(Choice {
        child,
        animation,
        hand,
        row,
        category: weapon.motion_category,
        sp_attack: sp,
        id,
        steps,
    })
}

/// Refuses a selector the arm style contradicts: one under `AttackBoth` (a two-handed attack) with a one-handed style, one
/// under `AttackRight` with a two-handed style. The player's script chooses between them by the style (`GetAttackRequest`:
/// a two-handed request where the style is 2 or more).
fn check_request(selector: &str, style: ArmStyle) -> std::result::Result<(), String> {
    let both = matches!(style, ArmStyle::LeftBoth | ArmStyle::RightBoth);
    if selector.starts_with("AttackBoth") && !both {
        return Err(format!(
            "{selector} is a two-handed attack; the arm style is {style:?}"
        ));
    }
    if selector.starts_with("AttackRight") && both {
        return Err(format!(
            "{selector} is a one-handed attack; the arm style is {style:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::components::attack_clip::weapon_row;
    use fromsoft_formats::Game;
    use fromsoft_formats::param::Row;

    fn weapons(rows: &[(i32, u8, u16)]) -> Vec<fromsoft_formats::weapon_params::WeaponRow> {
        let (wl, _) = fromsoft_formats::weapon_params::layouts(Game::EldenRing);
        let rows: Vec<Row> = rows
            .iter()
            .map(|(id, category, sp)| {
                let mut data = vec![0u8; wl.size];
                data[wl.motion_category] = *category;
                data[238..240].copy_from_slice(&sp.to_le_bytes());
                Row {
                    id: *id,
                    name: None,
                    data,
                }
            })
            .collect();
        rows.into_iter().map(|row| wl.read(&row).unwrap()).collect()
    }

    fn selector(offset_type: i64, anim_id: i32, children: &[&str]) -> Selector {
        Selector {
            name: "AttackBothLight1_CMSG".to_owned(),
            offset_type,
            offset_name: None,
            anim_id,
            children: children.iter().map(|c| Some((*c).to_owned())).collect(),
        }
    }

    /// The Uchigatana (29) in the right hand's first slot, a bow (44) in the left's.
    fn player(style: ArmStyle) -> PlayerState {
        let mut w = EquippedWeapons::default();
        w.right[0] = Some(9_000_000);
        w.left[0] = Some(41_000_000);
        PlayerState {
            weapons: w,
            active_right: 0,
            active_left: 0,
            arm_style: style,
        }
    }

    fn names(n: &[&str]) -> BTreeSet<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_hand_style_type_takes_the_right_weapons_category_unless_the_left_is_held_in_both() {
        let w = weapons(&[(9_000_000, 29, 0), (41_000_000, 44, 0)]);
        let s = selector(16, 32000, &["a000_032000", "a029_032000", "a044_032000"]);
        let anims = names(&["a029_032000", "a044_032000"]);
        let c = choose(&s, &player(ArmStyle::RightBoth), &w, &anims).unwrap();
        assert_eq!(
            (c.hand, c.row, c.category, c.id, c.child, c.animation.as_str()),
            (Hand::Right, 9_000_000, 29, 29_032_000, 1, "a029_032000")
        );
        let c = choose(&s, &player(ArmStyle::LeftBoth), &w, &anims).unwrap();
        assert_eq!((c.hand, c.animation.as_str()), (Hand::Left, "a044_032000"));
    }

    #[test]
    fn a_weapons_own_group_comes_first_then_its_category_then_the_hands_fallback_then_a000() {
        let w = weapons(&[(9_000_000, 29, 112)]);
        let s = selector(
            13,
            30000,
            &["a000_030000", "a029_030000", "a112_030000", "a023_030000"],
        );
        let p = player(ArmStyle::Right);
        let pick = |anims: &[&str]| choose(&s, &p, &w, &names(anims)).unwrap().animation;
        assert_eq!(pick(&["a112_030000", "a029_030000"]), "a112_030000");
        assert_eq!(pick(&["a029_030000"]), "a029_030000");
        assert_eq!(pick(&["a023_030000"]), "a023_030000");
        // no animation for any: the category's id
        assert_eq!(pick(&[]), "a029_030000");
        // which no child plays: the child of animId alone, else child 0
        let s = selector(13, 30000, &["a112_030000", "a000_030000"]);
        assert_eq!(choose(&s, &p, &w, &names(&[])).unwrap().animation, "a000_030000");
        let s = selector(13, 30000, &["a112_030000"]);
        assert_eq!(choose(&s, &p, &w, &names(&[])).unwrap().child, 0);
    }

    #[test]
    fn other_offset_types_and_contradicting_requests_are_refused_by_name() {
        let w = weapons(&[(9_000_000, 29, 0)]);
        for t in [0, 11, 15, 17, 18] {
            let err = choose(
                &selector(t, 30000, &["a000_030000"]),
                &player(ArmStyle::Right),
                &w,
                &names(&[]),
            )
            .unwrap_err();
            assert!(err.contains(&format!("offset type {t}")), "{err}");
        }
        assert!(check_request("AttackBothLight1_CMSG", ArmStyle::RightBoth).is_ok());
        assert!(check_request("AttackBothLight1_CMSG", ArmStyle::Right).is_err());
        assert!(check_request("AttackRightLight1_CMSG", ArmStyle::LeftBoth).is_err());
        assert!(check_request("AttackRightLight1_CMSG", ArmStyle::Left).is_ok());
    }

    #[test]
    fn a_weapon_row_drops_the_upgrade_level() {
        assert_eq!(
            (weapon_row(9_000_025), weapon_row(110_000)),
            (Some(9_000_000), Some(110_000))
        );
    }
}

/// Select the requested attack with the saved hold another registered pass interpreted.
#[derive(Clone, Debug)]
pub struct SelectFromSoftSavedAttack(pub String);
pub const SELECT_FROMSOFT_SAVED_ATTACK: PassSpec = PassSpec {
    name: "select-fromsoft-saved-attack",
    options: &[OptionSpec::required(
        "selector",
        "AttackBothLight1_CMSG",
        "the explicitly requested behavior selector",
    )],
    make: |g| Ok(Box::new(SelectFromSoftSavedAttack(g.required("selector").into()))),
};
impl Pass for SelectFromSoftSavedAttack {
    fn name(&self) -> &'static str {
        "select-fromsoft-saved-attack"
    }
    fn options(&self) -> Options {
        Options::from([("selector".into(), self.0.clone())])
    }
    fn requires(&self) -> &[&'static str] {
        &[
            AttackInputs::NAME,
            ClipKey::NAME,
            Name::NAME,
            crate::sekiro::components::standalone::SelectedSavedHold::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[AttackSelection::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let state = w
            .resource::<crate::sekiro::components::standalone::SelectedSavedHold>()
            .ok_or("no selected saved hold")?
            .state
            .clone();
        SelectFromSoftAttack::new(&self.0, state).run(w, log)
    }
}
