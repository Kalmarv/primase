//! `select-forzatech-drawn`: the placements the game draws at a camera distance the program states, by the game's own
//! distances; the others dropped by name, and the prototypes no placement uses any more with them.
//!
//! A rule-placed placement is drawn at distance `d` when a range of its template's draw behaviour in the main view's
//! slot ([`MAIN_VIEW_SLOT`]) holds `d` (`start <= d < end`). That slot 13 is the main colour view is inferred: it is the
//! only slot whose LOD set holds the finest LOD of every template and the whole LOD chain (a tree's 0-25, 25-80,
//! 80-225 m), and an impostor's range in it starts where its full form's chain ends; the other slots draw one coarser
//! set (impostor cards from 0 m, flattenable-bush proxies), so counting them kept, at 5 m, impostors and proxies
//! whose full forms stand at the same positions. A hand-placed one is drawn when the band of its LOD profile group's main-view record (pass 2)
//! for the stated quality tier that holds `d` has a LOD row (band `k` holds `[max(distances 0..k), distance k)`); a
//! placement whose group has no such record is not drawn in the main view, and one with no group is drawn when its
//! nearest band has a row. So at a short distance the impostors, far billboards and HLOD proxies, drawn only further
//! out, are dropped, without reading their names.
use crate::components::{ForzaLevels, LodProfiles, Placement, PlacementOrigin, Template};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, World};
use std::collections::BTreeSet;

/// The `.pvs` pass number of the main view's LOD profile records.
pub const MAIN_VIEW: u16 = 2;

/// The draw behaviour slot of the main colour view (inferred: see the module documentation).
pub const MAIN_VIEW_SLOT: u8 = 13;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectForzaDrawn {
    /// The camera distance, metres.
    pub distance: f32,
    /// The quality tier of the hand-placed LOD profiles.
    pub tier: u16,
}

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::required(
        "distance",
        "5",
        "camera distance in metres the placements kept are drawn at",
    ),
    OptionSpec::required(
        "tier",
        "0",
        "the quality tier (0 to 4) of the hand-placed LOD profiles",
    ),
];

pub const SELECT_FORZATECH_DRAWN: PassSpec = PassSpec {
    name: "select-forzatech-drawn",
    options: OPTIONS,
    make: |g| Ok(Box::new(SelectForzaDrawn::from_given(g)?)),
};

impl SelectForzaDrawn {
    fn from_given(g: &Given<'_>) -> Result<Self, String> {
        let distance: f32 = g
            .required("distance")
            .parse()
            .map_err(|_| "distance needs a number of metres")?;
        let tier: u16 = g
            .required("tier")
            .parse()
            .map_err(|_| "tier needs a whole number")?;
        SelectForzaDrawn::new(distance, tier)
    }

    pub fn new(distance: f32, tier: u16) -> Result<Self, String> {
        if !(distance.is_finite() && distance >= 0.0) {
            return Err(format!(
                "distance {distance}: needs a finite number of metres, 0 or more"
            ));
        }
        Ok(SelectForzaDrawn { distance, tier })
    }

    /// `Ok` when a hand-placed placement is drawn at the distance, else why not.
    fn hand_drawn(
        &self,
        profiles: Option<&LodProfiles>,
        group: Option<u8>,
        band_rows: &[Option<u8>; 6],
    ) -> Result<(), &'static str> {
        let no_row = "hand-placed placements whose band at the distance has no LOD row";
        let Some(group) = group else {
            return band_rows[0].map(|_| ()).ok_or(no_row);
        };
        let Some(record) = profiles
            .and_then(|p| p.0.get(usize::from(group)))
            .and_then(|g| g.iter().find(|r| r.pass == MAIN_VIEW && r.tier == self.tier))
        else {
            return Err("hand-placed placements whose LOD profile group has no main-view record of the tier");
        };
        let mut from = 0f32;
        for (k, &to) in record.distances.iter().enumerate() {
            if from <= self.distance && self.distance < to {
                return band_rows[k].map(|_| ()).ok_or(no_row);
            }
            from = from.max(to);
        }
        Err("hand-placed placements beyond their profile's last band")
    }
}

impl Pass for SelectForzaDrawn {
    fn name(&self) -> &'static str {
        "select-forzatech-drawn"
    }
    fn options(&self) -> Options {
        Options::from([
            ("distance".into(), self.distance.to_string()),
            ("tier".into(), self.tier.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Placement::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let profiles = world.resource::<LodProfiles>().cloned();
        let profiles = profiles.as_ref();
        let mut dropped = Vec::new();
        for (e, p) in world.iter::<Placement>() {
            let drawn = match &p.origin {
                PlacementOrigin::Rule { .. } => world
                    .get::<Instance>(e)
                    .and_then(|i| world.get::<Template>(i.prototype))
                    .is_some_and(|t| {
                        t.ranges.iter().any(|r| {
                            r.slot == MAIN_VIEW_SLOT && r.start <= self.distance && self.distance < r.end
                        })
                    })
                    .then_some(())
                    .ok_or("rule-placed placements not drawn at the distance in the main view's slot"),
                PlacementOrigin::Hand {
                    band_rows, lod_group, ..
                } => self.hand_drawn(profiles, *lod_group, band_rows),
            };
            if let Err(why) = drawn {
                dropped.push((e, why));
            }
        }
        let before: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        for (e, why) in dropped {
            primase_core::ledger::despawn(world, e, log, why);
        }
        let after: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        for p in before.difference(&after) {
            let forms: Vec<Entity> = world
                .get::<ForzaLevels>(*p)
                .map(|l| l.forms.iter().map(|f| f.form).collect())
                .unwrap_or_default();
            primase_core::ledger::despawn(world, *p, log, "prototypes of no placement left");
            for f in forms {
                primase_core::ledger::despawn(world, f, log, "forms of prototypes of no placement left");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::DrawRange;

    /// A rule placement of a prototype whose template is drawn in `ranges` (slot, start, end).
    fn placed(w: &mut World, ranges: &[(u8, f32, f32)]) -> Entity {
        let p = w.spawn();
        w.insert(
            p,
            Template {
                name: "t".into(),
                category: "TREES".into(),
                draw_behaviour: "b".into(),
                models: Vec::new(),
                ranges: ranges
                    .iter()
                    .map(|&(slot, start, end)| DrawRange { slot, start, end })
                    .collect(),
            },
        );
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        w.insert(
            e,
            Placement {
                origin: PlacementOrigin::Rule {
                    file: "f.pgeo".into(),
                    category: "TREES".into(),
                    block: "t".into(),
                    record: 0,
                    route: String::new(),
                    gameplay_tag: String::new(),
                    variation: [255; 4],
                    frame: 0,
                },
                season_exclusion: 0,
                material_variant: 0,
            },
        );
        e
    }

    #[test]
    fn a_rule_placement_is_drawn_by_the_main_views_slot_alone() {
        let mut w = World::new();
        // a tree's full form: the LOD chain in slot 13, a coarse set in slot 11
        let full = placed(&mut w, &[(11, 0.0, 135.0), (13, 0.0, 25.0), (13, 25.0, 80.0)]);
        // its impostor: cards from 0 m in slots 1 and 6, in slot 13 only from where the chain ends
        let impostor = placed(&mut w, &[(1, 0.0, 400.0), (6, 0.0, 200.0), (13, 225.0, 1000.0)]);
        // a flattenable-bush proxy: slot 11 alone
        let proxy = placed(&mut w, &[(11, 0.0, 40.0)]);
        let mut log = PassLog::default();
        SelectForzaDrawn::new(5.0, 1)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        assert!(w.is_alive(full));
        assert!(!w.is_alive(impostor) && !w.is_alive(proxy));
        let mut far = World::new();
        let impostor = placed(&mut far, &[(1, 0.0, 400.0), (13, 225.0, 1000.0)]);
        SelectForzaDrawn::new(300.0, 1)
            .unwrap()
            .run(&mut far, &mut log)
            .unwrap();
        assert!(
            far.is_alive(impostor),
            "drawn where its main-view range holds the distance"
        );
    }
}
