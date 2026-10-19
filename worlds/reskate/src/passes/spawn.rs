//! `place-reskate-spawn{at=centre | east=E,north=N}`: a spawn for a skate scene that has none.
//!
//! The map writer makes no spawn of its own (a map without one is refused). A program converting a core world places
//! its spawn at selection (`place-spawn`), before raising; a scene read as skate data (interchange v1) that states
//! none gets one from this pass: a `skate.marker` of kind spawn at the centre of the scene's region (its ground
//! frame box; else the box of its placements' ground positions) or at a given ground position, snapped to the street
//! (the nearest drivable ground the writer's collision finds), facing 0 degrees, a trainer spot. A scene that already
//! has a spawn keeps it and gets none (counted).
//!
//! A map converted in parts places its spawn once: `summarize-reskate-spawn` records what each part holds of it (its
//! spawns, its placements' box, its smallest root), the summaries merge into the whole map's, and this pass places the
//! spawn, at the centre the merged box gives, only in the part whose roots hold the map's smallest id; none when any
//! part had a spawn. A part that is not the whole and has no summary is refused. The whole map goes through the same
//! summary.

use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Instance, Transform, World};

use crate::components::{MarkerKind, SkateMarker, SkateScene, SkateSpawnSummary, Snap, SnapMode};

/// What `w` holds of the map's spawn, of the entities it owns (see [`SkateSpawnSummary`]).
fn summarize(w: &World) -> SkateSpawnSummary {
    let owned = super::Owned::of(w);
    let spawns = w
        .iter::<SkateMarker>()
        .filter(|(_, m)| m.kind == MarkerKind::Spawn)
        .filter_map(|(e, _)| owned.key(e))
        .collect();
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    // a placement at scene (x, y, z) is on the ground at (east, north) = (x, -z)
    for (e, _) in w.iter::<Instance>().filter(|(e, _)| owned.contains(*e)) {
        if let Some(t) = w.get::<Transform>(e) {
            let g = [t.cols[3][0], -t.cols[3][2]];
            lo = [lo[0].min(g[0]), lo[1].min(g[1])];
            hi = [hi[0].max(g[0]), hi[1].max(g[1])];
        }
    }
    let first = match w.resource::<PartScope>().filter(|s| !s.whole) {
        Some(s) => s.roots.first().map(|e| e.index()),
        None => w.entities().next().map(|e| e.index()),
    };
    SkateSpawnSummary {
        spawns,
        lo,
        hi,
        first,
    }
}

/// Where [`PlaceReskateSpawn`] puts the spawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReskateSpawnAt {
    /// The centre of the scene's region, else of the box of its placements' ground positions.
    Centre,
    /// This ground position `(east, north)`, metres.
    Ground([f32; 2]),
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceReskateSpawn {
    at: ReskateSpawnAt,
}

impl PlaceReskateSpawn {
    /// A spawn at the centre of the scene's region (else of its placements).
    pub fn centre() -> PlaceReskateSpawn {
        PlaceReskateSpawn {
            at: ReskateSpawnAt::Centre,
        }
    }

    /// A spawn at this ground position; finite metres.
    pub fn at_ground(east: f32, north: f32) -> Result<PlaceReskateSpawn, String> {
        if !(east.is_finite() && north.is_finite()) {
            return Err(format!("({east}, {north}) is not a finite ground position"));
        }
        Ok(PlaceReskateSpawn {
            at: ReskateSpawnAt::Ground([east, north]),
        })
    }

    pub fn at(&self) -> ReskateSpawnAt {
        self.at
    }

    /// From its options (`at=centre`, or `east` and `north`), through the same constructors.
    pub fn from_options(o: &Options) -> Result<PlaceReskateSpawn, String> {
        let number = |k: &str, v: &str| v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number"));
        let (mut centre, mut east, mut north) = (false, None, None);
        for (k, v) in o {
            match k.as_str() {
                "at" if v == "centre" => centre = true,
                "at" => return Err(format!("at={v}: only at=centre (or east, north)")),
                "east" => east = Some(number(k, v)?),
                "north" => north = Some(number(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        match (centre, east, north) {
            (false, None, None) => Err("the spawn's place is required: at=centre, or east and north".into()),
            (true, None, None) => Ok(PlaceReskateSpawn::centre()),
            (false, Some(e), Some(n)) => PlaceReskateSpawn::at_ground(e, n),
            (true, _, _) => Err("at=centre or east, north: not both".into()),
            _ => Err("east and north go together".into()),
        }
    }
}

/// [`PlaceReskateSpawn::from_options`] as a pass factory.
pub fn place_reskate_spawn(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PlaceReskateSpawn::from_options(o)?))
}

impl Pass for PlaceReskateSpawn {
    fn name(&self) -> &'static str {
        "place-reskate-spawn"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        match self.at {
            ReskateSpawnAt::Centre => {
                o.insert("at".into(), "centre".into());
            }
            ReskateSpawnAt::Ground([e, n]) => {
                o.insert("east".into(), e.to_string());
                o.insert("north".into(), n.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.marker"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // what the whole map holds of its spawn (merged over every part); else, on the whole map, measured here
        let summary = match w.resource::<SkateSpawnSummary>() {
            Some(s) => s.clone(),
            None => {
                super::require_whole_map(w, self.name()).map_err(|e| {
                    format!(
                        "{e}: a map in parts places its spawn once, from what every part holds \
                         (summarize-reskate-spawn)"
                    )
                })?;
                summarize(w)
            }
        };
        if !summary.spawns.is_empty() {
            log.count("scenes with a spawn of their own: none placed", 1);
            return Ok(());
        }
        // the part that places it: the one holding the map's smallest root (the whole map holds all)
        let mine = match w.resource::<PartScope>().filter(|s| !s.whole) {
            Some(s) => summary
                .first
                .is_some_and(|f| s.roots.iter().any(|e| e.index() == f)),
            None => true,
        };
        if !mine {
            return Ok(());
        }
        let [east, north] = match self.at {
            ReskateSpawnAt::Ground(g) => g,
            ReskateSpawnAt::Centre => {
                let region = w.resource::<SkateScene>().and_then(|s| s.region);
                let [x0, y0, x1, y1] =
                    region.unwrap_or([summary.lo[0], summary.lo[1], summary.hi[0], summary.hi[1]]);
                [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
            }
        };
        if !(east.is_finite() && north.is_finite()) {
            return Err(
                "place-reskate-spawn: the scene has no region and no placement to centre a spawn on".into(),
            );
        }
        let spawn = w.spawn();
        w.insert(
            spawn,
            SkateMarker {
                kind: MarkerKind::Spawn,
                // scene space (x, z): the ground position's (east, -north); the height is the snap's
                position: Some(([east, -north], None)),
                yaw: Some(0.0),
                m: None,
                blueprint: None,
                snap: Some(Snap::Mode(SnapMode::Street)),
                lift: None,
                shelter: None,
                trainer: Some(true),
                other: Default::default(),
            },
        );
        log.place(spawn);
        log.count("spawns placed", 1);
        Ok(())
    }
}

/// `summarize-reskate-spawn`: what this part holds of the map's spawn (`skate.spawn_summary`); merged over the parts,
/// `place-reskate-spawn` places the spawn once from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeReskateSpawn;

/// [`SummarizeReskateSpawn`] as a pass factory: it takes no options.
pub fn summarize_reskate_spawn(o: &Options) -> Result<Box<dyn Pass>, String> {
    match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(SummarizeReskateSpawn)),
    }
}

impl Pass for SummarizeReskateSpawn {
    fn name(&self) -> &'static str {
        "summarize-reskate-spawn"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.spawn_summary"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
        let s = summarize(w);
        w.insert_resource(s);
        Ok(())
    }
}
