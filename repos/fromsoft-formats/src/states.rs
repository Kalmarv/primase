//! Part states from a block's event script: which parts the game enables under which event flags.
//!
//! A block's script initialises common functions once per entity or entity group, through `2000:6`
//! (InitializeCommonEvent) with exactly seven i32 arguments: the function, the entity, four flags and the four states
//! packed as bytes. The functions 20004110 (map pieces, `2012:1`), 20004112 (objects, `2005:3`) and 20004114
//! (collisions, `2011:1`) disable the entity, then enable it unless one of the (flag, state) pairs holds (flag 0: an
//! unused pair), and restart when a flag changes; so a part is present while every pair fails. The polarity was checked
//! on Sekiro's castle (m11_01): the walls with the charcoal layer are the group hidden while flag 8302 is off, and a
//! live save with 8301, 8302 and 8306 off shows the intact walls.
//!
//! Calls that cannot be decided
//! statically (another argument count, or arguments filled by parameter substitution), other common functions that
//! touch parts, and state instructions with literal entities in story events are not interpreted: the parts they name
//! keep their default presence and are counted in the report.

use std::collections::{BTreeMap, BTreeSet};

use crate::emevd::Script;
use crate::msb::{Part, PartKind};

/// The common functions that switch a part, by the part kind they address.
const STATE_FUNCTIONS: [(i32, PartKind); 3] = [
    (20004110, PartKind::MapPiece),
    (20004112, PartKind::Object),
    (20004114, PartKind::Collision),
];
/// Instructions that switch a part directly: `2012:1` map piece, `2005:3` object, `2011:1` collision state.
const DIRECT: [(i32, i32); 3] = [(2012, 1), (2005, 3), (2011, 1)];

/// One decided rule: the entity is disabled while any of the pairs holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub event: i64,
    pub function: i32,
    pub entity: i32,
    /// (flag, state): disabled while the flag has this state (true: on).
    pub disabled_when: Vec<(i32, bool)>,
}

/// A script's part-state rules, and what it leaves open.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    rules: BTreeMap<(PartKindKey, i32), Vec<Rule>>,
    /// Every ID an uninterpreted call or instruction names.
    open: BTreeSet<i32>,
    /// Decided rules per function.
    pub decided: BTreeMap<i32, usize>,
    /// Part-state calls not interpreted: wrong argument count or arguments filled by substitution.
    pub undecidable_calls: usize,
    /// Calls of other common functions per function (their arguments may name parts).
    pub other_calls: BTreeMap<i32, usize>,
    /// Direct state instructions (story events); those with a literal entity name it as uninterpreted.
    pub direct_instructions: usize,
}

/// `PartKind` as a map key.
type PartKindKey = u8;
fn key(kind: PartKind) -> PartKindKey {
    match kind {
        PartKind::MapPiece => 0,
        PartKind::Object => 1,
        PartKind::Collision => 2,
        _ => 255,
    }
}

impl Rules {
    /// The rules of one script.
    pub fn read(script: &Script) -> Self {
        let mut s = Rules::default();
        for e in &script.events {
            for (k, ins) in e.instructions.iter().enumerate() {
                let substituted = e.parameters.iter().any(|p| p.instruction == k);
                // the complete little-endian words; a trailing partial word makes a known call undecidable
                let (words, rest) = ins.args.as_chunks::<4>();
                let args: Vec<i32> = words.iter().map(|&w| i32::from_le_bytes(w)).collect();
                let aligned = rest.is_empty();
                if DIRECT.contains(&(ins.bank, ins.id)) {
                    s.direct_instructions += 1;
                    if !substituted && let Some(&id) = args.first() {
                        s.open.insert(id);
                    }
                    continue;
                }
                if (ins.bank, ins.id) != (2000, 6) || args.is_empty() {
                    continue;
                }
                let function = args[0];
                let Some(&(_, kind)) = STATE_FUNCTIONS.iter().find(|(f, _)| *f == function) else {
                    *s.other_calls.entry(function).or_default() += 1;
                    s.open.extend(args.iter().skip(1).copied().filter(|&x| x > 0));
                    continue;
                };
                if !aligned || args.len() != 7 || substituted {
                    s.undecidable_calls += 1;
                    if let Some(&id) = args.get(1) {
                        s.open.insert(id);
                    }
                    continue;
                }
                let states = args[6].to_le_bytes();
                let disabled_when = (0..4)
                    .filter(|&i| args[2 + i] != 0)
                    .map(|i| (args[2 + i], states[i] != 0))
                    .collect();
                *s.decided.entry(function).or_default() += 1;
                s.rules.entry((key(kind), args[1])).or_default().push(Rule {
                    event: e.id,
                    function,
                    entity: args[1],
                    disabled_when,
                });
            }
        }
        s
    }

    /// The rules deciding a part (through its entity ID or any of its groups), in script order per ID.
    pub fn rules_for(&self, part: &Part) -> Vec<&Rule> {
        part.entity_ids()
            .flat_map(|id| self.rules.get(&(key(part.kind), id)).into_iter().flatten())
            .collect()
    }

    /// Whether something uninterpreted names one of the part's IDs, so it keeps its default presence.
    pub fn left_open(&self, part: &Part) -> bool {
        part.entity_ids().any(|id| self.open.contains(&id))
    }

    /// Every flag a rule names, in flag order.
    pub fn flags(&self) -> BTreeSet<i32> {
        self.rules
            .values()
            .flatten()
            .flat_map(|r| r.disabled_when.iter().map(|&(f, _)| f))
            .collect()
    }
}
