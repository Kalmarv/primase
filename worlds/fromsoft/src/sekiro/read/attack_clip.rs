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
//! `SwordArtsCategory` 18 and any unnamed value) read other state: [`choose`] refuses them by name.
//!
//! Which selector plays is the player's request (an attack button and the arm style, in the player's behaviour script's
//! `GetAttackRequest`, which is not read): it is stated, and [`check_request`] refuses one the arm style contradicts.
//! A one-handed light attack is under `AttackRight`, a two-handed one under `AttackBoth`.
use std::collections::BTreeSet;

use fromsoft_formats::save::{ArmStyle, Slot};
use havok_formats::behavior::{ClipGenerator, CustomManualSelectorGenerator, Document};

use crate::sekiro::effect_data::EquippedWeapons;
use crate::sekiro::passes::weapons::Grip;
use crate::sekiro::read::weapons::WeaponParams;

/// A hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

/// The player's state a save states: the weapon row in each slot of each hand, the active slot of each hand and the arm
/// style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    /// Every hand slot's weapon row, slot order (Elden Ring has three a hand).
    pub weapons: EquippedWeapons,
    pub active_right: usize,
    pub active_left: usize,
    pub arm_style: ArmStyle,
}

/// The `EquipParamWeapon` row of a weapon item id: the id less its upgrade level, its last two digits.
pub fn weapon_row(item: u32) -> Option<i32> {
    i32::try_from(item / 100 * 100).ok()
}

impl PlayerState {
    /// The state a save's character slot holds.
    pub fn from_save(slot: &Slot) -> std::result::Result<Self, String> {
        let mut weapons = EquippedWeapons::default();
        for (to, from) in [
            (&mut weapons.right, &slot.equipped.right_hand),
            (&mut weapons.left, &slot.equipped.left_hand),
        ] {
            for (i, item) in from.iter().enumerate() {
                if let Some(item) = item {
                    to[i] = Some(weapon_row(*item).ok_or_else(|| format!("weapon item {item} is no row"))?);
                }
            }
        }
        let active = |n: u32| usize::try_from(n).map_err(|_| format!("active slot {n}"));
        Ok(Self {
            weapons,
            active_right: active(slot.hold.right_hand)?,
            active_left: active(slot.hold.left_hand)?,
            arm_style: slot.hold.arm_style,
        })
    }

    /// How the weapons are held, for `hold-fromsoft-weapons`: in both hands for the two `Both` styles, else each in its
    /// own hand.
    pub fn grip(&self) -> Grip {
        match self.arm_style {
            ArmStyle::RightBoth => Grip::TwoHandedRight,
            ArmStyle::LeftBoth => Grip::TwoHandedLeft,
            ArmStyle::Left | ArmStyle::Right => Grip::OneHanded,
        }
    }

    /// The weapon row in `hand`'s active slot.
    pub fn active_weapon(&self, hand: Hand) -> Option<i32> {
        let (slots, active) = match hand {
            Hand::Right => (&self.weapons.right, self.active_right),
            Hand::Left => (&self.weapons.left, self.active_left),
        };
        slots.get(active).copied().flatten()
    }

    /// The weapons the character holds in the scene: each hand's active one, in its first slot.
    pub fn held(&self) -> EquippedWeapons {
        let mut held = EquippedWeapons::default();
        held.right[0] = self.active_weapon(Hand::Right);
        held.left[0] = self.active_weapon(Hand::Left);
        held
    }
}

/// A `CustomManualSelectorGenerator` as the file stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub name: String,
    /// `offsetType`, as stored, and its name in the file's table.
    pub offset_type: i64,
    pub offset_name: Option<&'static str>,
    pub anim_id: i32,
    /// Each child's animation name; `None` for a child that is not a clip generator.
    pub children: Vec<Option<String>>,
}

impl Selector {
    /// Every selector of `document`, in object order.
    pub fn all(document: &Document<'_, '_>) -> std::result::Result<Vec<Self>, String> {
        let mut out = Vec::new();
        for object in document.objects().map_err(|e| e.detail)? {
            let Some(s) = object.view::<CustomManualSelectorGenerator>() else {
                continue;
            };
            let offset = s.offset_type().map_err(|e| e.detail)?;
            let mut children = Vec::new();
            for child in s.generators().map_err(|e| e.detail)? {
                children.push(match child.and_then(|c| c.view::<ClipGenerator>()) {
                    Some(clip) => Some(clip.animation_name().map_err(|e| e.detail)?),
                    None => None,
                });
            }
            out.push(Self {
                name: s.name().map_err(|e| e.detail)?,
                offset_type: offset.raw,
                offset_name: offset.name(),
                anim_id: s.anim_id().map_err(|e| e.detail)?,
                children,
            });
        }
        Ok(out)
    }

    /// The selector named `name` of `selectors`; refused where none or several have the name.
    pub fn named(selectors: Vec<Self>, name: &str) -> std::result::Result<Self, String> {
        let mut found: Vec<Self> = selectors.into_iter().filter(|s| s.name == name).collect();
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(format!("the behaviour graph has no selector {name}")),
            n => Err(format!("the behaviour graph has {n} selectors named {name}")),
        }
    }
}

/// The animation name of an id: `a<id / 1000000>_<id % 1000000>`, 3 and 6 digits.
pub fn animation_name(id: i64) -> String {
    format!("a{:03}_{:06}", id / 1_000_000, id % 1_000_000)
}

/// What [`choose`] chose, and from what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The child played and its animation.
    pub child: usize,
    pub animation: String,
    /// The hand whose weapon decided, its row, the row's `wepmotionCategory` and `spAtkcategory`.
    pub hand: Hand,
    pub row: i32,
    pub category: u8,
    pub sp_attack: u16,
    /// The animation id the rule gave.
    pub id: i64,
    /// One line for each step the rule took.
    pub steps: Vec<String>,
}

/// The child `selector` plays for `player` (see the module documentation); `animations` are the names of the character's
/// animations.
pub fn choose(
    selector: &Selector,
    player: &PlayerState,
    weapons: &WeaponParams,
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
    let weapon = weapons.weapon(row).map_err(|e| e.to_string())?;
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
pub fn check_request(selector: &str, style: ArmStyle) -> std::result::Result<(), String> {
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

/// Every selector of character `character`'s behaviour graph (`chr/<character>.behbnd`, its member
/// `Behaviors/<character>.hkx`).
pub fn read_selectors(
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    character: &str,
) -> std::result::Result<Vec<Selector>, String> {
    let path = format!("/chr/{character}.behbnd.dcx");
    let binder =
        fromsoft_formats::Binder::read(&archive.read(&path, decoder).map_err(|e| e.to_string())?, decoder)
            .map_err(|e| e.to_string())?;
    let wanted = format!("{character}.hkx");
    let member = binder
        .files
        .iter()
        .find(|f| {
            f.name.as_deref().is_some_and(|n| {
                let n = n.replace('\\', "/").to_lowercase();
                n.contains("/behaviors/") && n.rsplit('/').next() == Some(wanted.as_str())
            })
        })
        .ok_or_else(|| format!("{path} holds no Behaviors/{wanted}"))?;
    let bytes = decoder.decode_if_dcx(&member.bytes).map_err(|e| e.to_string())?;
    let tag = havok_formats::Tagfile::read(&bytes, None).map_err(|e| e.detail)?;
    let document = Document::read(&tag).map_err(|e| e.detail)?;
    Selector::all(&document)
}

/// The selector `name` of character `character`'s behaviour graph ([`read_selectors`]).
pub fn read_selector(
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    character: &str,
    name: &str,
) -> std::result::Result<Selector, String> {
    Selector::named(read_selectors(archive, decoder, character)?, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::Game;
    use fromsoft_formats::param::{Param, Row};

    fn weapons(rows: &[(i32, u8, u16)]) -> WeaponParams {
        let (wl, hl) = crate::sekiro::read::weapons::layouts(Game::EldenRing);
        let param = |param_type: &str, row_size: usize, rows: Vec<Row>| Param {
            param_type: param_type.to_owned(),
            data_version: 0,
            unk06: 0,
            row_size,
            rows,
        };
        let rows = rows
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
        WeaponParams::of(
            param("EQUIP_PARAM_WEAPON_ST", wl.size, rows),
            param("WEP_ABSORP_POS_PARAM_ST", hl.size, vec![]),
            Game::EldenRing,
        )
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
