//! `select-reskate-lights{per_cell=N,cell=M}`: at most `N` skate lights in each `M`-metre ground cell (cells at
//! multiples of `M` from the origin, half-open), the rest left out (counted). In a cell, lights of the default group
//! (`""`) go first, then the highest priority (a light's own, else energy times range), then a total order over every
//! field ([`crate::scene::light_order`]), so which lights stay never depends on the order they arrived in. Both options
//! are required: a cap and its grouping are the program's choice, and with no such pass every light is written.
//!
//! The cap is the map's, over all its lights: a conversion in more than one part is refused (a part with context, or a
//! partition plan of more than one group), since each part would cap its own cells on its own.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, World};

use crate::dialect::SkateLight;
use crate::scene::{PlacedLight, light_order, placed_light};

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

    /// From its options (`per_cell` and `cell`, both required), through [`SelectReskateLights::new`].
    pub fn from_options(o: &Options) -> Result<SelectReskateLights, String> {
        if let Some(k) = o.keys().find(|k| !["per_cell", "cell"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let per_cell = o
            .get("per_cell")
            .ok_or("per_cell is required: a light cap is the program's choice")?;
        let per_cell = per_cell
            .parse()
            .map_err(|_| format!("per_cell={per_cell} is not a count"))?;
        let cell = o
            .get("cell")
            .ok_or("cell is required (metres): the cap's grouping is the program's choice")?;
        let cell = cell
            .parse()
            .map_err(|_| format!("cell={cell} is not a number of metres"))?;
        SelectReskateLights::new(per_cell, cell)
    }
}

/// [`SelectReskateLights::from_options`] as a pass factory.
pub fn select_reskate_lights(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SelectReskateLights::from_options(o)?))
}

/// The indices of `lights` a cap of `per_cell` per `cell`-metre ground cell keeps (see the module documentation), in
/// [`light_order`].
pub fn keep_lights(lights: &[PlacedLight], per_cell: usize, cell: f32) -> Vec<usize> {
    let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, l) in lights.iter().enumerate() {
        let k = ((l.pos.x / cell).floor() as i64, (l.pos.y / cell).floor() as i64);
        cells.entry(k).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, mut g) in cells {
        g.sort_by(|&a, &b| {
            let (a, b) = (&lights[a], &lights[b]);
            (!a.group.is_empty())
                .cmp(&!b.group.is_empty())
                .then(b.priority.total_cmp(&a.priority))
                .then_with(|| light_order(a, b))
        });
        out.extend(g.into_iter().take(per_cell));
    }
    out.sort_by(|&a, &b| light_order(&lights[a], &lights[b]));
    out
}

/// An error when `w` is one part of a conversion in several (see the module documentation).
fn one_map(w: &World, pass: &str) -> Result<(), String> {
    let context = w
        .resource::<primase_core::convert::PartScope>()
        .is_some_and(|s| !s.context.is_empty());
    let groups = w
        .resource::<primase_core::convert::PartitionPlan>()
        .map_or(0, |p| p.groups.len());
    if context || groups > 1 {
        return Err(format!(
            "{pass} works on the whole map: this is one part of a conversion in several"
        ));
    }
    Ok(())
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
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        one_map(w, self.name())?;
        let lights: Vec<(Entity, PlacedLight)> = w
            .iter::<SkateLight>()
            .map(|(e, l)| (e, placed_light(w, e, l)))
            .collect();
        let placed: Vec<PlacedLight> = lights.iter().map(|(_, l)| l.clone()).collect();
        let mut keep = vec![false; lights.len()];
        for i in keep_lights(&placed, self.per_cell, self.cell) {
            keep[i] = true;
        }
        let mut left_out = 0;
        for ((e, _), kept) in lights.iter().zip(keep) {
            if !kept {
                w.despawn(*e);
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

    /// Both options are required and checked; the struct and its options are one; a part of a conversion in several
    /// is refused.
    #[test]
    fn options_are_required_and_parts_refused() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(SelectReskateLights::from_options(&o(&[("per_cell", "8")])).is_err());
        assert!(SelectReskateLights::from_options(&o(&[("cell", "200")])).is_err());
        assert!(SelectReskateLights::from_options(&o(&[("per_cell", "8"), ("cell", "0")])).is_err());
        assert!(SelectReskateLights::new(8, f32::NAN).is_err());
        let full = o(&[("per_cell", "8"), ("cell", "200")]);
        assert_eq!(SelectReskateLights::from_options(&full).unwrap().options(), full);
        let mut w = World::new();
        let neighbour = w.spawn();
        w.insert_resource(primase_core::convert::PartScope {
            roots: Default::default(),
            context: [neighbour].into(),
        });
        assert!(
            SelectReskateLights::new(8, 200.0)
                .unwrap()
                .run(&mut w, &mut PassLog::default())
                .is_err()
        );
    }
}
