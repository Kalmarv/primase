//! `place-skatebird-markers{fallback=X,Y,Z,YAW|none,drop=M,margin=M,travel=N,play-max-extent=M}`: the level's spawn
//! and travel points as `core.marker`s, from the `ResetPosition`s the reader read ([`SkatebirdResetPosition`]); every
//! option is the program's, stated (no default). Run it after the level's state is resolved (what is stored inactive is
//! gone) and its collision lowered.
//!
//! - The spawn is the `ResetPosition` stored as the level's start (`startingPosition`). A level without one (level7)
//!   takes `fallback`, a point in the core frame (metres) and a yaw (degrees, 0 facing +Z, 90 +X; a Unity yaw `a` is
//!   `180 - a`), dropped to the first collision surface below it, a surface up to `drop` metres above it counting
//!   (`fallback=none` refuses such a level). The fallback and its drop are counted.
//! - The travel points are the `ResetPosition`s inside the play space ([`primase_meshops::collision_query`]: the box
//!   of every placed collision soup no wider than `play-max-extent` metres on any axis, grown by `margin` metres), the
//!   starting one first, then each the one farthest from those taken, `travel` in all. Mission givers are not used.
//! - Each marker is its own entity: kind `spawn` or `travel`, placed by a transform at the point facing the
//!   `ResetPosition`'s forward (Unity's +Z, the core transform's -Z column), so `raise-skate` gives it the game's own
//!   yaw (level2's start Unity 172.9341 is scene 7.0659), `core.name` its GameObject's.

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Marker, Name, Transform, World};

use crate::components::SkatebirdResetPosition;

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceSkatebirdMarkers {
    /// The spawn of a level without a starting `ResetPosition`: core-frame point and yaw (degrees); `None` refuses one.
    pub fallback: Option<([f64; 3], f64)>,
    /// Metres above the fallback a surface may be and still be the one it drops to.
    pub drop: f64,
    /// Metres around the play space a travel point may lie.
    pub margin: f64,
    /// Travel points in all.
    pub travel: usize,
    /// Metres: a collision soup wider than this on an axis is not play space.
    pub play_max_extent: f64,
}

fn number(k: &str, v: &str) -> Result<f64, String> {
    v.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("{k}={v}: not a finite number"))
}

impl PlaceSkatebirdMarkers {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<PlaceSkatebirdMarkers, String> {
        let fallback = match g.required("fallback") {
            "none" => None,
            v => {
                let n = (v.split(','))
                    .map(|x| number("fallback", x.trim()))
                    .collect::<Result<Vec<_>, _>>()?;
                let [x, y, z, yaw] = n[..] else {
                    return Err(format!("fallback={v}: x,y,z,yaw or none"));
                };
                Some(([x, y, z], yaw))
            }
        };
        let travel = g.required("travel");
        let pass = PlaceSkatebirdMarkers {
            fallback,
            drop: number("drop", g.required("drop"))?,
            margin: number("margin", g.required("margin"))?,
            travel: travel.parse().map_err(|_| format!("travel={travel}: a count"))?,
            play_max_extent: number("play-max-extent", g.required("play-max-extent"))?,
        };
        pass.check()?;
        Ok(pass)
    }

    fn check(&self) -> Result<(), String> {
        for (k, v) in [
            ("drop", self.drop),
            ("margin", self.margin),
            ("play-max-extent", self.play_max_extent),
        ] {
            if !(v.is_finite() && v >= 0.0) {
                return Err(format!("{k}={v}: metres, 0 or more"));
            }
        }
        Ok(())
    }
}

/// `place-skatebird-markers`' declaration ([`primase_core::pass::PassRegistry::add`]): every option is the program's,
/// stated.
pub const PLACE_SKATEBIRD_MARKERS: PassSpec = PassSpec {
    name: "place-skatebird-markers",
    options: &[
        OptionSpec::required(
            "fallback",
            "0,1.189,0,7.0659",
            "the spawn of a level without a starting point: x,y,z (core frame, metres),yaw (degrees), or none to refuse \
             one",
        ),
        OptionSpec::required(
            "drop",
            "0.05",
            "metres, 0 or more: how far above the fallback a surface may be and still be the one it drops to",
        ),
        OptionSpec::required(
            "margin",
            "10",
            "metres, 0 or more: how far around the play space a travel point may lie",
        ),
        OptionSpec::required("travel", "5", "the travel points in all, a count"),
        OptionSpec::required(
            "play-max-extent",
            "100",
            "metres, 0 or more: a collision soup wider than this on an axis is not play space",
        ),
    ],
    make: |g| Ok(Box::new(PlaceSkatebirdMarkers::from_given(g)?)),
};

/// A point: its name, position (core frame) and yaw (degrees, 0 facing +Z, 90 +X).
#[derive(Clone, Debug, PartialEq)]
struct Point {
    name: String,
    position: [f64; 3],
    yaw: f64,
}

/// A `ResetPosition`'s point: its translation, facing its GameObject's forward (Unity's +Z, the core transform's -Z
/// column).
fn point(name: String, t: &Transform) -> Point {
    let [x, _, z] = t.cols[2];
    Point {
        name,
        position: t.cols[3].map(f64::from),
        yaw: (-f64::from(x)).atan2(-f64::from(z)).to_degrees(),
    }
}

/// The marker's transform: at the point, its local +Z facing the yaw.
fn transform(p: &Point) -> Transform {
    let (s, c) = p.yaw.to_radians().sin_cos();
    let (s, c) = (s as f32, c as f32);
    Transform {
        cols: [
            [c, 0.0, -s],
            [0.0, 1.0, 0.0],
            [s, 0.0, c],
            p.position.map(|v| v as f32),
        ],
    }
}

/// At most `n` of `pool`, the first and then each the one farthest from those taken.
fn spread(pool: Vec<Point>, n: usize) -> Vec<Point> {
    if pool.len() <= n {
        return pool;
    }
    let d2 = |a: &Point, b: &Point| {
        (0..3)
            .map(|i| (a.position[i] - b.position[i]).powi(2))
            .sum::<f64>()
    };
    let mut taken: Vec<usize> = Vec::new();
    if n > 0 {
        taken.push(0);
    }
    while taken.len() < n {
        let nearest = |i: usize| {
            taken
                .iter()
                .map(|&t| d2(&pool[i], &pool[t]))
                .fold(f64::INFINITY, f64::min)
        };
        let Some(next) = (0..pool.len())
            .filter(|i| !taken.contains(i))
            .max_by(|&a, &b| nearest(a).total_cmp(&nearest(b)))
        else {
            break;
        };
        taken.push(next);
    }
    taken.into_iter().map(|i| pool[i].clone()).collect()
}

impl Pass for PlaceSkatebirdMarkers {
    fn name(&self) -> &'static str {
        "place-skatebird-markers"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "fallback".to_owned(),
                match self.fallback {
                    None => "none".to_owned(),
                    Some(([x, y, z], yaw)) => format!("{x},{y},{z},{yaw}"),
                },
            ),
            ("drop".to_owned(), self.drop.to_string()),
            ("margin".to_owned(), self.margin.to_string()),
            ("travel".to_owned(), self.travel.to_string()),
            ("play-max-extent".to_owned(), self.play_max_extent.to_string()),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME, Transform::NAME, Name::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        self.check()?;
        let mut start = None;
        let mut resets = Vec::new();
        for (e, r) in w.iter::<SkatebirdResetPosition>() {
            let Some(t) = w.get::<Transform>(e) else {
                log.count("reset positions without a transform, not used", 1);
                continue;
            };
            let name = w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone());
            let p = point(name, t);
            if r.starting_position {
                if start.is_some() {
                    return Err("more than one starting reset position".into());
                }
                start = Some(p.clone());
            }
            resets.push((r.starting_position, p));
        }
        let spawn = match start {
            Some(p) => p,
            None => {
                let ([x, y, z], yaw) = self
                    .fallback
                    .ok_or("no starting reset position and fallback=none")?;
                log.count("spawns at the stated fallback (no starting reset position)", 1);
                let y = match primase_meshops::collision_query::ground_below(w, [x, y, z], self.drop) {
                    Some(g) => {
                        log.count("fallback spawns dropped to the collision below", 1);
                        g
                    }
                    None => {
                        log.count("fallback spawns with no collision below, kept at their height", 1);
                        y
                    }
                };
                Point {
                    name: "spawn".into(),
                    position: [x, y, z],
                    yaw,
                }
            }
        };
        // the travel points: inside the play space, the starting one first, then spread
        let space = primase_meshops::collision_query::collision_box(w, self.play_max_extent)
            .ok_or("the level has no collision to measure its play space by")?;
        let m = self.margin;
        let inside =
            |p: &Point| (0..3).all(|i| space[0][i] - m <= p.position[i] && p.position[i] <= space[1][i] + m);
        let outside = resets.iter().filter(|(_, p)| !inside(p)).count();
        log.count(
            "reset positions outside the play space, not travel points",
            outside as u64,
        );
        let (starters, rest): (Vec<_>, Vec<_>) = resets
            .into_iter()
            .filter(|(_, p)| inside(p))
            .partition(|(s, _)| *s);
        let mut travel: Vec<Point> = starters.into_iter().take(1).map(|(_, p)| p).collect();
        let left = self.travel.saturating_sub(travel.len());
        travel.extend(spread(rest.into_iter().map(|(_, p)| p).collect(), left));
        travel.truncate(self.travel);
        for (kind, p) in std::iter::once(("spawn", &spawn)).chain(travel.iter().map(|p| ("travel", p))) {
            let e = w.spawn();
            w.insert(
                e,
                Marker {
                    kind: kind.into(),
                    ground: None,
                    yaw: None,
                },
            );
            w.insert(e, transform(p));
            w.insert(e, Name(p.name.clone()));
            log.place(e);
            log.count(&format!("{kind} markers placed"), 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Collision;
    use primase_unity::components::UnityObject;

    /// A ResetPosition's transform as the reader gives it for a Unity yaw `a` (degrees) at `at` (core frame): its
    /// forward, Unity's +Z, is the -Z column.
    fn reset(w: &mut World, name: &str, unity_yaw: f64, at: [f32; 3], starting: bool) {
        let a = unity_yaw.to_radians();
        let e = w.spawn();
        w.insert(
            e,
            Transform {
                cols: [
                    [(-a.cos()) as f32, 0.0, (-a.sin()) as f32],
                    [0.0, 1.0, 0.0],
                    [(-a.sin()) as f32, 0.0, a.cos() as f32],
                    at,
                ],
            },
        );
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            SkatebirdResetPosition {
                starting_position: starting,
                behaviour: UnityObject {
                    file: "level2".into(),
                    path_id: 1,
                },
                path: name.into(),
            },
        );
    }

    fn floor(w: &mut World) {
        let e = w.spawn();
        w.insert(
            e,
            Collision {
                positions: vec![[-100.0, 0.0, -100.0], [100.0, 0.0, -100.0], [0.0, 0.0, 100.0]],
                triangles: vec![[0, 2, 1]],
                surfaces: vec![0],
            },
        );
    }

    /// The scene yaw raise-skate gives a marker placed by `t`: its local +Z, `atan2(x, z)`.
    fn raised_yaw(t: &Transform) -> f64 {
        f64::from(t.cols[2][0])
            .atan2(f64::from(t.cols[2][2]))
            .to_degrees()
    }

    fn pass(fallback: Option<([f64; 3], f64)>) -> PlaceSkatebirdMarkers {
        PlaceSkatebirdMarkers {
            fallback,
            drop: 0.05,
            margin: 10.0,
            travel: 5,
            // the fixtures' floor is 200 m across
            play_max_extent: 500.0,
        }
    }

    /// The game's own yaws, checked against its scan: level2's start Unity 172.9341 is scene 7.0659, level1's Unity
    /// -54.8157 is scene 234.8157 (as -125.1843); the spawn is the starting reset position, travel the starting one
    /// first.
    #[test]
    fn markers_face_the_games_own_yaw() {
        for (unity, scene) in [(172.9341, 7.0659), (-54.8157, 234.8157)] {
            let mut w = World::new();
            floor(&mut w);
            reset(&mut w, "start", unity, [-18.7, 1.189, -84.8], true);
            reset(&mut w, "other", 0.0, [10.0, 1.0, 10.0], false);
            pass(None).run(&mut w, &mut PassLog::default()).unwrap();
            let spawns: Vec<_> = (w.iter::<Marker>())
                .filter(|(_, m)| m.kind == "spawn")
                .map(|(e, _)| e)
                .collect();
            assert_eq!(spawns.len(), 1);
            let t = w.get::<Transform>(spawns[0]).unwrap();
            let yaw = raised_yaw(t).rem_euclid(360.0);
            assert!(
                (yaw - scene).abs() < 1e-3,
                "Unity {unity}: scene {yaw}, the game's {scene}"
            );
            assert_eq!(t.cols[3], [-18.7, 1.189, -84.8]);
            let travel: Vec<_> = (w.iter::<Marker>())
                .filter(|(_, m)| m.kind == "travel")
                .map(|(e, _)| w.get::<Name>(e).unwrap().0.clone())
                .collect();
            assert_eq!(travel, ["start", "other"]);
        }
    }

    /// No starting reset position: the stated fallback, dropped to the floor below and counted; `none` refuses.
    #[test]
    fn a_level_without_a_start_takes_the_stated_fallback_dropped() {
        let mut w = World::new();
        floor(&mut w);
        reset(&mut w, "a", 0.0, [5.0, 1.0, 5.0], false);
        let mut log = PassLog::default();
        pass(Some(([0.0, 1.189, 0.0], 7.0659)))
            .run(&mut w, &mut log)
            .unwrap();
        let (e, _) = w.iter::<Marker>().find(|(_, m)| m.kind == "spawn").unwrap();
        assert_eq!(w.get::<Transform>(e).unwrap().cols[3], [0.0, 0.0, 0.0]);
        assert_eq!(
            log.counts.get("fallback spawns dropped to the collision below"),
            Some(&1)
        );
        let mut w = World::new();
        floor(&mut w);
        reset(&mut w, "a", 0.0, [5.0, 1.0, 5.0], false);
        assert!(pass(None).run(&mut w, &mut PassLog::default()).is_err());
    }

    /// The options read back; a missing one is refused.
    #[test]
    fn every_option_is_stated() {
        let o = pass(Some(([0.0, 1.189, 0.0], 7.0659))).options();
        assert_eq!(PLACE_SKATEBIRD_MARKERS.instance(&o).unwrap().options(), o);
        let mut missing = o.clone();
        missing.remove("travel");
        assert!(PLACE_SKATEBIRD_MARKERS.instance(&missing).is_err());
    }
}
