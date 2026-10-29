//! Draw groups as an optional presence axis. A map piece, object or asset stores up to 256 draw groups; the community
//! documents that a part is drawn while one of its groups is among the *display groups* that are active, the active
//! ones being those of the collision the player stands on (a collision's own display groups; the other part kinds store
//! display groups that do nothing). With `display_groups: Axes` the read makes one bool axis per draw group any kept
//! placement is under, `fromsoft.display_group.<n>` (class `display_group`, scope global, **no default**: which groups
//! are active is the program's state, not the file's), and gives each placement the condition "one of its draw groups
//! is displayed" (an OR of one-literal clauses, ANDed with the event-flag clause when `states` are read).
//! `resolve-state` then keeps the placements the program's display groups draw: `assign=fromsoft.display_group.3:on;...`
//! or `assign-class=display_group:on` (every group displayed: only the placements with no draw group at all differ from
//! a read without the axes, none).
//!
//! Which program state matches the game's is not settled, so an axis is the *group*, not the collision property that
//! switches it on. A game run (Elden Ring 1.17.2, `m10_00`, loose layout) gave every map piece and asset the draw group 200
//! and every collision the display group 200 and, in another arm, 201: the scenery was gone in both, so the collisions' display
//! groups meeting the parts' alone do not draw them. The collisions' draw groups, the other set that nearly every part's bits
//! fall inside (see [`CollisionGroups::draw`]), are the remaining candidate.
//!
//! A part with no draw group set (a zero array, or no draw block) carries no condition and is present in every state.
//! That is a choice, not a finding: a game run (Elden Ring 1.17.2, `m10_00`, loose layout) showed that setting every
//! map piece's and asset's draw groups to zero hides the scenery that had non-zero groups, but whether the 317,870
//! Elden Ring assets and 654 Sekiro parts that store zero draw groups in the stock files are drawn by another rule was not
//! observed. The counts of the parts the axes cannot decide are in the report.
//!
//! An overlay layout (`m10_00_00_99`) stores no collision of its own: the draw groups of its parts are met by the display
//! groups of the collisions of its base map (1,279 of the 1,298 parts of `m10_00_00_99` with draw groups meet those of
//! `m10_00_00_00`, none meets its own), so reading an overlay adds its base map's collisions to the groups displayed.
//!
//! The groups are numbered by [`fromsoft_formats::msb::group_numbers`] (group `n` is bit `n % 32` of word `n / 32`); the
//! files only compare one group word with another, so the numbering is this crate's naming of the axes, nothing more.
use std::collections::{BTreeMap, BTreeSet};

use fromsoft_formats::Result;
use fromsoft_formats::msb::{GroupCensus, MapLayout, Part, PartDrawInfo, PartKind, group_numbers};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Entity, World};

/// The class of the axes this read makes.
pub const CLASS: &str = "display_group";

/// Whether a read brings in the parts' draw groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayGroups {
    /// None (default): every part present, the groups unread.
    #[default]
    None,
    /// One axis per draw group of a kept placement, and the placements' conditions on them.
    Axes,
}

/// The display groups one collision sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollisionGroups {
    /// The groups of the collision's draw block (the ones that decide what is drawn while the player is on it).
    pub display: Vec<u8>,
    /// The groups of its second block (Elden Ring's collisions and both games' connect collisions store one), as stored:
    /// what they do is not known.
    pub display2: Vec<u8>,
    /// The collision's own draw groups. Nearly every part's draw bits are among those of its map's collisions
    /// (Elden Ring 107,130 of 107,905 parts), more than among the display groups (29,325): which set the game takes from the
    /// collision under the player is not established, so both are listed.
    pub draw: Vec<u8>,
}

/// What the groups gave a read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The axes made: the distinct draw groups of the kept placements.
    pub axes: usize,
    /// Kept placements under at least one draw group: given a condition.
    pub conditioned: usize,
    /// Kept placements under none (a zero array or no draw block): present in every state.
    pub unconditioned: usize,
    /// Kept placements of the map's own layout whose draw groups none of the map's collisions displays: they are drawn
    /// only if another set of display groups is made active.
    pub unmet: usize,
    /// How the whole layout's drawn parts meet its collisions' display groups (all parts, kept or not).
    pub layout: GroupCensus,
    /// The map's collisions that set a display group in either block or a draw group, by name.
    pub collisions: BTreeMap<String, CollisionGroups>,
}

/// The draw groups of the placements of the layouts a read holds, and the axes made from them.
#[derive(Clone, Debug, Default)]
pub struct Groups {
    /// Per map name, the draw group numbers of each part that has any, by part index.
    draws: BTreeMap<String, BTreeMap<usize, Vec<u8>>>,
    /// The groups the map's own collisions display (their draw blocks' groups).
    shown: BTreeSet<u8>,
    /// The name of the map whose layout the read is of.
    own: String,
    axes: BTreeMap<u8, Entity>,
    pub report: Report,
}

impl Groups {
    /// The groups of the `layouts` a read holds (the map's own first, named `own`, then the larger maps' parts it
    /// includes).
    pub fn read<'a>(
        layouts: impl IntoIterator<Item = (&'a str, &'a MapLayout)>,
        own: &str,
    ) -> Result<Groups> {
        let mut groups = Groups {
            own: own.to_owned(),
            ..Groups::default()
        };
        for (name, layout) in layouts {
            let infos = layout.draw_infos()?;
            let mut draws = BTreeMap::new();
            for info in &infos {
                if let Some(draw) = &info.draw {
                    let numbers = draw.draw_group_numbers();
                    if !numbers.is_empty() {
                        draws.insert(info.part, numbers);
                    }
                }
            }
            groups.draws.insert(name.to_owned(), draws);
            if name == own {
                groups.report.layout = layout.group_census()?;
                groups.collect_collisions(layout, &infos, "");
            }
        }
        Ok(groups)
    }

    /// Count the collisions of `layout`, the base map of a `_99` overlay that has none of its own, among those whose display
    /// groups the map shows (their names are listed in the report as `<base map>:<part name>`).
    pub fn add_collisions_of(&mut self, base: &str, layout: &MapLayout) -> Result<()> {
        let infos = layout.draw_infos()?;
        self.collect_collisions(layout, &infos, &format!("{base}:"));
        Ok(())
    }

    /// The display groups of `layout`'s collisions into `shown` and the report.
    fn collect_collisions(&mut self, layout: &MapLayout, infos: &[PartDrawInfo], prefix: &str) {
        let parts: BTreeMap<usize, &Part> = layout.parts.iter().map(|p| (p.index, p)).collect();
        for info in infos {
            let Some(part) = parts.get(&info.part).filter(|p| p.kind == PartKind::Collision) else {
                continue;
            };
            let display = info
                .draw
                .as_ref()
                .map(|d| d.display_group_numbers())
                .unwrap_or_default();
            let display2 = info
                .display2
                .as_ref()
                .map(|d| group_numbers(&d.display_groups))
                .unwrap_or_default();
            let draw = info
                .draw
                .as_ref()
                .map(|d| d.draw_group_numbers())
                .unwrap_or_default();
            self.shown.extend(display.iter().copied());
            if !display.is_empty() || !display2.is_empty() || !draw.is_empty() {
                self.report.collisions.insert(
                    format!("{prefix}{}", part.name),
                    CollisionGroups {
                        display,
                        display2,
                        draw,
                    },
                );
            }
        }
    }

    /// The draw groups of part `part` of the layout of `map`: empty for a part with none.
    pub fn numbers(&self, map: &str, part: usize) -> &[u8] {
        self.draws
            .get(map)
            .and_then(|parts| parts.get(&part))
            .map_or(&[], Vec::as_slice)
    }

    /// Spawn one axis per group number in `numbers`, in number order.
    pub fn spawn_axes(&mut self, world: &mut World, numbers: impl IntoIterator<Item = u8>) {
        let numbers: BTreeSet<u8> = numbers.into_iter().collect();
        for n in numbers {
            let e = world.spawn();
            world.insert(
                e,
                StateAxis {
                    key: axis_key(n),
                    domain: Domain::Bool,
                    default: None,
                    scope: Scope::Global,
                    class: CLASS.into(),
                    source: format!("display group {n}"),
                },
            );
            self.axes.insert(n, e);
        }
        self.report.axes = self.axes.len();
    }

    /// The condition of the placement of part `part` of the layout of `map`: "one of its draw groups is displayed",
    /// ANDed with `event` (the event-flag clause) when there is one. `None` when neither applies. Axes must have been
    /// spawned for the part's groups.
    pub fn condition(&mut self, map: &str, part: usize, event: Option<Clause>) -> Option<Condition> {
        let numbers = self.numbers(map, part).to_vec();
        let mut clauses = Vec::with_capacity(numbers.len());
        for n in &numbers {
            if let Some(&axis) = self.axes.get(n) {
                clauses.push(Clause(vec![Literal { axis, values: 0b10 }]));
            }
        }
        if clauses.is_empty() {
            self.report.unconditioned += 1;
            return event.map(|clause| Condition(vec![clause]));
        }
        self.report.conditioned += 1;
        if map == self.own && !numbers.iter().any(|n| self.shown.contains(n)) {
            self.report.unmet += 1;
        }
        Some(Condition(match event {
            None => clauses,
            Some(event) => clauses.into_iter().map(|group| and(&event, group)).collect(),
        }))
    }
}

/// The key of the axis of display group `n`.
pub fn axis_key(n: u8) -> String {
    format!("fromsoft.display_group.{n}")
}

/// The AND of two clauses: both literal lists, ordered and without repeats.
fn and(a: &Clause, b: Clause) -> Clause {
    let mut literals: Vec<Literal> = a.0.iter().copied().chain(b.0).collect();
    literals.sort_by_key(|l| (l.axis, l.values));
    literals.dedup();
    Clause(literals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(draws: &[(usize, &[u8])], shown: &[u8]) -> Groups {
        Groups {
            draws: BTreeMap::from([(
                "m11_01_00_00".to_owned(),
                draws.iter().map(|&(part, g)| (part, g.to_vec())).collect(),
            )]),
            shown: shown.iter().copied().collect(),
            own: "m11_01_00_00".to_owned(),
            ..Groups::default()
        }
    }

    /// One axis per group, in group order, none with a default; a part under two groups is present while either is
    /// displayed, a part under none is present always.
    #[test]
    fn a_part_is_present_while_one_of_its_groups_is_displayed() {
        let mut g = groups(&[(0, &[3]), (1, &[3, 40]), (2, &[200])], &[3, 40]);
        let mut world = World::default();
        g.spawn_axes(&mut world, [40, 3, 200, 3]);
        assert_eq!(g.report.axes, 3);
        let keys: Vec<String> = world.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
        assert_eq!(
            keys,
            [
                "fromsoft.display_group.3",
                "fromsoft.display_group.40",
                "fromsoft.display_group.200"
            ]
        );
        assert!(
            world
                .iter::<StateAxis>()
                .all(|(_, a)| a.default.is_none() && a.class == CLASS)
        );
        let one = g.condition("m11_01_00_00", 0, None).unwrap();
        assert_eq!(one.0.len(), 1);
        assert_eq!(one.0[0].0.len(), 1);
        assert_eq!(one.0[0].0[0].values, 0b10);
        let two = g.condition("m11_01_00_00", 1, None).unwrap();
        assert_eq!(two.0.len(), 2);
        // no draw group: no condition, counted
        assert!(g.condition("m11_01_00_00", 9, None).is_none());
        // a part of another map's layout is not found
        assert!(g.condition("m11_00_00_00", 0, None).is_none());
        // under a group nothing displays: conditioned, and unmet
        assert!(g.condition("m11_01_00_00", 2, None).is_some());
        assert_eq!(
            (g.report.conditioned, g.report.unconditioned, g.report.unmet),
            (3, 2, 1)
        );
    }

    /// With an event-flag clause every group's clause carries its literals too, in axis order and without repeats; a
    /// part with no groups keeps just the event clause.
    #[test]
    fn a_groups_condition_is_anded_with_the_event_clause() {
        let mut g = groups(&[(0, &[3, 40])], &[3, 40]);
        let mut world = World::default();
        let flag = world.spawn();
        world.insert(
            flag,
            StateAxis {
                key: "fromsoft.flag.8302".into(),
                domain: Domain::Bool,
                default: Some(0),
                scope: Scope::Global,
                class: "event".into(),
                source: "event flag 8302".into(),
            },
        );
        g.spawn_axes(&mut world, [3, 40]);
        let event = Clause(vec![Literal {
            axis: flag,
            values: 0b01,
        }]);
        let condition = g.condition("m11_01_00_00", 0, Some(event.clone())).unwrap();
        assert_eq!(condition.0.len(), 2);
        for clause in &condition.0 {
            assert_eq!(clause.0.len(), 2);
            assert!(clause.0.contains(&Literal {
                axis: flag,
                values: 0b01
            }));
            assert!(
                clause
                    .0
                    .windows(2)
                    .all(|w| (w[0].axis, w[0].values) < (w[1].axis, w[1].values))
            );
        }
        let only_event = g.condition("m11_01_00_00", 5, Some(event.clone())).unwrap();
        assert_eq!(only_event, Condition(vec![event]));
    }

    /// A placement whose groups none of the map's collisions displays is counted unmet; a larger map's parts are not.
    #[test]
    fn unmet_counts_the_own_maps_placements_under_groups_nothing_displays() {
        let mut g = groups(&[(0, &[3]), (1, &[9])], &[3]);
        g.draws
            .insert("m60_44_36_00".to_owned(), BTreeMap::from([(0, vec![9])]));
        let mut world = World::default();
        g.spawn_axes(&mut world, [3, 9]);
        g.condition("m11_01_00_00", 0, None);
        g.condition("m11_01_00_00", 1, None);
        g.condition("m60_44_36_00", 0, None);
        assert_eq!((g.report.conditioned, g.report.unmet), (3, 1));
    }
}
