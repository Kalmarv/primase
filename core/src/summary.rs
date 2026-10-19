//! Summaries and decisions: what a conversion in parts knows of the whole of what it writes.
//!
//! A decision over the whole of what is written (a map's floor, a light cap per cell that crosses parts, a texture
//! budget) cannot be made from one part. A conversion in parts ([`crate::convert::convert_parts_summarized`]) goes over
//! the parts twice: first each part's summary passes make small typed records of what the part holds (a
//! [`Summary`]: the lowest points of its placed parts, its texture weights), which are merged over every part; the
//! decision passes then make the decisions from the merged summaries, once, as records of their own; and each part,
//! loaded again, gets the summaries and decisions before its passes run, so a pass replays a decision instead of
//! making it from what one part holds.
//!
//! A summary's merge is a property of the data, not of the pass that made it, and it must not depend on the order the
//! parts arrive in or how they are grouped: the merge is commutative and associative, a contract its crate states and
//! tests (shuffled orders, groupings, overlapping inputs). The conversion keeps every part's summaries until it merges
//! them (metadata, each summary stating the bytes it holds, [`Summary::retained_bytes`]; the conversion reports their
//! total, [`crate::convert::ConvertReport::summary_bytes`]), merges them in arrival order and in the
//! reverse order, and refuses a summary whose two merges differ: a forward/reverse consistency check on every
//! conversion, which catches many order dependencies but proves none absent (a merge that keeps only its first two
//! arrivals can agree on A, B, A forward and backward and still differ on A, A, B). The crate that defines a summary
//! or a decision registers it ([`SummaryRegistry`]), as it registers its passes.

use std::collections::BTreeMap;

use crate::World;
use crate::world::Resource;

/// A record a pass makes from one part, merged with the other parts' into the whole's. The merge must be order-free
/// (commutative and associative); equality is what the conversion compares the two merge orders by.
pub trait Summary: Resource + PartialEq {
    /// Fold `other`, another part's, into this one; an error for summaries that cannot be merged (made for other
    /// settings).
    fn merge(&mut self, other: Self) -> Result<(), String>;
    /// The bytes it holds (its heap and inline data, near enough to bound what a conversion keeps of it).
    fn retained_bytes(&self) -> usize;
}

/// What the registry does with one kind of record, by type, through plain functions.
#[derive(Clone, Copy)]
struct Kind {
    /// Merge `from`'s record (if any) into `into`'s, or move it there when `into` has none.
    merge: fn(into: &mut World, from: &World) -> Result<(), String>,
    /// Whether two worlds hold equal records (both none counts as equal).
    equal: fn(a: &World, b: &World) -> bool,
    /// Copy `from`'s record (if any) into `to`.
    copy: fn(from: &World, to: &mut World),
    /// The bytes `w`'s record holds (0 without one; decisions are not counted).
    bytes: fn(w: &World) -> usize,
    /// A summary (merged over parts), or a decision (made once, handed on).
    summary: bool,
}

impl std::fmt::Debug for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.summary { "summary" } else { "decision" })
    }
}

fn merge_typed<S: Summary>(into: &mut World, from: &World) -> Result<(), String> {
    let Some(s) = from.resource::<S>().cloned() else {
        return Ok(());
    };
    match into.resource_mut::<S>() {
        Some(m) => m.merge(s),
        None => {
            into.insert_resource(s);
            Ok(())
        }
    }
}

fn bytes_typed<S: Summary>(w: &World) -> usize {
    w.resource::<S>().map_or(0, Summary::retained_bytes)
}

fn no_bytes(_: &World) -> usize {
    0
}

fn equal_typed<S: Summary>(a: &World, b: &World) -> bool {
    a.resource::<S>() == b.resource::<S>()
}

fn copy_typed<R: Resource>(from: &World, to: &mut World) {
    if let Some(r) = from.resource::<R>() {
        to.insert_resource(r.clone());
    }
}

fn never_merged<R: Resource>(_: &mut World, _: &World) -> Result<(), String> {
    Err(format!("{} is a decision, made once: it is not merged", R::NAME))
}

/// Decisions are not compared: they are made once.
fn never_compared(_: &World, _: &World) -> bool {
    true
}

/// The summaries and decisions a conversion in parts carries, by resource name.
#[derive(Debug, Default)]
pub struct SummaryRegistry {
    kinds: BTreeMap<&'static str, Kind>,
}

impl SummaryRegistry {
    pub fn new() -> Self {
        SummaryRegistry::default()
    }

    /// Register a summary: merged over every part, then handed to every part.
    pub fn summary<S: Summary>(&mut self) {
        self.kinds.insert(
            S::NAME,
            Kind {
                merge: merge_typed::<S>,
                equal: equal_typed::<S>,
                copy: copy_typed::<S>,
                bytes: bytes_typed::<S>,
                summary: true,
            },
        );
    }

    /// Register a decision: made once from the merged summaries, then handed to every part.
    pub fn decision<D: Resource>(&mut self) {
        self.kinds.insert(
            D::NAME,
            Kind {
                merge: never_merged::<D>,
                equal: never_compared,
                copy: copy_typed::<D>,
                bytes: no_bytes,
                summary: false,
            },
        );
    }

    /// The summaries' names.
    pub fn summaries(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.iter().filter(|(_, k)| k.summary).map(|(n, _)| *n)
    }

    /// The decisions' names.
    pub fn decisions(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.iter().filter(|(_, k)| !k.summary).map(|(n, _)| *n)
    }

    /// The bytes `w`'s summaries hold, as they state them.
    pub fn retained(&self, w: &World) -> usize {
        self.kinds.values().map(|k| (k.bytes)(w)).sum()
    }

    /// A world holding only `part`'s summaries (a copy of each), to keep and merge.
    pub fn take_summaries(&self, part: &World) -> World {
        let mut out = World::new();
        for k in self.kinds.values().filter(|k| k.summary) {
            (k.copy)(part, &mut out);
        }
        out
    }

    /// The parts' summaries (each world as [`SummaryRegistry::take_summaries`] made it) merged in the order given, and
    /// in the reverse order: an error naming a summary whose two merges differ (its merge depends on the order the
    /// parts arrive in) or that cannot be merged. A consistency check, not a proof of order-freedom (see the module).
    pub fn merge_order_free(&self, parts: &[World]) -> Result<World, String> {
        let merged = |order: &mut dyn Iterator<Item = &World>| -> Result<World, String> {
            let mut out = World::new();
            for p in order {
                for (name, k) in self.kinds.iter().filter(|(_, k)| k.summary) {
                    (k.merge)(&mut out, p).map_err(|e| format!("{name}: {e}"))?;
                }
            }
            Ok(out)
        };
        let forward = merged(&mut parts.iter())?;
        let backward = merged(&mut parts.iter().rev())?;
        for (name, k) in self.kinds.iter().filter(|(_, k)| k.summary) {
            if !(k.equal)(&forward, &backward) {
                return Err(format!(
                    "{name}: its merge depends on the order the parts arrive in (the merge of {} parts differs \
                     forward and backward)",
                    parts.len()
                ));
            }
        }
        Ok(forward)
    }

    /// Copy `from`'s summaries and decisions into `to` (a part about to run its passes). They stay in the part: the
    /// writer's target judges them as any other resource.
    pub fn hand_on(&self, from: &World, to: &mut World) {
        for k in self.kinds.values() {
            (k.copy)(from, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lowest of the values seen: order-free.
    #[derive(Clone, Debug, PartialEq)]
    struct Lowest(f64);
    impl Resource for Lowest {
        const NAME: &'static str = "toy.lowest";
    }
    impl Summary for Lowest {
        fn merge(&mut self, other: Self) -> Result<(), String> {
            self.0 = self.0.min(other.0);
            Ok(())
        }
        fn retained_bytes(&self) -> usize {
            8
        }
    }

    /// The values in the order they were merged: not order-free.
    #[derive(Clone, Debug, PartialEq)]
    struct Arrivals(Vec<u32>);
    impl Resource for Arrivals {
        const NAME: &'static str = "toy.arrivals";
    }
    impl Summary for Arrivals {
        fn merge(&mut self, other: Self) -> Result<(), String> {
            self.0.extend(other.0);
            Ok(())
        }
        fn retained_bytes(&self) -> usize {
            4 * self.0.len()
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Floor(f64);
    impl Resource for Floor {
        const NAME: &'static str = "toy.floor";
    }

    fn part(lowest: f64, arrival: Option<u32>) -> World {
        let mut w = World::new();
        w.insert_resource(Lowest(lowest));
        if let Some(a) = arrival {
            w.insert_resource(Arrivals(vec![a]));
        }
        w
    }

    #[test]
    fn an_order_free_summary_merges_and_an_order_bound_one_is_refused() {
        let mut r = SummaryRegistry::new();
        r.summary::<Lowest>();
        r.decision::<Floor>();
        let parts: Vec<World> = [3.0, -2.0, 7.0]
            .into_iter()
            .map(|l| r.take_summaries(&part(l, Some(1))))
            .collect();
        // a summary not registered is not taken
        assert!(parts[0].resource::<Arrivals>().is_none());
        let merged = r.merge_order_free(&parts).unwrap();
        assert_eq!(merged.resource::<Lowest>(), Some(&Lowest(-2.0)));
        // the decisions and summaries are handed on
        let mut decided = merged;
        decided.insert_resource(Floor(-2.0));
        let mut p = World::new();
        r.hand_on(&decided, &mut p);
        assert_eq!(
            (p.resource::<Lowest>(), p.resource::<Floor>()),
            (Some(&Lowest(-2.0)), Some(&Floor(-2.0)))
        );

        r.summary::<Arrivals>();
        let parts: Vec<World> = [1, 2, 3]
            .into_iter()
            .map(|a| r.take_summaries(&part(0.0, Some(a))))
            .collect();
        let e = r.merge_order_free(&parts).unwrap_err();
        assert!(
            e.starts_with("toy.arrivals: its merge depends on the order"),
            "{e}"
        );
        // one part, or parts without it: nothing to differ
        assert!(r.merge_order_free(&parts[..1]).is_ok());
        let without: Vec<World> = (0..2).map(|_| r.take_summaries(&part(1.0, None))).collect();
        assert!(r.merge_order_free(&without).is_ok());
    }

    #[test]
    fn a_decision_is_not_merged() {
        let mut r = SummaryRegistry::new();
        r.decision::<Floor>();
        let mut into = World::new();
        let mut from = World::new();
        from.insert_resource(Floor(1.0));
        let k = r.kinds[Floor::NAME];
        assert!((k.merge)(&mut into, &from).unwrap_err().contains("a decision"));
        assert_eq!(r.decisions().collect::<Vec<_>>(), [Floor::NAME]);
        assert_eq!(r.summaries().count(), 0);
    }
}
