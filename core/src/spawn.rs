//! `place-spawn`: a spawn marker the caller asks for, placed on the read world before a writer plans.

use crate::pass::{Options, Pass, PassLog};
use crate::{Instance, Marker, Name, Transform, World};

/// Where [`PlaceSpawn`] puts the spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpawnAt {
    /// The per-axis median of the placements' ground positions: a few placements far from the rest do not move it
    /// off the map.
    Centre,
    /// This ground position `(east, north)`, metres ([`crate::frame::ground`]).
    Ground([f32; 2]),
}

/// `place-spawn{at=centre (the default) | east=E,north=N}[,yaw=DEGREES]}`: a `core.marker` of kind `spawn` at a ground position
/// (its height left to the consumer, which snaps it), facing `yaw` ([`Marker::yaw`]). The caller states where; a
/// world whose source already places a spawn is refused (the source's own stands). It runs as a selection pass, on
/// the world a writer plans from, so one spawn serves every part of a conversion; the marker is content of its own
/// ([`PassLog::place`]). Made by [`PlaceSpawn::centre`] or [`PlaceSpawn::at_ground`], facing [`PlaceSpawn::with_yaw`];
/// from options by the same calls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceSpawn {
    at: SpawnAt,
    yaw: Option<f32>,
}

impl PlaceSpawn {
    /// A spawn at the median of the placements' ground positions.
    pub fn centre() -> PlaceSpawn {
        PlaceSpawn {
            at: SpawnAt::Centre,
            yaw: None,
        }
    }

    /// A spawn at this ground position; finite metres.
    pub fn at_ground(east: f32, north: f32) -> Result<PlaceSpawn, String> {
        if !(east.is_finite() && north.is_finite()) {
            return Err(format!("({east}, {north}) is not a finite ground position"));
        }
        Ok(PlaceSpawn {
            at: SpawnAt::Ground([east, north]),
            yaw: None,
        })
    }

    /// Facing `degrees` (finite) about the up axis ([`Marker::yaw`]).
    pub fn with_yaw(mut self, degrees: f32) -> Result<PlaceSpawn, String> {
        if !degrees.is_finite() {
            return Err(format!("yaw {degrees} is not finite"));
        }
        self.yaw = Some(degrees);
        Ok(self)
    }

    pub fn at(&self) -> SpawnAt {
        self.at
    }

    pub fn yaw(&self) -> Option<f32> {
        self.yaw
    }

    /// From its options (`at=centre`, or `east` and `north`; `yaw`), through the same constructors.
    pub fn from_options(o: &Options) -> Result<PlaceSpawn, String> {
        let number = |k: &str, v: &str| v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number"));
        let (mut centre, mut east, mut north, mut yaw) = (false, None, None, None);
        for (k, v) in o {
            match k.as_str() {
                "at" if v == "centre" => centre = true,
                "at" => return Err(format!("at={v}: only at=centre (or east, north)")),
                "east" => east = Some(number(k, v)?),
                "north" => north = Some(number(k, v)?),
                "yaw" => yaw = Some(number(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        let pass = match (centre, east, north) {
            (_, None, None) => PlaceSpawn::centre(),
            (false, Some(e), Some(n)) => PlaceSpawn::at_ground(e, n)?,
            (true, _, _) => return Err("at=centre or east, north: not both".into()),
            _ => return Err("east and north go together".into()),
        };
        match yaw {
            Some(y) => pass.with_yaw(y),
            None => Ok(pass),
        }
    }
}

/// [`PlaceSpawn::from_options`] as a pass factory.
pub fn place_spawn(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PlaceSpawn::from_options(o)?))
}

impl Pass for PlaceSpawn {
    fn name(&self) -> &'static str {
        "place-spawn"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        match self.at {
            SpawnAt::Centre => {
                o.insert("at".into(), "centre".into());
            }
            SpawnAt::Ground([e, n]) => {
                o.insert("east".into(), e.to_string());
                o.insert("north".into(), n.to_string());
            }
        }
        if let Some(y) = self.yaw {
            o.insert("yaw".into(), y.to_string());
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &["core.marker", "core.name"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if let Some((e, _)) = w.iter::<Marker>().find(|(_, m)| m.kind == "spawn") {
            return Err(format!(
                "the world already has a spawn ({e:?}): the source's own stands; leave out place-spawn"
            ));
        }
        let ground = match self.at {
            SpawnAt::Ground(g) => g,
            SpawnAt::Centre => {
                let (mut east, mut north): (Vec<f32>, Vec<f32>) = w
                    .iter::<Instance>()
                    .map(|(e, _)| {
                        let t = w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
                        let [x, n] = crate::frame::ground(t.cols[3]);
                        (x, n)
                    })
                    .unzip();
                if east.is_empty() {
                    return Err("place-spawn at=centre: the world has no placement to centre on".into());
                }
                east.sort_by(f32::total_cmp);
                north.sort_by(f32::total_cmp);
                [median(&east), median(&north)]
            }
        };
        let e = w.spawn();
        w.insert(e, Name("spawn".into()));
        w.insert(
            e,
            Marker {
                kind: "spawn".into(),
                ground: Some(ground),
                yaw: self.yaw,
            },
        );
        log.place(e);
        log.count("spawn markers placed", 1);
        Ok(())
    }
}

/// The median of sorted, nonempty `v` (the mean of the middle two for an even count).
fn median(v: &[f32]) -> f32 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::{PassManager, PassRegistry, Pipeline, RunReport};

    fn placed(w: &mut World, east: f32, north: f32) {
        let proto = w.spawn();
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        // ground (east, north) is (x, -z)
        t.cols[3] = [east, 0.0, -north];
        w.insert(e, t);
    }

    fn run(w: &mut World, text: &str) -> Result<RunReport, String> {
        let mut reg = PassRegistry::new();
        reg.register("place-spawn", place_spawn);
        let pipeline: Pipeline = reg.parse(text).map_err(|e| e.to_string())?;
        let mut report = RunReport::default();
        PassManager::new(reg)
            .run(w, &pipeline, &mut report)
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    /// Worked by hand: placements at east 0, 10, 20 and 1,000 (an outlier) and north 0, 0, 40, 40 have medians east 15
    /// (the mean of 10 and 20) and north 20; the spawn is a ground marker there, facing yaw 90, placed by the pass.
    #[test]
    fn the_centre_is_the_median_of_the_placements() {
        let mut w = World::new();
        for (e, n) in [(0., 0.), (10., 0.), (20., 40.), (1000., 40.)] {
            placed(&mut w, e, n);
        }
        let report = run(&mut w, "place-spawn{at=centre,yaw=90}").unwrap();
        let (e, m) = w.iter::<Marker>().next().unwrap();
        assert_eq!(
            m,
            &Marker {
                kind: "spawn".into(),
                ground: Some([15.0, 20.0]),
                yaw: Some(90.0),
            }
        );
        assert!(!w.has::<Transform>(e));
        assert!(report.steps[0].log.placed.contains(&e));
        assert!(w.verify_components().is_empty());
    }

    #[test]
    fn a_given_position_a_source_spawn_and_bad_options() {
        let mut w = World::new();
        run(&mut w, "place-spawn{east=3,north=-4}").unwrap();
        let (_, m) = w.iter::<Marker>().next().unwrap();
        assert_eq!((m.ground, m.yaw), (Some([3.0, -4.0]), None));
        // the source's own spawn stands
        assert!(
            run(&mut w, "place-spawn{at=centre}")
                .unwrap_err()
                .contains("already has a spawn")
        );
        // nothing to centre on
        // a position or yaw that is not a finite number is refused, typed or from options
        assert!(PlaceSpawn::at_ground(f32::NAN, 0.0).is_err());
        assert!(PlaceSpawn::centre().with_yaw(f32::INFINITY).is_err());
        assert!(run(&mut World::new(), "place-spawn{east=inf,north=0}").is_err());
        assert!(
            run(&mut World::new(), "place-spawn{at=centre}")
                .unwrap_err()
                .contains("no placement")
        );
        for bad in [
            "place-spawn{east=3}",
            "place-spawn{at=centre,east=1,north=2}",
            "place-spawn{at=middle}",
            "place-spawn{yaw=nan,at=centre}",
            "place-spawn{size=2}",
        ] {
            assert!(run(&mut World::new(), bad).is_err(), "{bad}");
        }
    }
}
