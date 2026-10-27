//! World states from a block's event script ([`fromsoft_formats::states`]): each flag a rule names becomes a bool
//! axis (`fromsoft.flag.<id>`, class `event`, default off: the game's start), and a part's condition is one clause,
//! the AND over its rules of "flag is not the state". Parts something uninterpreted names keep their default presence
//! and are counted in the report.

use std::collections::BTreeMap;

use primase_core::World;
use primase_core::state::{Clause, Domain, Literal, Scope, StateAxis};

use fromsoft_formats::emevd::Script;
use fromsoft_formats::msb::Part;
use fromsoft_formats::states::Rules;

/// What the script decides and what it leaves open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Decided rules per function.
    pub rules: BTreeMap<i32, usize>,
    /// Part-state calls not interpreted: wrong argument count or arguments filled by substitution.
    pub undecidable_calls: usize,
    /// Calls of other common functions per function (their arguments may name parts).
    pub other_calls: BTreeMap<i32, usize>,
    /// Direct state instructions (story events); those with a literal entity name it as uninterpreted.
    pub direct_instructions: usize,
    /// Parts with a condition, per kind.
    pub decided_parts: BTreeMap<String, usize>,
    /// Parts some uninterpreted instruction or call names, per kind, decided rules or not: kept at their default
    /// presence.
    pub undecided_parts: BTreeMap<String, usize>,
    /// Axes made.
    pub axes: usize,
}

/// A block's state rules, ready to give parts their conditions.
#[derive(Clone, Debug, Default)]
pub struct States {
    rules: Rules,
    axes: BTreeMap<i32, primase_core::Entity>,
    pub report: Report,
}

impl States {
    /// The rules of one script.
    pub fn read(script: &Script) -> Self {
        let rules = Rules::read(script);
        let report = Report {
            rules: rules.decided.clone(),
            undecidable_calls: rules.undecidable_calls,
            other_calls: rules.other_calls.clone(),
            direct_instructions: rules.direct_instructions,
            ..Report::default()
        };
        States {
            rules,
            axes: BTreeMap::new(),
            report,
        }
    }

    /// Spawn one bool axis per flag any rule names, in flag order (so entity ids do not depend on part order).
    pub fn spawn_axes(&mut self, world: &mut World) {
        for f in self.rules.flags() {
            let e = world.spawn();
            world.insert(
                e,
                StateAxis {
                    key: format!("fromsoft.flag.{f}"),
                    domain: Domain::Bool,
                    default: Some(0),
                    scope: Scope::Global,
                    class: "event".into(),
                    source: format!("event flag {f}"),
                },
            );
            self.axes.insert(f, e);
        }
        self.report.axes = self.axes.len();
    }

    /// The part's clause (the AND over its rules of "flag is not the state"), None when no rule decides it; and the
    /// report's counts. Axes must have been spawned.
    pub fn clause(&mut self, part: &Part) -> Option<Clause> {
        let kind = format!("{:?}", part.kind);
        // an ID something uninterpreted also names wins over the decided rules: the part keeps its default presence
        if self.rules.left_open(part) {
            *self.report.undecided_parts.entry(kind).or_default() += 1;
            return None;
        }
        let rules = self.rules.rules_for(part);
        if rules.is_empty() {
            return None;
        }
        let mut literals: Vec<Literal> = rules
            .iter()
            .flat_map(|r| r.disabled_when.iter())
            .map(|&(f, on)| Literal {
                axis: self.axes[&f],
                // present while the flag has the other value: off -> mask 0b01, on -> 0b10
                values: if on { 0b01 } else { 0b10 },
            })
            .collect();
        literals.sort_by_key(|l| (l.axis, l.values));
        literals.dedup();
        *self.report.decided_parts.entry(kind).or_default() += 1;
        Some(Clause(literals))
    }
}
