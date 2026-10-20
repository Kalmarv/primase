//! `ground-reskate-markers{kinds=K+K...,above=M}`: raised markers of the stated kinds that stand at a height
//! (`skate.marker` with a position and a height, raised from a placed `core.marker`) and have no snap of their own are
//! snapped down to the ground below them: the map's ground snap searches down from their height plus `above` metres.
//! `raise-skate` keeps such a marker where it stands, unsnapped (only a ground marker, which has no height, snaps), so
//! a source whose spawn or travel points float above the ground (Sekiro's, GTA's spawns) states this. Both options are
//! the program's; a marker with a snap keeps it. Run after `raise-skate`, before the snap is resolved
//! (`snap-reskate-markers`).

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, World};

use crate::components::{MarkerKind, Other, SkateMarker, Snap, SnapMode};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundReskateMarkers {
    kinds: BTreeSet<MarkerKind>,
    /// Metres, finite and at least 0, kept as their bits so the pass compares exactly.
    above: u64,
}

impl GroundReskateMarkers {
    /// Markers of `kinds` (at least one) snapped down from their height plus `above` metres (finite, at least 0).
    pub fn new(kinds: BTreeSet<MarkerKind>, above: f64) -> Result<GroundReskateMarkers, String> {
        if kinds.is_empty() {
            return Err("ground-reskate-markers needs at least one marker kind".into());
        }
        if !above.is_finite() || above < 0.0 {
            return Err(format!("above={above} is not finite metres at least 0"));
        }
        Ok(GroundReskateMarkers {
            kinds,
            above: above.to_bits(),
        })
    }
    pub fn kinds(&self) -> &BTreeSet<MarkerKind> {
        &self.kinds
    }
    pub fn above(&self) -> f64 {
        f64::from_bits(self.above)
    }
    /// From its options (both required), through [`GroundReskateMarkers::new`].
    pub fn from_options(o: &Options) -> Result<GroundReskateMarkers, String> {
        let (mut kinds, mut above) = (None, None);
        for (k, v) in o {
            match k.as_str() {
                "kinds" => {
                    kinds = Some(
                        v.split('+')
                            .map(|w| {
                                MarkerKind::parse(w).ok_or_else(|| format!("kinds: {w} is not a marker kind"))
                            })
                            .collect::<Result<BTreeSet<_>, _>>()?,
                    )
                }
                "above" => above = Some(v.parse::<f64>().map_err(|_| format!("above={v} is not metres"))?),
                k => return Err(format!("unknown option {k}")),
            }
        }
        GroundReskateMarkers::new(
            kinds.ok_or("ground-reskate-markers needs kinds=K+K...")?,
            above.ok_or("ground-reskate-markers needs above=M")?,
        )
    }
}

impl Pass for GroundReskateMarkers {
    fn name(&self) -> &'static str {
        "ground-reskate-markers"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "kinds".to_owned(),
                self.kinds
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join("+"),
            ),
            ("above".to_owned(), self.above().to_string()),
        ])
    }
    // a world without markers has none to snap
    fn modifies(&self) -> &[&'static str] {
        &[SkateMarker::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let standing: Vec<Entity> = w
            .iter::<SkateMarker>()
            .filter(|(_, m)| self.kinds.contains(&m.kind) && matches!(m.position, Some((_, Some(_)))))
            .map(|(e, _)| e)
            .collect();
        let (mut snapped, mut kept) = (0, 0);
        for e in standing {
            let m = w.get_mut::<SkateMarker>(e).expect("listed");
            if m.snap.is_some() {
                kept += 1;
                continue;
            }
            m.snap = Some(Snap::Object {
                mode: SnapMode::Ground,
                above: Some(self.above()),
                from: None,
                min_up: None,
                rings: None,
                directions: None,
                max_above_ground: None,
                headroom: None,
                other: Other::new(),
            });
            snapped += 1;
        }
        log.count("markers snapped down to the ground below their height", snapped);
        log.count("markers standing at a height that keep their own snap", kept);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};

    fn marker(kind: MarkerKind, height: Option<f32>, snap: Option<Snap>) -> SkateMarker {
        SkateMarker {
            kind,
            position: Some(([1.0, 2.0], height)),
            yaw: None,
            m: None,
            blueprint: None,
            snap,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        }
    }

    #[test]
    fn markers_of_the_kinds_standing_at_a_height_snap_down_from_it() {
        let mut w = World::new();
        let travel = w.spawn();
        w.insert(travel, marker(MarkerKind::Travel, Some(10.0), None));
        let trainer = w.spawn();
        w.insert(trainer, marker(MarkerKind::Trainer, Some(10.0), None));
        let ground = w.spawn();
        w.insert(ground, marker(MarkerKind::Spawn, None, None));
        let own = w.spawn();
        let street = Snap::Mode(SnapMode::Street);
        w.insert(own, marker(MarkerKind::Spawn, Some(5.0), Some(street.clone())));
        let pass =
            GroundReskateMarkers::new(BTreeSet::from([MarkerKind::Spawn, MarkerKind::Travel]), 0.5).unwrap();
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )
            .unwrap();
        let snap = |e| w.get::<SkateMarker>(e).unwrap().snap.clone();
        assert!(matches!(
            snap(travel),
            Some(Snap::Object { mode, above: Some(a), .. }) if mode == SnapMode::Ground && a == 0.5
        ));
        // a kind not stated, a marker with no height (raise-skate snaps it already) and one with its own snap: as they were
        assert_eq!(snap(trainer), None);
        assert_eq!(snap(ground), None);
        assert_eq!(snap(own), Some(street));
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["markers snapped down to the ground below their height"],
            1
        );
        assert_eq!(
            log.counts["markers standing at a height that keep their own snap"],
            1
        );
    }

    #[test]
    fn options_are_required_and_say_every_choice() {
        let pass =
            GroundReskateMarkers::new(BTreeSet::from([MarkerKind::Spawn, MarkerKind::Travel]), 1.0).unwrap();
        assert_eq!(GroundReskateMarkers::from_options(&pass.options()), Ok(pass));
        assert!(GroundReskateMarkers::from_options(&Options::new()).is_err());
        assert!(GroundReskateMarkers::new(BTreeSet::new(), 1.0).is_err());
        assert!(GroundReskateMarkers::new(BTreeSet::from([MarkerKind::Spawn]), -1.0).is_err());
        assert!(
            GroundReskateMarkers::from_options(&Options::from([
                ("kinds".into(), "spawn+boat".into()),
                ("above".into(), "1".into())
            ]))
            .is_err()
        );
    }
}
