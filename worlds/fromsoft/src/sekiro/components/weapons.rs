//! Interpreted authored weapon records shared by readers and passes.
use super::effect::Trace;
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
