//! The weapons a character holds, from the game's params. A weapon the character holds is a row of `EquipParamWeapon`
//! (which one the character holds is the player's state, so it is stated: [`EquippedWeapons`]). The row's `equipModelId`
//! names the weapon's part binder (`parts/wp_a_<id as four digits>`), whose forms are the weapon's models (`wp_a_0300` model
//! 0, `wp_a_0300_1` model 1: Sekiro's katana and its scabbard), and its `absorpParamId` names a row of `WepAbsorpPosParam`:
//! for each model, the character's dummy polygon it stands at in each way of holding it ([`Holds`]).
//!
//! The field offsets are the Paramdex definitions' (`SDT` and `ER` `EquipParamWeapon.xml`, `WepAbsorpPosParam.xml`) laid
//! out by the definition rule (`fromsoft_formats::paramdef`) at the installs' row sizes; `tests/weapon_params.rs` checks
//! them on every stock row (each model id a part binder the archives hold, each `absorpParamId` a row, each dummy polygon
//! one the player's model has).
//!
//! [`EquippedWeapons`]: crate::sekiro::effect_data::EquippedWeapons
use crate::sekiro::effect_data::{Holds, Trace};
use fromsoft_formats::param::{Param, Row};
use fromsoft_formats::{Archive, Game, Install, Result, dcx::Decoder};

const FORMAT: &str = "FromSoftware weapon params";

/// Where an `EQUIP_PARAM_WEAPON_ST` row keeps the fields the read uses, for one game: the row size the offsets are for, and
/// the byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponLayout {
    pub size: usize,
    /// `equipModelId` (u16).
    pub model: usize,
    /// `wepmotionCategory` (u8): the animation group of the weapon's moves (Sekiro's katana 50: `a050_300000`).
    pub motion_category: usize,
    /// `absorpParamId` (s32): the `WepAbsorpPosParam` row.
    pub hold_row: usize,
    /// `traceSfxId0` (s32), the first of the eight trace slots: `traceSfxId<n>`, `traceDmyIdHead<n>`, `traceDmyIdTail<n>`
    /// (three s32 each, one slot after another).
    pub traces: usize,
}

/// The trace slots a weapon row has.
pub const TRACE_SLOTS: usize = 8;

/// Where a `WEP_ABSORP_POS_PARAM_ST` row keeps its dummy polygons: the row size, the first model's block (five s16: held
/// in the right hand, in the left, with both hands, hung on the left, hung on the right; one block per model, `models` of
/// them) and the first of the `leftBoth_<n>` s16s (held with both hands in the left, one per model).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldLayout {
    pub size: usize,
    pub first: usize,
    pub models: usize,
    pub left_both: usize,
}

/// The layouts of `game`'s two params.
pub fn layouts(game: Game) -> (WeaponLayout, HoldLayout) {
    match game {
        Game::EldenRing => (
            WeaponLayout {
                size: 664,
                model: 188,
                motion_category: 231,
                hold_row: 372,
                traces: 272,
            },
            HoldLayout {
                size: 96,
                first: 8,
                models: 4,
                left_both: 52,
            },
        ),
        _ => (
            WeaponLayout {
                size: 632,
                model: 184,
                motion_category: 227,
                hold_row: 368,
                traces: 268,
            },
            HoldLayout {
                size: 128,
                first: 4,
                models: 6,
                left_both: 70,
            },
        ),
    }
}

/// A weapon row: its part's model id, its motion category, its hold row and its trace slots that name an effect (a number
/// above 0), with their slot numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponRow {
    pub row: i32,
    pub model: u16,
    pub motion_category: u8,
    pub hold_row: i32,
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

fn bytes<const N: usize>(row: &Row, at: usize, what: &str) -> Result<[u8; N]> {
    at.checked_add(N)
        .and_then(|end| row.data.get(at..end))
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| crate::invalid(FORMAT, at, format!("row {}: no {what}", row.id)))
}

fn s16(row: &Row, at: usize, what: &str) -> Result<i16> {
    bytes::<2>(row, at, what).map(i16::from_le_bytes)
}

/// A dummy polygon a hold field names: none for a number below 1 (the stock rows hold -1 and 0 where a model has no place).
fn dummy(n: i16) -> Option<i32> {
    (n > 0).then_some(i32::from(n))
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

    /// Every row of `EquipParamWeapon`, as read.
    pub fn weapons(&self) -> Result<Vec<WeaponRow>> {
        self.weapons.rows.iter().map(|r| self.weapon_of(r)).collect()
    }

    fn weapon_of(&self, row: &Row) -> Result<WeaponRow> {
        let l = self.layouts.0;
        let word = |at: usize, what: &str| bytes::<4>(row, at, what).map(i32::from_le_bytes);
        let mut traces = Vec::new();
        for n in 0..TRACE_SLOTS {
            let at = l.traces + 12 * n;
            let effect = word(at, "traceSfxId")?;
            if effect > 0 {
                let dummy = |id: i32| (id > 0).then_some(id);
                traces.push((
                    n,
                    Trace {
                        effect,
                        head: dummy(word(at + 4, "traceDmyIdHead")?),
                        tail: dummy(word(at + 8, "traceDmyIdTail")?),
                    },
                ));
            }
        }
        Ok(WeaponRow {
            row: row.id,
            model: u16::from_le_bytes(bytes::<2>(row, l.model, "equipModelId")?),
            motion_category: bytes::<1>(row, l.motion_category, "wepmotionCategory")?[0],
            hold_row: word(l.hold_row, "absorpParamId")?,
            traces,
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
        let l = self.layouts.1;
        (0..l.models)
            .map(|m| {
                let at = l.first + 10 * m;
                Ok(Holds {
                    right: dummy(s16(r, at, "right_n")?),
                    left: dummy(s16(r, at + 2, "left_n")?),
                    both: dummy(s16(r, at + 4, "both_n")?),
                    left_hang: dummy(s16(r, at + 6, "leftHang_n")?),
                    right_hang: dummy(s16(r, at + 8, "rightHang_n")?),
                    left_both: dummy(s16(r, l.left_both + 2 * m, "leftBoth_n")?),
                })
            })
            .collect()
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
                // slot 1 names no effect; slot 2's dummy polygons are none
                traces: vec![
                    (0, trace(401000, Some(300), Some(301))),
                    (2, trace(401001, None, None))
                ],
            }
        );
        assert_eq!(weapon.part(), "wp_a_0300");
        let holds = params.holds(5000).unwrap();
        assert_eq!(holds.len(), 6);
        assert_eq!(
            holds[0],
            Holds {
                right: Some(20),
                left: Some(21),
                both: None,
                left_hang: None,
                right_hang: Some(147),
                left_both: None,
            }
        );
        assert_eq!(
            (holds[1].right, holds[1].left, holds[1].left_both),
            (Some(147), None, Some(9))
        );
        assert!(params.weapon(1).is_err());
        assert!(params.holds(1).is_err());
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
