//! The weapons a character holds, from the game's params. A weapon the character holds is a row of `EquipParamWeapon`
//! (which one the character holds is the player's state, so it is stated: [`EquippedWeapons`]). The row's `equipModelId`
//! names the weapon's part binder (`parts/wp_a_<id as four digits>`), whose forms are the weapon's models (`wp_a_0300` model
//! 0, `wp_a_0300_1` model 1: Sekiro's katana and its scabbard), and its `absorpParamId` names a row of `WepAbsorpPosParam`:
//! for each model, the character's dummy polygon it stands at in each way of holding it ([`Holds`]).
//!
//! Field layouts and byte decoding are public in `fromsoft_formats::weapon_params`; this adapter keeps only the
//! interpreted world records. `tests/weapon_params.rs` checks
//! them on every stock row (each model id a part binder the archives hold, each `absorpParamId` a row, each dummy polygon
//! one the player's model has).
//!
//! [`EquippedWeapons`]: crate::sekiro::effect_data::EquippedWeapons
use crate::sekiro::effect_data::{Hold, Holds, Trace};
#[cfg(test)]
use fromsoft_formats::Game;
use fromsoft_formats::param::{Param, Row};
use fromsoft_formats::{Archive, Install, Result, dcx::Decoder};

const FORMAT: &str = "FromSoftware weapon params";

pub use fromsoft_formats::weapon_params::{HoldLayout, WeaponLayout, layouts};

/// A weapon row: its part's model id, its motion category, its hold row, its `spAtkcategory` (Elden Ring) and its trace
/// slots that name an effect (a number above 0), with their slot numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponRow {
    pub row: i32,
    pub model: u16,
    pub motion_category: u8,
    pub hold_row: i32,
    /// `spAtkcategory`, where the layout reads it.
    pub sp_attack_category: Option<u16>,
    pub traces: Vec<(usize, Trace)>,
}

impl WeaponRow {
    /// The base name of the weapon's part binder (`wp_a_0300`), before any level suffix.
    pub fn part(&self) -> String {
        format!("wp_a_{:04}", self.model)
    }
}

/// The two params of an install.
#[derive(Debug)]
pub struct WeaponParams {
    weapons: Param,
    holds: Param,
    layouts: (WeaponLayout, HoldLayout),
}

impl WeaponParams {
    /// The install's `EquipParamWeapon` and `WepAbsorpPosParam`, refused where their rows are not the size the layouts are
    /// for.
    pub fn read(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<Self> {
        let read = |leaf: &str| fromsoft_formats::game_params::read_param(install, archive, decoder, leaf);
        let (weapons, holds) = (read("EquipParamWeapon.param")?, read("WepAbsorpPosParam.param")?);
        let layouts = layouts(install.game);
        for (param, size, name) in [
            (&weapons, layouts.0.size, "EquipParamWeapon"),
            (&holds, layouts.1.size, "WepAbsorpPosParam"),
        ] {
            if let Some(row) = param.rows.iter().find(|r| r.data.len() != size) {
                return Err(crate::invalid(
                    FORMAT,
                    0,
                    format!(
                        "{name} row {} is {} bytes; the layout read is for {size}",
                        row.id,
                        row.data.len()
                    ),
                ));
            }
        }
        Ok(WeaponParams {
            weapons,
            holds,
            layouts,
        })
    }

    /// Params made of `weapons` and `holds` in `game`'s layouts, for tests that build their rows.
    #[cfg(test)]
    pub(crate) fn of(weapons: Param, holds: Param, game: Game) -> Self {
        Self {
            weapons,
            holds,
            layouts: layouts(game),
        }
    }

    /// Every supported raw weapon record, retaining inactive trace slots as authored.
    pub fn records(&self) -> Result<Vec<fromsoft_formats::weapon_params::WeaponRow>> {
        self.weapons
            .rows
            .iter()
            .map(|row| self.layouts.0.read(row))
            .collect()
    }

    /// Every row of `EquipParamWeapon`, as read.
    pub fn weapons(&self) -> Result<Vec<WeaponRow>> {
        self.weapons.rows.iter().map(|r| self.weapon_of(r)).collect()
    }

    fn weapon_of(&self, row: &Row) -> Result<WeaponRow> {
        let row = self.layouts.0.read(row)?;
        Ok(WeaponRow {
            row: row.row,
            model: row.model,
            motion_category: row.motion_category,
            hold_row: row.hold_row,
            sp_attack_category: row.sp_attack_category,
            traces: row
                .traces
                .into_iter()
                .enumerate()
                .filter(|(_, trace)| trace.effect > 0)
                .collect(),
        })
    }

    /// The weapon row `row`; refused where the param has none, or has it twice.
    pub fn weapon(&self, row: i32) -> Result<WeaponRow> {
        let mut found = self.weapons.rows.iter().filter(|r| r.id == row);
        match (found.next(), found.next()) {
            (Some(r), None) => self.weapon_of(r),
            (None, _) => Err(crate::invalid(
                FORMAT,
                0,
                format!("EquipParamWeapon has no row {row}"),
            )),
            (Some(_), Some(_)) => Err(crate::invalid(
                FORMAT,
                0,
                format!("EquipParamWeapon has row {row} twice"),
            )),
        }
    }

    /// The ids of the rows of `WepAbsorpPosParam`.
    pub fn hold_rows(&self) -> impl Iterator<Item = i32> + '_ {
        self.holds.rows.iter().map(|r| r.id)
    }

    /// Where each model of hold row `row` stands, model 0 first; refused where the param has no such row.
    pub fn holds(&self, row: i32) -> Result<Vec<Holds>> {
        let r = self
            .holds
            .rows
            .iter()
            .find(|r| r.id == row)
            .ok_or_else(|| crate::invalid(FORMAT, 0, format!("WepAbsorpPosParam has no row {row}")))?;
        let hold = |point: fromsoft_formats::weapon_params::Hold| {
            (point.dummy > 0).then_some(Hold {
                dummy: i32::from(point.dummy),
                shown: point.shown,
            })
        };
        Ok(self
            .layouts
            .1
            .read(r)?
            .into_iter()
            .map(|row| Holds {
                right: hold(row.right),
                left: hold(row.left),
                both: hold(row.both),
                left_hang: hold(row.left_hang),
                right_hang: hold(row.right_hang),
                left_both: hold(row.left_both),
            })
            .collect())
    }
}

/// The weapon's model number of a form of its part (`wp_a_0300_1` is model 1 of `wp_a_0300`, `wp_a_0300` model 0), from the
/// form's name without its part's level suffix; `None` for a form not named so.
pub fn model_of_form(part_base: &str, form_base: &str) -> Option<usize> {
    let rest = form_base.strip_prefix(part_base)?;
    if rest.is_empty() {
        return Some(0);
    }
    rest.strip_prefix('_')?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::effect_data::Shown;

    fn row(id: i32, size: usize, words: &[(usize, &[u8])]) -> Row {
        let mut data = vec![0u8; size];
        for (at, b) in words {
            data[*at..*at + b.len()].copy_from_slice(b);
        }
        Row { id, name: None, data }
    }

    fn param(param_type: &str, row_size: usize, row: Row) -> Param {
        Param {
            param_type: param_type.to_owned(),
            data_version: 0,
            unk06: 0,
            row_size,
            rows: vec![row],
        }
    }

    /// Elden Ring's rows read `spAtkcategory` at 238 beside the motion category at 231 (the Uchigatana's: 29 and 0).
    #[test]
    fn elden_rows_read_their_sp_attack_category() {
        let (wl, hl) = layouts(Game::EldenRing);
        let w = row(
            9000000,
            wl.size,
            &[(wl.motion_category, &[29]), (238, &112u16.to_le_bytes())],
        );
        let params = WeaponParams {
            weapons: param("EQUIP_PARAM_WEAPON_ST", wl.size, w),
            holds: param("WEP_ABSORP_POS_PARAM_ST", hl.size, row(0, hl.size, &[])),
            layouts: (wl, hl),
        };
        let weapon = params.weapon(9000000).unwrap();
        assert_eq!(
            (weapon.motion_category, weapon.sp_attack_category),
            (29, Some(112))
        );
    }

    /// The fields come from their offsets: a weapon row's model id, motion category and hold row, and a hold row's dummy
    /// polygons by model (a number below 1 is none).
    #[test]
    fn the_fields_are_read_at_their_offsets() {
        let (wl, hl) = layouts(Game::Sekiro);
        let w = row(
            8000000,
            wl.size,
            &[
                (wl.model, &300u16.to_le_bytes()),
                (wl.motion_category, &[50]),
                (wl.hold_row, &5000i32.to_le_bytes()),
                (wl.traces, &401000i32.to_le_bytes()),
                (wl.traces + 4, &300i32.to_le_bytes()),
                (wl.traces + 8, &301i32.to_le_bytes()),
                (wl.traces + 12, &(-1i32).to_le_bytes()),
                (wl.traces + 24, &401001i32.to_le_bytes()),
                (wl.traces + 28, &(-1i32).to_le_bytes()),
                (wl.traces + 32, &(-1i32).to_le_bytes()),
            ],
        );
        let h = row(
            5000,
            hl.size,
            &[
                (hl.first, &20i16.to_le_bytes()),
                (hl.first + 2, &21i16.to_le_bytes()),
                (hl.first + 8, &147i16.to_le_bytes()),
                (hl.first + 10, &147i16.to_le_bytes()),
                (hl.first + 12, &(-1i16).to_le_bytes()),
                (hl.left_both + 2, &9i16.to_le_bytes()),
            ],
        );
        let params = WeaponParams {
            weapons: param("EQUIP_PARAM_WEAPON_ST", wl.size, w),
            holds: param("WEP_ABSORP_POS_PARAM_ST", hl.size, h),
            layouts: (wl, hl),
        };
        let weapon = params.weapon(8000000).unwrap();
        let trace = |effect, head, tail| Trace { effect, head, tail };
        assert_eq!(
            weapon,
            WeaponRow {
                row: 8000000,
                model: 300,
                motion_category: 50,
                hold_row: 5000,
                sp_attack_category: None,
                // slot 1 names no effect; slot 2's dummy polygons are none (-1)
                traces: vec![(0, trace(401000, 300, 301)), (2, trace(401001, -1, -1))],
            }
        );
        assert_eq!(weapon.part(), "wp_a_0300");
        let holds = params.holds(5000).unwrap();
        assert_eq!(holds.len(), 6);
        assert_eq!(
            holds[0],
            Holds {
                right: Some(Hold::at(20)),
                left: Some(Hold::at(21)),
                both: None,
                left_hang: None,
                right_hang: Some(Hold::at(147)),
                left_both: None,
            }
        );
        assert_eq!(
            (holds[1].right, holds[1].left, holds[1].left_both),
            (Some(Hold::at(147)), None, Some(Hold::at(9)))
        );
        assert!(params.weapon(1).is_err());
        assert!(params.holds(1).is_err());
    }

    /// Elden Ring's rows say how each slot shows its model (`dispPosType_<way>_<n>`, six per model after the
    /// `leftBoth_<n>`): model 1's both-hands slot at 2117 shown in the left hand, its right-hand slot at 2066 on the body.
    #[test]
    fn elden_rings_slots_say_how_they_show_the_model() {
        let (wl, hl) = layouts(Game::EldenRing);
        let shown = hl.shown.unwrap();
        let h = row(
            42,
            hl.size,
            &[
                (hl.first, &115i16.to_le_bytes()),
                (hl.first + 10, &2066i16.to_le_bytes()),
                (hl.first + 14, &2117i16.to_le_bytes()),
                (hl.left_both + 2, &2117i16.to_le_bytes()),
                (shown, &[0]),
                (shown + 6, &[3, 3, 1, 1, 3, 3]),
            ],
        );
        let params = WeaponParams {
            weapons: param("EQUIP_PARAM_WEAPON_ST", wl.size, row(1, wl.size, &[])),
            holds: param("WEP_ABSORP_POS_PARAM_ST", hl.size, h),
            layouts: (wl, hl),
        };
        let holds = params.holds(42).unwrap();
        assert_eq!(holds.len(), 4);
        let at = |dummy, shown| {
            Some(Hold {
                dummy,
                shown: Some(shown),
            })
        };
        assert_eq!(holds[0].right, at(115, Shown::RightHand));
        assert_eq!(
            (holds[1].right, holds[1].both, holds[1].left_both, holds[1].left),
            (
                at(2066, Shown::Body),
                at(2117, Shown::LeftHand),
                at(2117, Shown::LeftHand),
                None
            )
        );
        assert_eq!(Shown::of(2), Shown::Other(2));
    }

    #[test]
    fn a_forms_model_number_follows_its_name() {
        assert_eq!(model_of_form("wp_a_0300", "wp_a_0300"), Some(0));
        assert_eq!(model_of_form("wp_a_0300", "wp_a_0300_1"), Some(1));
        assert_eq!(model_of_form("wp_a_0300", "wp_a_0300_2"), Some(2));
        assert_eq!(model_of_form("wp_a_0300", "wp_a_0310"), None);
        assert_eq!(model_of_form("wp_a_0300", "wp_a_0300_x"), None);
    }
}
