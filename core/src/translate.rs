//! `translate-world{by=X,Y,Z}`: the whole world moved by a stated offset (core's frame, metres), as a program does to
//! bring an area far from its source's origin near a destination's (a region several kilometres out, for an engine
//! whose precision or bounds want it near zero). What moves is core's world-space data: every entity's transform and
//! every transform a state override gives, every curve's points and handles, every ground marker's position, and a
//! partition plan's ground areas ([`crate::convert::PartitionPlan`], so its parts still read where their content is); local
//! geometry (prototypes' meshes, collision, colliders) stays as it is. A dialect component that holds world positions
//! isn't core's to move: lower it first.

use crate::convert::PartitionPlan;
use crate::marker::{Curve, Marker};
use crate::pass::{Options, Pass, PassLog};
use crate::state::{Override, StateOverride};
use crate::{Component, Entity, Resource, Transform, World};

/// The pass: the world moved by `by`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranslateWorld {
    pub by: [f64; 3],
}

/// The pass factory (`PassRegistry::register("translate-world", translate_world)`): `by=X,Y,Z`, required.
pub fn translate_world(o: &Options) -> Result<Box<dyn Pass>, String> {
    if let Some(k) = o.keys().find(|k| k.as_str() != "by") {
        return Err(format!("unknown option {k}"));
    }
    let by = o
        .get("by")
        .ok_or("by=X,Y,Z is required: the offset is the program's choice")?;
    let v: Vec<f64> = by
        .split(',')
        .map(|x| {
            x.trim()
                .parse::<f64>()
                .map_err(|_| format!("by: {x} is not a number"))
        })
        .collect::<Result<_, _>>()?;
    let [x, y, z] = v[..] else {
        return Err("by takes three numbers, X,Y,Z".into());
    };
    if ![x, y, z].iter().all(|v| v.is_finite()) {
        return Err(format!("by {x},{y},{z}: finite numbers"));
    }
    Ok(Box::new(TranslateWorld { by: [x, y, z] }))
}

impl TranslateWorld {
    /// `p` moved, in f64 and rounded once.
    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| (f64::from(p[i]) + self.by[i]) as f32)
    }

    fn transform(&self, t: &mut Transform) {
        t.cols[3] = self.point(t.cols[3]);
    }
}

impl Pass for TranslateWorld {
    fn name(&self) -> &'static str {
        "translate-world"
    }
    fn options(&self) -> Options {
        let [x, y, z] = self.by;
        Options::from([("by".to_owned(), format!("{x},{y},{z}"))])
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            Transform::NAME,
            Curve::NAME,
            Marker::NAME,
            StateOverride::NAME,
            PartitionPlan::NAME,
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let placed: Vec<Entity> = w.iter::<Transform>().map(|(e, _)| e).collect();
        for &e in &placed {
            if let Some(t) = w.get_mut::<Transform>(e) {
                self.transform(t);
            }
        }
        log.count("transforms moved", placed.len() as u64);
        let overridden: Vec<Entity> = w.iter::<StateOverride>().map(|(e, _)| e).collect();
        for e in overridden {
            if let Some(o) = w.get_mut::<StateOverride>(e) {
                for (_, change) in &mut o.0 {
                    if let Override::Transform(t) = change {
                        self.transform(t);
                        log.count("override transforms moved", 1);
                    }
                }
            }
        }
        let curves: Vec<Entity> = w.iter::<Curve>().map(|(e, _)| e).collect();
        for e in curves {
            if let Some(c) = w.get_mut::<Curve>(e) {
                for p in &mut c.points {
                    *p = self.point(*p);
                }
                for handles in c.tangents.iter_mut().flatten() {
                    *handles = handles.map(|h| self.point(h));
                }
                log.count("curves moved", 1);
            }
        }
        // a ground position is (east, north) = (x, -z)
        let [east, north] = [self.by[0], -self.by[2]];
        let markers: Vec<Entity> = w.iter::<Marker>().map(|(e, _)| e).collect();
        for e in markers {
            if let Some(Marker { ground: Some(g), .. }) = w.get_mut::<Marker>(e) {
                *g = [(f64::from(g[0]) + east) as f32, (f64::from(g[1]) + north) as f32];
                log.count("ground markers moved", 1);
            }
        }
        // a plan's areas are ground rectangles [west, south, east, north]: they move with what they hold
        if let Some(plan) = w.resource_mut::<PartitionPlan>() {
            for area in plan.groups.iter_mut().filter_map(|g| g.area.as_mut()) {
                *area = [area[0] + east, area[1] + north, area[2] + east, area[3] + north];
                log.count("partition areas moved", 1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placement_a_curve_and_a_ground_marker_move_by_the_offset() {
        let mut w = World::new();
        let placed = w.spawn();
        w.insert(
            placed,
            Transform {
                cols: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [-3500.0, 2330.0, 1200.0],
                ],
            },
        );
        let curve = w.spawn();
        w.insert(
            curve,
            Curve {
                kind: "rail".into(),
                points: vec![[-3500.0, 2330.0, 1200.0], [-3490.0, 2331.0, 1200.0]],
                closed: false,
                tangents: None,
                up: None,
            },
        );
        let marker = w.spawn();
        w.insert(
            marker,
            Marker {
                kind: "spawn".into(),
                ground: Some([-3500.0, -1200.0]),
                yaw: None,
            },
        );
        w.insert_resource(PartitionPlan {
            by: "partition-grid{cell=200}".into(),
            groups: vec![crate::convert::PartitionGroup {
                key: "cell".into(),
                area: Some([-3600.0, -1300.0, -3400.0, -1100.0]),
                ..Default::default()
            }],
        });
        let pass = TranslateWorld {
            by: [3500.0, -2330.0, -1200.0],
        };
        pass.run(&mut w, &mut PassLog::default()).expect("run");
        assert_eq!(
            w.get::<Transform>(placed).map(|t| t.cols[3]),
            Some([0.0, 0.0, 0.0])
        );
        assert_eq!(
            w.get::<Curve>(curve).map(|c| c.points.clone()),
            Some(vec![[0.0, 0.0, 0.0], [10.0, 1.0, 0.0]])
        );
        // east -3500 + 3500, north -1200 - (-1200)
        assert_eq!(w.get::<Marker>(marker).and_then(|m| m.ground), Some([0.0, 0.0]));
        // the plan's area moves the same: east +3500, north +1200
        assert_eq!(
            w.resource::<PartitionPlan>().unwrap().groups[0].area,
            Some([-100.0, -100.0, 100.0, 100.0])
        );
    }
}
