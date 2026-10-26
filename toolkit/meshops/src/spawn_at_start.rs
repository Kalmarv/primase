//! `spawn-at-start{from=KIND,prefer=A|B|...}[,avoid=A|B][,place=NAME,ahead=M][,nodes=KIND]`: the skater's spawn at one
//! of the level's own start positions, for a source that states its starts as markers (of kind `from`) but no spawn.
//!
//! The start is, in order:
//! - the first marker of kind `from` whose name contains a preferred text, tried in the order given; else the first
//!   one whose name contains none of the `avoid` texts (a start that is not the player's: a vehicle's, an NPC's);
//! - with no such marker, the placement named `place`, on the collision under the point `ahead` metres along its local
//!   +Z from its origin (a level that states no start of its own);
//! - with none, the first marker of kind `nodes`, where it stands;
//! - else the placements' centre, on the collision under it.
//!
//! Each is placed as `drop-marker` places a point (it checks the collision under it): a marker or node where it
//! stands (collision within 2 m under it), a placement on the collision under it (its origin can float over the floor).
//! The markers read stay as read.
//!
//! Facing: a marker's or placement's local +Z, turned about +Y only (the source keeps a start's whole matrix; which
//! local axis it calls forward is the program's knowledge, stated by choosing what to place); a node or the centre is
//! unturned.

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Marker, Name, Transform, World, kind_is_valid};

use crate::drop_marker::{DropMarker, MarkerAt};

/// See the module documentation. Made by [`SpawnAtStart::new`], refined by [`SpawnAtStart::avoiding`],
/// [`SpawnAtStart::placing`] and [`SpawnAtStart::with_nodes`]; from options by the same calls.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnAtStart {
    from: String,
    prefer: Vec<String>,
    avoid: Vec<String>,
    place: Option<(String, f32)>,
    nodes: Option<String>,
}

impl SpawnAtStart {
    /// The start among the markers of kind `from` (a word of `a-z`, `0-9` and `_`) whose name contains one of
    /// `prefer`, in that order (texts of at least one character).
    pub fn new(from: &str, prefer: &[&str]) -> Result<SpawnAtStart, String> {
        if !kind_is_valid(from) {
            return Err(format!("from={from:?} is not a marker kind"));
        }
        if prefer.iter().any(|p| p.is_empty()) {
            return Err("prefer: an empty text matches every name".into());
        }
        Ok(SpawnAtStart {
            from: from.into(),
            prefer: prefer.iter().map(|p| (*p).into()).collect(),
            avoid: Vec::new(),
            place: None,
            nodes: None,
        })
    }

    /// Without a preferred start, take the first one whose name contains none of `avoid` (texts of at least one
    /// character).
    pub fn avoiding(mut self, avoid: &[&str]) -> Result<SpawnAtStart, String> {
        if avoid.iter().any(|p| p.is_empty()) {
            return Err("avoid: an empty text matches every name".into());
        }
        self.avoid = avoid.iter().map(|p| (*p).into()).collect();
        Ok(self)
    }

    /// With no start, the placement named `name`, `ahead` (finite) metres along its local +Z from its origin.
    pub fn placing(mut self, name: &str, ahead: f32) -> Result<SpawnAtStart, String> {
        if !ahead.is_finite() {
            return Err(format!("ahead={ahead} is not finite metres"));
        }
        self.place = Some((name.into(), ahead));
        Ok(self)
    }

    /// With no start or placement, the first marker of kind `kind`.
    pub fn with_nodes(mut self, kind: &str) -> Result<SpawnAtStart, String> {
        if !kind_is_valid(kind) {
            return Err(format!("nodes={kind:?} is not a marker kind"));
        }
        self.nodes = Some(kind.into());
        Ok(self)
    }

    /// From its options, through the same constructors.
    fn from_given<'a>(g: &Given<'a>) -> Result<SpawnAtStart, String> {
        let list = |text: &'a str| text.split('|').collect::<Vec<_>>();
        let mut pass = SpawnAtStart::new(g.required("from"), &list(g.required("prefer")))?;
        if let Some(a) = g.optional("avoid") {
            pass = pass.avoiding(&list(a))?;
        }
        let ahead = g
            .optional("ahead")
            .map(|v| v.parse::<f32>().map_err(|_| format!("ahead={v} is not a number")))
            .transpose()?;
        match (g.optional("place"), ahead) {
            (Some(name), ahead) => pass = pass.placing(name, ahead.unwrap_or(0.0))?,
            (None, Some(_)) => return Err("ahead goes with place".into()),
            (None, None) => {}
        }
        match g.optional("nodes") {
            Some(kind) => pass.with_nodes(kind),
            None => Ok(pass),
        }
    }
}

/// `spawn-at-start`'s declaration ([`primase_core::pass::PassRegistry::add`]): which markers are starts and which to
/// prefer are the program's, no default.
pub const SPAWN_AT_START: PassSpec = PassSpec {
    name: "spawn-at-start",
    options: &[
        OptionSpec::required(
            "from",
            "start",
            "the kind of the markers that are the level's starts",
        ),
        OptionSpec::required(
            "prefer",
            "Start|Spawn",
            "texts joined by |: the first start whose name contains the first of them, else the second, ...",
        ),
        OptionSpec::optional(
            "avoid",
            "Train|Npc",
            "texts joined by |: without a preferred start, the first start whose name contains none of them",
            "the first start, whatever its name",
        ),
        OptionSpec::optional(
            "place",
            "Station",
            "a placement's name: with no start, stand at its origin (turned to its local +Z)",
            "no placement to stand at",
        ),
        OptionSpec::optional(
            "ahead",
            "9",
            "metres, with place: along its local +Z from its origin",
            "0: at its origin",
        ),
        OptionSpec::optional(
            "nodes",
            "spawn_point",
            "a marker kind: with no start or placement, stand at the first marker of it",
            "the placements' centre",
        ),
    ],
    make: |g| Ok(Box::new(SpawnAtStart::from_given(g)?)),
};

impl Pass for SpawnAtStart {
    fn name(&self) -> &'static str {
        "spawn-at-start"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("from".to_owned(), self.from.clone()),
            ("prefer".to_owned(), self.prefer.join("|")),
        ]);
        if !self.avoid.is_empty() {
            o.insert("avoid".into(), self.avoid.join("|"));
        }
        if let Some((name, ahead)) = &self.place {
            o.insert("place".into(), name.clone());
            o.insert("ahead".into(), ahead.to_string());
        }
        if let Some(kind) = &self.nodes {
            o.insert("nodes".into(), kind.clone());
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &["core.marker", "core.name", "core.transform"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let starts: Vec<(String, [f32; 3], f32)> = w
            .iter::<Marker>()
            .filter(|(_, m)| m.kind == self.from)
            .filter_map(|(e, _)| {
                let name = w.get::<Name>(e)?.0.clone();
                let t = w.get::<Transform>(e)?;
                Some((name, t.cols[3], heading(t)))
            })
            .collect();
        let chosen = self
            .prefer
            .iter()
            .find_map(|p| starts.iter().find(|(n, ..)| n.contains(p.as_str())))
            .or_else(|| {
                starts
                    .iter()
                    .find(|(n, ..)| !self.avoid.iter().any(|a| n.contains(a.as_str())))
            });
        let drop = |name: &str, at, yaw: Option<f32>| {
            let pass = DropMarker::new("spawn", name, at)?;
            match yaw {
                Some(y) => pass.with_yaw(y),
                None => Ok(pass),
            }
        };
        if let Some((name, at, yaw)) = chosen {
            log.count(
                &format!("spawn at the start {name:?}, facing {yaw:.0} degrees"),
                1,
            );
            return drop(name, MarkerAt::Stated(*at), Some(*yaw))?.run(w, log);
        }
        // no start: the named placement, facing its local +Z as a start does, on the collision under its origin
        if let Some((place, ahead)) = &self.place {
            let found = w
                .iter::<Name>()
                .filter(|(_, n)| n.0 == *place)
                .find_map(|(e, _)| {
                    let t = w.get::<Transform>(e)?;
                    let [x, _, z] = t.cols[2];
                    let [ox, oy, oz] = t.cols[3];
                    Some(([ox + x * ahead, oy, oz + z * ahead], heading(t)))
                });
            let (at, yaw) = found.ok_or(format!("no placement named {place:?} to start at"))?;
            log.count(
                &format!(
                    "spawn at the placement {place:?}, {ahead} m ahead, facing {yaw:.0} degrees (no start position)"
                ),
                1,
            );
            return drop(place, MarkerAt::Under(at), Some(yaw))?.run(w, log);
        }
        // no start: the first node, else the placements' centre
        if let Some(kind) = &self.nodes {
            let node = w
                .iter::<Marker>()
                .filter(|(_, m)| m.kind == *kind)
                .find_map(|(e, _)| Some((w.get::<Name>(e)?.0.clone(), w.get::<Transform>(e)?.cols[3])));
            if let Some((name, at)) = node {
                log.count(&format!("spawn at the {kind} {name:?} (no start position)"), 1);
                return drop(&name, MarkerAt::Stated(at), None)?.run(w, log);
            }
        }
        log.count("spawn at the placements' centre (no start position or node)", 1);
        drop("centre", MarkerAt::Centre, None)?.run(w, log)
    }
}

/// A transform's local +Z turned about +Y only: degrees, 0 facing +Z and 90 facing +X.
fn heading(t: &Transform) -> f32 {
    let [x, _, z] = t.cols[2];
    x.atan2(z).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::{Collision, Instance};

    /// A world with one 100 m square of floor at height `h`.
    fn world(h: f32) -> World {
        let mut w = World::new();
        w.insert_resource(primase_core::Surfaces(vec![primase_core::Surface::new(
            "concrete",
        )]));
        let proto = w.spawn();
        w.insert(
            proto,
            Collision {
                positions: vec![
                    [-50.0, 0.0, -50.0],
                    [50.0, 0.0, -50.0],
                    [50.0, 0.0, 50.0],
                    [-50.0, 0.0, 50.0],
                ],
                triangles: vec![[0, 2, 1], [0, 3, 2]],
                surfaces: vec![0, 0],
            },
        );
        let floor = w.spawn();
        w.insert(
            floor,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [0.0, h, 0.0];
        w.insert(floor, t);
        w
    }

    /// An entity named `name` at `at` whose local +Z is +X (heading 90 degrees), a marker of `kind` when given.
    fn put(w: &mut World, name: &str, kind: Option<&str>, at: [f32; 3]) {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            Transform {
                cols: [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], at],
            },
        );
        if let Some(kind) = kind {
            w.insert(
                e,
                Marker {
                    kind: kind.into(),
                    ground: None,
                    yaw: None,
                },
            );
        }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5)
    }

    /// Runs `text` on `w`; the spawn it made: its name, position and heading.
    fn spawn(w: &mut World, text: &str) -> Result<(String, [f32; 3], f32), String> {
        let mut reg = PassRegistry::new();
        reg.add(&SPAWN_AT_START);
        let pipeline: Pipeline = reg.parse(text).map_err(|e| e.to_string())?;
        PassManager::new(reg)
            .run(w, &pipeline, &mut RunReport::default())
            .map_err(|e| e.to_string())?;
        let (e, _) = w.iter::<Marker>().find(|(_, m)| m.kind == "spawn").unwrap();
        let t = w.get::<Transform>(e).unwrap();
        Ok((w.get::<Name>(e).unwrap().0.clone(), t.cols[3], heading(t)))
    }

    #[test]
    fn the_first_preferred_start_is_taken_where_it_stands_facing_its_forward() {
        let mut w = world(0.0);
        put(&mut w, "A_Start", Some("start"), [1.0, 0.0, 1.0]);
        put(&mut w, "B_Start_Main", Some("start"), [3.0, 0.5, -2.0]);
        let (name, at, yaw) = spawn(&mut w, "spawn-at-start{from=start,prefer=Main|A_}").unwrap();
        assert_eq!(name, "B_Start_Main");
        assert!(near(at, [3.0, 0.5, -2.0]), "{at:?}");
        assert!((yaw - 90.0).abs() < 1e-4, "{yaw}");
    }

    /// Without a preferred start, the first that is not avoided; a preferred start is taken even when avoided.
    #[test]
    fn without_a_preferred_start_an_avoided_one_is_passed_over() {
        let mut w = world(0.0);
        put(&mut w, "Train_Start", Some("start"), [1.0, 0.0, 1.0]);
        put(&mut w, "Plaza_Start", Some("start"), [3.0, 0.0, -2.0]);
        let (name, ..) = spawn(&mut w, "spawn-at-start{from=start,prefer=Nope,avoid=Train}").unwrap();
        assert_eq!(name, "Plaza_Start");
        // a preferred start is taken even when avoided
        let mut w = world(0.0);
        put(&mut w, "Train_Start", Some("start"), [1.0, 0.0, 1.0]);
        let (name, ..) = spawn(&mut w, "spawn-at-start{from=start,prefer=Train,avoid=Train}").unwrap();
        assert_eq!(name, "Train_Start");
    }

    #[test]
    fn a_placement_is_stood_at_on_the_floor_under_its_origin_facing_its_forward() {
        let mut w = world(1.5);
        put(&mut w, "Station", None, [3.0, 4.0, -2.0]);
        let (_, at, yaw) = spawn(&mut w, "spawn-at-start{from=start,prefer=Start,place=Station}").unwrap();
        assert!(near(at, [3.0, 1.5, -2.0]), "{at:?}");
        assert!((yaw - 90.0).abs() < 1e-4, "{yaw}");
    }

    #[test]
    fn ahead_moves_the_start_along_the_placements_forward() {
        let mut w = world(1.5);
        put(&mut w, "Station", None, [3.0, 4.0, -2.0]);
        let (_, at, yaw) = spawn(
            &mut w,
            "spawn-at-start{from=start,prefer=Start,place=Station,ahead=7}",
        )
        .unwrap();
        assert!(near(at, [10.0, 1.5, -2.0]), "{at:?}");
        assert!((yaw - 90.0).abs() < 1e-4, "{yaw}");
    }

    /// With no start or placement: the first node where it stands, unturned; with no node, the placements' centre.
    #[test]
    fn a_node_then_the_centre_stand_when_there_is_no_start() {
        let mut w = world(1.5);
        put(&mut w, "Npc", Some("spawn_point"), [3.0, 1.5, -2.0]);
        let (name, at, yaw) = spawn(
            &mut w,
            "spawn-at-start{from=start,prefer=Start,nodes=spawn_point}",
        )
        .unwrap();
        assert_eq!(name, "Npc");
        assert!(near(at, [3.0, 1.5, -2.0]) && yaw.abs() < 1e-4, "{at:?} {yaw}");
        let mut w = world(1.5);
        let (name, at, _) = spawn(&mut w, "spawn-at-start{from=start,prefer=Start}").unwrap();
        assert_eq!(name, "centre");
        assert!(near(at, [0.0, 1.5, 0.0]), "{at:?}");
    }

    #[test]
    fn a_missing_placement_and_bad_options_are_refused() {
        let mut w = world(0.0);
        assert!(
            spawn(&mut w, "spawn-at-start{from=start,prefer=Start,place=Nowhere}")
                .unwrap_err()
                .contains("no placement named")
        );
        for bad in [
            "spawn-at-start{from=Start,prefer=Start}",
            "spawn-at-start{from=start,prefer=}",
            "spawn-at-start{from=start}",
            "spawn-at-start{prefer=Start}",
            "spawn-at-start{from=start,prefer=Start,ahead=3}",
            "spawn-at-start{from=start,prefer=Start,place=S,ahead=inf}",
            "spawn-at-start{from=start,prefer=Start,avoid=}",
            "spawn-at-start{from=start,prefer=Start,nodes=Node}",
        ] {
            assert!(spawn(&mut world(0.0), bad).is_err(), "{bad}");
        }
        // what the pass prints is what makes it again
        let pass = SpawnAtStart::new("start", &["A", "B"])
            .unwrap()
            .avoiding(&["T"])
            .unwrap()
            .placing("P", 2.5)
            .unwrap()
            .with_nodes("node")
            .unwrap();
        let mut reg = PassRegistry::new();
        reg.add(&SPAWN_AT_START);
        assert_eq!(
            reg.make("spawn-at-start", &pass.options()).unwrap().options(),
            pass.options()
        );
    }
}
