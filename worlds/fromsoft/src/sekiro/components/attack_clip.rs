//! Authored selector and weapon records, caller-stated player holds, and a pass's traced attack selection.
use crate::sekiro::effect_data::EquippedWeapons;
use crate::sekiro::passes::weapons::Grip;
use fromsoft_formats::save::{ArmStyle, Slot};
use primase_core::{
    Resource,
    json::{JsonError, to_value},
};
use serde::Serialize;
use serde_json::{Value, json};
/// A hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

/// What the registered attack-selection pass chose, and from what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

/// Every supported selector and weapon row, read before any attack is chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AttackInputs {
    pub character: String,
    pub selectors: Vec<Selector>,
    pub weapons: Vec<fromsoft_formats::weapon_params::WeaponRow>,
}
impl Resource for AttackInputs {
    const NAME: &'static str = "fromsoft.attack_inputs";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// Display provenance of a selected attack. Consumers load the retained core clip plan, not fields of this report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackSelection {
    pub selector: Selector,
    pub choice: Choice,
}
impl Resource for AttackSelection {
    const NAME: &'static str = "fromsoft.attack_selection";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "selector": self.selector.name, "offset_type": self.selector.offset_type,
            "offset_name": self.selector.offset_name, "anim_id": self.selector.anim_id,
            "hand": format!("{:?}", self.choice.hand), "weapon_row": self.choice.row,
            "wepmotionCategory": self.choice.category, "spAtkcategory": self.choice.sp_attack,
            "id": self.choice.id, "child": self.choice.child, "clip": self.choice.animation,
            "steps": self.choice.steps,
        })))
    }
}
