//! `drop-marker`: a `core.marker` with a transform, stood on the placed collision, for a source that states none of its
//! own (a skater's start, a travel point). Where, by `at=centre`, `under=X,Y,Z` or `x`, `y`, `z`:
//! - **centre**: the median of the placements' ground positions (as `place-spawn{at=centre}` takes it), on the highest
//!   collision under it;
//! - **under=x,y,z**: on the highest collision at or below that point (a placement's origin: no roof over it);
//! - **x, y, z**: a stated position (core's frame), kept as it is; there must be collision under it within
//!   [`STATED_DROP`] metres (a marker over nothing would drop the skater through the world). For a point picked in game:
//!   the highest collision under a point can be a roof, a tarp or a blimp over it.
//!
//! `place-spawn` is the other way to a spawn: a ground marker with no height, left to its consumer to snap, which a
//! consumer's street snap may not find ground for (a plaza far from any road). This pass gives the marker its height
//! and transform from the collision itself, for any kind (`spawn`, `travel`, ...). Facing: `yaw` degrees about +Y, 0
//! facing +Z and 90 facing +X ([`Marker::yaw`]), as the marker's transform turns its local +Z; none: unturned. A world
//! that already has a spawn is refused when the kind is `spawn` (the source's own stands).

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Marker, Name, Transform, World, centre_ground, kind_is_valid};

use crate::collision_query::Ground;

/// How far under a stated position collision must be, metres.
pub const STATED_DROP: f32 = 2.0;

/// Where [`DropMarker`] stands the marker: see the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerAt {
    /// The median of the placements' ground positions, on the highest collision under it.
    Centre,
    /// (x, y, z), metres, core's frame: kept, with collision within [`STATED_DROP`] under it.
    Stated([f32; 3]),
    /// On the highest collision at or below (x, y, z), core's frame: a placement's origin, so a roof over it is never
    /// chosen.
    Under([f32; 3]),
}

/// See the module documentation. Made by [`DropMarker::new`], facing [`DropMarker::with_yaw`]; from options by the same
/// calls.
#[derive(Clone, Debug, PartialEq)]
pub struct DropMarker {
    kind: String,
    name: String,
    at: MarkerAt,
    yaw: Option<f32>,
}

impl DropMarker {
    /// A marker of `kind` (a word of `a-z`, `0-9` and `_`) named `name`, at `at` (finite metres).
    pub fn new(kind: &str, name: &str, at: MarkerAt) -> Result<DropMarker, String> {
        if !kind_is_valid(kind) {
            return Err(format!("kind {kind:?} is not a word of a-z, 0-9 and _"));
        }
        if let MarkerAt::Stated(p) | MarkerAt::Under(p) = at
            && !p.iter().all(|v| v.is_finite())
        {
            return Err(format!("{p:?} is not a finite position"));
        }
        Ok(DropMarker {
            kind: kind.into(),
            name: name.into(),
            at,
            yaw: None,
        })
    }

    /// Facing `degrees` (finite) about +Y ([`Marker::yaw`]).
    pub fn with_yaw(mut self, degrees: f32) -> Result<DropMarker, String> {
        if !degrees.is_finite() {
            return Err(format!("yaw {degrees} is not finite"));
        }
        self.yaw = Some(degrees);
        Ok(self)
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn at(&self) -> MarkerAt {
        self.at
    }

    pub fn yaw(&self) -> Option<f32> {
        self.yaw
    }

    /// From its options (`kind`, `name`, `at=centre`, `under=X,Y,Z` or `x`, `y` and `z`; `yaw`), through the same
    /// constructors.
    fn from_given(g: &Given) -> Result<DropMarker, String> {
        let number = |k: &str| {
            g.optional(k)
                .map(|v| v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let at = match (g.optional_choice::<At>("at"), g.optional("under"), number("x")?) {
            (Some(At::Centre), None, None) => MarkerAt::Centre,
            (None, Some(u), None) => MarkerAt::Under(point(u)?),
            (None, None, Some(x)) => match (number("y")?, number("z")?) {
                (Some(y), Some(z)) => MarkerAt::Stated([x, y, z]),
                _ => return Err("x, y and z go together".into()),
            },
            _ => return Err("at, under or x: one".into()),
        };
        if !matches!(at, MarkerAt::Stated(_)) && (g.optional("y").is_some() || g.optional("z").is_some()) {
            return Err("y and z go with x".into());
        }
        let pass = DropMarker::new(g.required("kind"), g.required("name"), at)?;
        match number("yaw")? {
            Some(y) => pass.with_yaw(y),
            None => Ok(pass),
        }
    }
}

/// `X,Y,Z`: three finite numbers.
fn point(text: &str) -> Result<[f32; 3], String> {
    let v: Vec<f32> = text
        .split(',')
        .map(|x| {
            x.trim()
                .parse::<f32>()
                .map_err(|_| format!("under: {x} is not a number"))
        })
        .collect::<Result<_, _>>()?;
    <[f32; 3]>::try_from(v).map_err(|_| "under takes three numbers, X,Y,Z".into())
}

/// `drop-marker`'s `at`: the one place it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    Centre,
}

impl Choice for At {
    const VALUES: &'static [(&'static str, Self)] = &[("centre", At::Centre)];
}

/// `drop-marker`'s declaration ([`primase_core::pass::PassRegistry::add`]): what the marker is and where it goes are the
/// program's, no default.
pub const DROP_MARKER: PassSpec = PassSpec {
    name: "drop-marker",
    options: &[
        OptionSpec::required("kind", "spawn", "the marker's kind: a word of a-z, 0-9 and _"),
        OptionSpec::required("name", "start", "the marker's name"),
        OptionSpec::one_of_choice::<At>(
            "place",
            "at",
            "centre: the median of the placements' ground positions, on the highest collision under it",
        ),
        OptionSpec::one_of(
            "place",
            "under",
            "0,100,0",
            "X,Y,Z, metres in core's frame: on the highest collision at or below that point",
        ),
        OptionSpec::one_of(
            "place",
            "x",
            "0",
            "metres, with y and z: this position, kept, with collision within 2 m under it",
        ),
        OptionSpec::optional("y", "0", "metres, with x", "required with x, not taken otherwise"),
        OptionSpec::optional("z", "0", "metres, with x", "required with x, not taken otherwise"),
        OptionSpec::optional(
            "yaw",
            "90",
            "degrees about +Y, 0 facing +Z and 90 facing +X: the marker's heading",
            "unturned",
        ),
    ],
    make: |g| Ok(Box::new(DropMarker::from_given(g)?)),
};

impl Pass for DropMarker {
    fn name(&self) -> &'static str {
        "drop-marker"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        o.insert("kind".into(), self.kind.clone());
        o.insert("name".into(), self.name.clone());
        match self.at {
            MarkerAt::Centre => {
                o.insert("at".into(), At::Centre.text().into());
            }
            MarkerAt::Stated([x, y, z]) => {
                o.insert("x".into(), x.to_string());
                o.insert("y".into(), y.to_string());
                o.insert("z".into(), z.to_string());
            }
            MarkerAt::Under([x, y, z]) => {
                o.insert("under".into(), format!("{x},{y},{z}"));
            }
        }
        if let Some(y) = self.yaw {
            o.insert("yaw".into(), y.to_string());
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &["core.marker", "core.name", "core.transform"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if self.kind == "spawn"
            && let Some((e, _)) = w.iter::<Marker>().find(|(_, m)| m.kind == "spawn")
        {
            return Err(format!(
                "the world already has a spawn ({e:?}): the source's own stands; leave out drop-marker"
            ));
        }
        let what = format!("drop-marker {} {:?}", self.kind, self.name);
        let ground = Ground::of(w);
        // the highest collision straight under ground (x, z), at or below `ceiling` (core +Y)
        let below = |x: f32, z: f32, ceiling: f32| {
            ground
                .surface_below([f64::from(x), f64::from(ceiling), f64::from(z)], 0.0)
                .map(|s| s.height as f32)
        };
        let at = match self.at {
            MarkerAt::Under([x, y, z]) => {
                let height =
                    below(x, z, y).ok_or_else(|| format!("{what}: no collision under ({x}, {y}, {z})"))?;
                [x, height, z]
            }
            MarkerAt::Stated([x, y, z]) => {
                // collision under it, near enough to stand on
                if below(x, z, y + 0.5).is_none_or(|h| h < y - STATED_DROP) {
                    return Err(format!(
                        "{what}: no collision within {STATED_DROP} m under ({x}, {y}, {z}): state a point over the ground"
                    ));
                }
                [x, y, z]
            }
            MarkerAt::Centre => {
                let [east, north] = centre_ground(w, "drop-marker")?;
                let height = below(east, -north, f32::INFINITY).ok_or_else(|| {
                    format!("{what}: no collision under ({east}, {north}): state a point over the ground")
                })?;
                [east, height, -north]
            }
        };
        let (sin, cos) = self.yaw.map_or((0.0, 1.0), |y| y.to_radians().sin_cos());
        let e = w.spawn();
        w.insert(e, Name(self.name.clone()));
        w.insert(
            e,
            Marker {
                kind: self.kind.clone(),
                ground: None,
                yaw: None,
            },
        );
        w.insert(
            e,
            Transform {
                cols: [[cos, 0.0, -sin], [0.0, 1.0, 0.0], [sin, 0.0, cos], at],
            },
        );
        log.place(e);
        log.count(&format!("{} markers placed", self.kind), 1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::{Collision, Instance};

    /// A placed 10 m square of collision at height `h` (core +Y) centred on ground (`east`, `north`).
    fn floor(w: &mut World, east: f32, north: f32, h: f32) {
        let proto = w.spawn();
        w.insert(
            proto,
            Collision {
                positions: vec![
                    [-5.0, 0.0, -5.0],
                    [5.0, 0.0, -5.0],
                    [5.0, 0.0, 5.0],
                    [-5.0, 0.0, 5.0],
                ],
                triangles: vec![[0, 2, 1], [0, 3, 2]],
                surfaces: vec![0, 0],
            },
        );
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, h, -north];
        w.insert(e, t);
    }

    fn run(w: &mut World, text: &str) -> Result<(), String> {
        let mut reg = PassRegistry::new();
        reg.add(&DROP_MARKER);
        let pipeline: Pipeline = reg.parse(text).map_err(|e| e.to_string())?;
        let mut report = RunReport::default();
        PassManager::new(reg)
            .run(w, &pipeline, &mut report)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn the_marker_stands_on_the_highest_collision_under_a_point() {
        let mut w = World::new();
        w.insert_resource(primase_core::Surfaces(vec![primase_core::Surface::new(
            "concrete",
        )]));
        floor(&mut w, 10.0, 20.0, 2.0);
        floor(&mut w, 10.0, 20.0, 7.5);
        floor(&mut w, 40.0, 20.0, 30.0);
        run(&mut w, "drop-marker{kind=spawn,name=spawn,under=11,100,-21}").unwrap();
        let (e, m) = w.iter::<Marker>().next().unwrap();
        assert_eq!(m.kind, "spawn");
        assert_eq!(w.get::<Transform>(e).unwrap().cols[3], [11.0, 7.5, -21.0]);
        assert!(w.verify_components().is_empty());
    }

    /// Worked by hand: the medians of the three placements' grounds are (2, 2), which all three floors cover; the
    /// highest is 3 m.
    #[test]
    fn the_centre_is_the_placements_median_ground_position() {
        let mut w = World::new();
        floor(&mut w, 0.0, 0.0, 1.0);
        floor(&mut w, 4.0, 2.0, 1.0);
        floor(&mut w, 2.0, 4.0, 3.0);
        run(&mut w, "drop-marker{kind=spawn,name=centre,at=centre}").unwrap();
        let (e, _) = w.iter::<Marker>().next().unwrap();
        assert_eq!(w.get::<Transform>(e).unwrap().cols[3], [2.0, 3.0, -2.0]);
    }

    /// A stated point under a roof stays where it is stated (the floor at 2 m, a roof at 7.5 m over it): not on the
    /// roof as the highest collision under it would be. Facing 90 degrees: its local +Z is +X. With no floor within
    /// 2 m under it, refused.
    #[test]
    fn a_stated_point_keeps_its_height_and_faces_its_yaw() {
        let mut w = World::new();
        floor(&mut w, 10.0, 20.0, 2.0);
        floor(&mut w, 10.0, 20.0, 7.5);
        run(
            &mut w,
            "drop-marker{kind=travel,name=Waypoint 1,x=11,y=2,z=-21,yaw=90}",
        )
        .unwrap();
        let (e, m) = w.iter::<Marker>().next().unwrap();
        assert_eq!(m.kind, "travel");
        assert_eq!(w.get::<Name>(e).unwrap().0, "Waypoint 1");
        let t = w.get::<Transform>(e).unwrap();
        assert_eq!(t.cols[3], [11.0, 2.0, -21.0]);
        let f = t.cols[2];
        assert!(
            (f[0] - 1.0).abs() < 1e-6 && f[1].abs() < 1e-6 && f[2].abs() < 1e-6,
            "{f:?}"
        );
        let mut w = World::new();
        floor(&mut w, 10.0, 20.0, 2.0);
        assert!(run(&mut w, "drop-marker{kind=spawn,name=s,x=11,y=5,z=-21}").is_err());
    }

    #[test]
    fn refusals() {
        let mut w = World::new();
        floor(&mut w, 10.0, 20.0, 2.0);
        // over no collision
        assert!(
            run(&mut w, "drop-marker{kind=spawn,name=s,under=100,50,0}")
                .unwrap_err()
                .contains("no collision under")
        );
        // the source's own spawn stands; a travel point is not a spawn
        run(&mut w, "drop-marker{kind=travel,name=t,under=11,50,-21}").unwrap();
        run(&mut w, "drop-marker{kind=spawn,name=s,under=11,50,-21}").unwrap();
        assert!(
            run(&mut w, "drop-marker{kind=spawn,name=s,under=11,50,-21}")
                .unwrap_err()
                .contains("already has a spawn")
        );
        for bad in [
            "drop-marker{kind=Spawn,name=s,at=centre}",
            "drop-marker{kind=spawn,name=s,x=1,y=2}",
            "drop-marker{kind=spawn,name=s,at=centre,y=2}",
            "drop-marker{kind=spawn,name=s,under=1,2}",
            "drop-marker{kind=spawn,name=s,under=1,2,nan}",
            "drop-marker{kind=spawn,name=s,at=centre,yaw=inf}",
            "drop-marker{kind=spawn,name=s,at=centre,under=1,2,3}",
            "drop-marker{kind=spawn,at=centre}",
            "drop-marker{kind=spawn,name=s}",
        ] {
            assert!(run(&mut World::new(), bad).is_err(), "{bad}");
        }
        // nothing to centre on
        assert!(
            run(&mut World::new(), "drop-marker{kind=spawn,name=s,at=centre}")
                .unwrap_err()
                .contains("no placement")
        );
    }
}
