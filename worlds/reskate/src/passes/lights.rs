//! `select-reskate-lights{per_cell=N,cell=M}`: at most `N` skate lights in each `M`-metre ground cell (cells at
//! multiples of `M` from the origin, half-open), the rest left out (counted). In a cell, lights of the default group
//! (`""`) go first, then the highest priority (a light's own, else energy times range), then a total order over every
//! field ([`crate::scene::light_order`]), so which lights stay never depends on the order they arrived in. Both options
//! are required: a cap and its grouping are the program's choice, and with no such pass every light is written.
//!
//! The cap is the map's, over all its lights. A map converted in parts caps them over every part:
//! `summarize-reskate-lights` with the same cap records what each part's cap keeps of the lights it owns
//! (`skate.lights_summary`), the summaries merge into what the cap keeps of every light, and this pass keeps those of
//! each part's own lights (a summary for another cap is refused). A part that is not the whole and has no summary is
//! refused. The whole map capped at once goes through the same summary.

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Entity, World};

use super::Owned;
use crate::components::{RankedLight, SkateLight, SkateLightsSummary};
use crate::scene::{PlacedLight, light_fields, light_order, placed_light};

/// See the module documentation. Made by [`SelectReskateLights::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectReskateLights {
    per_cell: usize,
    cell: f32,
}

impl SelectReskateLights {
    /// At most `per_cell` lights (0 leaves out every light) in each `cell`-metre ground cell (finite, above 0).
    pub fn new(per_cell: usize, cell: f32) -> Result<SelectReskateLights, String> {
        if !(cell.is_finite() && cell > 0.0) {
            return Err(format!("cell {cell} is not a positive number of metres"));
        }
        Ok(SelectReskateLights { per_cell, cell })
    }

    pub fn per_cell(&self) -> usize {
        self.per_cell
    }

    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// From its options, through [`SelectReskateLights::new`].
    fn from_given(g: &Given) -> Result<SelectReskateLights, String> {
        let per_cell = g.required("per_cell");
        let per_cell = per_cell
            .parse()
            .map_err(|_| format!("per_cell={per_cell} is not a count"))?;
        let cell = g.required("cell");
        let cell = cell
            .parse()
            .map_err(|_| format!("cell={cell} is not a number of metres"))?;
        SelectReskateLights::new(per_cell, cell)
    }
}

/// `per_cell` and `cell`, as `select-reskate-lights` and `summarize-reskate-lights` take them: the program's, no
/// default.
const CAP_OPTIONS: [OptionSpec; 2] = [
    OptionSpec::required(
        "per_cell",
        "1",
        "a count: the most lights kept in each cell (0 leaves out every light)",
    ),
    OptionSpec::required(
        "cell",
        "200",
        "metres above 0: the edge of the ground cells the cap groups lights by",
    ),
];

/// `select-reskate-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): a light cap and its grouping are
/// the program's, no default.
pub const SELECT_RESKATE_LIGHTS: PassSpec = PassSpec {
    name: "select-reskate-lights",
    options: &CAP_OPTIONS,
    make: |g| Ok(Box::new(SelectReskateLights::from_given(g)?)),
};

/// Light `l` as the cap ranks it, `id` naming it.
fn ranked(l: &PlacedLight, id: u64) -> RankedLight {
    RankedLight {
        id,
        default_group: l.group.is_empty(),
        priority: l.priority,
        fields: light_fields(l),
        spot: l.spot,
        time_of_day: l.time_of_day,
        cast_shadow: l.cast_shadow,
        group: l.group.clone(),
        source: l.source.clone(),
    }
}

/// The indices of `lights` a cap of `per_cell` per `cell`-metre ground cell keeps (see the module documentation), in
/// [`light_order`].
pub fn keep_lights(lights: &[PlacedLight], per_cell: usize, cell: f32) -> Vec<usize> {
    let ranked: Vec<RankedLight> = lights
        .iter()
        .enumerate()
        .map(|(i, l)| ranked(l, i as u64))
        .collect();
    let mut out: Vec<usize> = SkateLightsSummary::of(per_cell, cell, ranked)
        .kept()
        .into_iter()
        .map(|i| i as usize)
        .collect();
    out.sort_by(|&a, &b| light_order(&lights[a], &lights[b]));
    out
}

/// What a cap of `per_cell` per `cell` metres keeps of the lights `owned` owns, each named by its owned key.
fn summarize(w: &World, per_cell: usize, cell: f32, owned: &Owned) -> SkateLightsSummary {
    let lights: Vec<RankedLight> = w
        .iter::<SkateLight>()
        .filter_map(|(e, l)| owned.key(e).map(|k| ranked(&placed_light(w, e, l), k)))
        .collect();
    SkateLightsSummary::of(per_cell, cell, lights)
}

impl Pass for SelectReskateLights {
    fn name(&self) -> &'static str {
        "select-reskate-lights"
    }
    fn options(&self) -> Options {
        Options::from([
            ("per_cell".to_owned(), self.per_cell.to_string()),
            ("cell".to_owned(), self.cell.to_string()),
        ])
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // what the cap keeps over every part (the merged summary), for this cap; else, on the whole map, here
        let summary = match w.resource::<SkateLightsSummary>() {
            Some(s) => {
                if (s.per_cell, s.cell.to_bits()) != (self.per_cell, self.cell.to_bits()) {
                    return Err(format!(
                        "select-reskate-lights: the lights were summarized for {} per {} m cell, this pass caps {} per \
                         {} m",
                        s.per_cell, s.cell, self.per_cell, self.cell
                    ));
                }
                s.clone()
            }
            None => {
                super::require_whole_map(w, self.name()).map_err(|e| {
                    format!(
                        "{e}: a map in parts caps its lights over every part (summarize-reskate-lights with the same \
                         cap)"
                    )
                })?;
                summarize(w, self.per_cell, self.cell, &Owned::all())
            }
        };
        let kept = summary.kept();
        let lights: Vec<(Entity, u64)> = {
            let owned = Owned::of(w);
            w.iter::<SkateLight>()
                .filter_map(|(e, _)| owned.key(e).map(|k| (e, k)))
                .collect()
        };
        let mut left_out = 0;
        for &(e, k) in &lights {
            if !kept.contains(&k) {
                w.despawn(e);
                left_out += 1;
            }
        }
        log.count("lights kept", (lights.len() - left_out) as u64);
        log.count(
            &format!(
                "lights left out: more than {} in their {} m cell",
                self.per_cell, self.cell
            ),
            left_out as u64,
        );
        Ok(())
    }
}

/// `summarize-reskate-lights{per_cell,cell}`: what a cap keeps of the lights this part owns, per cell
/// (`skate.lights_summary`); merged over the parts, it is what [`SelectReskateLights`] with the same cap keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeReskateLights(SelectReskateLights);

impl SummarizeReskateLights {
    /// For the cap `select-reskate-lights` applies.
    pub fn new(cap: SelectReskateLights) -> SummarizeReskateLights {
        SummarizeReskateLights(cap)
    }
}

/// `summarize-reskate-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): `select-reskate-lights`' cap.
pub const SUMMARIZE_RESKATE_LIGHTS: PassSpec = PassSpec {
    name: "summarize-reskate-lights",
    options: &CAP_OPTIONS,
    make: |g| {
        Ok(Box::new(SummarizeReskateLights(SelectReskateLights::from_given(
            g,
        )?)))
    },
};

impl Pass for SummarizeReskateLights {
    fn name(&self) -> &'static str {
        "summarize-reskate-lights"
    }
    fn options(&self) -> Options {
        self.0.options()
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.lights_summary"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let s = summarize(w, self.0.per_cell, self.0.cell, &Owned::of(w));
        log.count("lights a part's cap keeps", s.kept().len() as u64);
        w.insert_resource(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(w: &mut World, x: f32, z: f32, energy: f32, group: &str) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            SkateLight {
                spot: false,
                position: [x, 0.0, z],
                direction: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                color: [1.0; 3],
                energy,
                range: 10.0,
                radius: 0.1,
                spot_size: 0.0,
                spot_blend: 0.0,
                time_of_day: 127,
                cast_shadow: false,
                priority: None,
                group: group.into(),
                unknown_flags: 0,
            },
        );
        e
    }

    fn energies(w: &World) -> Vec<f32> {
        let mut v: Vec<f32> = w.iter::<SkateLight>().map(|(_, l)| l.energy).collect();
        v.sort_by(f32::total_cmp);
        v
    }

    /// Worked by hand: two 200 m cells (scene x 10, 20, 30 and 250; z -10 is north 10, the same cell). One per cell:
    /// the first cell keeps its highest priority (energy 3), unless a light of the default group is there, which goes
    /// first whatever its priority; a zero cap leaves out every light; without the pass every light stays.
    #[test]
    fn a_cap_per_cell_keeps_the_default_group_then_the_highest_priority() {
        let world = |grouped: bool| {
            let mut w = World::new();
            light(&mut w, 10.0, -10.0, 1.0, "");
            light(&mut w, 20.0, -10.0, 3.0, if grouped { "lamps" } else { "" });
            light(&mut w, 30.0, -10.0, 2.0, "lamps");
            light(&mut w, 250.0, -10.0, 4.0, "");
            w
        };
        let mut w = world(false);
        let mut log = PassLog::default();
        SelectReskateLights::new(1, 200.0)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(energies(&w), [3.0, 4.0]);
        assert_eq!(log.counts["lights left out: more than 1 in their 200 m cell"], 2);
        let mut w = world(true);
        SelectReskateLights::new(1, 200.0)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(energies(&w), [1.0, 4.0]);
        let mut w = world(false);
        SelectReskateLights::new(0, 200.0)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert!(energies(&w).is_empty());
        // a 1 km cell holds all four
        let mut w = world(false);
        SelectReskateLights::new(2, 1000.0)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(energies(&w), [3.0, 4.0]);
    }

    /// Both options are required and checked; the struct and its options are one.
    #[test]
    fn options_are_required_and_checked() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(SELECT_RESKATE_LIGHTS.instance(&o(&[("per_cell", "8")])).is_err());
        assert!(SELECT_RESKATE_LIGHTS.instance(&o(&[("cell", "200")])).is_err());
        assert!(
            SELECT_RESKATE_LIGHTS
                .instance(&o(&[("per_cell", "8"), ("cell", "0")]))
                .is_err()
        );
        assert!(SelectReskateLights::new(8, f32::NAN).is_err());
        let full = o(&[("per_cell", "8"), ("cell", "200")]);
        assert_eq!(SELECT_RESKATE_LIGHTS.instance(&full).unwrap().options(), full);
    }
}
