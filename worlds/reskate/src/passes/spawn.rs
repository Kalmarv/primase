//! `place-reskate-spawn{at=centre | east=E,north=N}`: a spawn for a skate scene that has none.
//!
//! The map writer makes no spawn of its own (a map without one is refused). A program converting a core world places
//! its spawn at selection (`place-spawn`), before raising; a scene read as skate data (interchange v1) that states
//! none gets one from this pass: a `skate.marker` of kind spawn at the centre of the scene's region (its ground
//! frame box; else the box of its placements' ground positions) or at a given ground position, snapped to the street
//! (the nearest drivable ground the writer's collision finds), facing 0 degrees, a trainer spot. A scene that already
//! has a spawn keeps it and gets none (counted).

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Instance, Transform, World};

use crate::components::{MarkerKind, SkateMarker, SkateScene, Snap, SnapMode};

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
            (_, None, None) => Ok(PlaceReskateSpawn::centre()),
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
        super::require_one_part(w, self.name())?;
        if w.iter::<SkateMarker>().any(|(_, m)| m.kind == MarkerKind::Spawn) {
            log.count("scenes with a spawn of their own: none placed", 1);
            return Ok(());
        }
        let [east, north] = match self.at {
            ReskateSpawnAt::Ground(g) => g,
            ReskateSpawnAt::Centre => {
                let region = w.resource::<SkateScene>().and_then(|s| s.region);
                // a placement at scene (x, y, z) is on the ground at (east, north) = (x, -z)
                let [x0, y0, x1, y1] = region.unwrap_or_else(|| {
                    let ground: Vec<[f32; 2]> = w
                        .iter::<Instance>()
                        .filter_map(|(e, _)| w.get::<Transform>(e))
                        .map(|t| [t.cols[3][0], -t.cols[3][2]])
                        .collect();
                    let lo = |k: usize| ground.iter().map(|g| g[k]).fold(f32::INFINITY, f32::min);
                    let hi = |k: usize| ground.iter().map(|g| g[k]).fold(f32::NEG_INFINITY, f32::max);
                    [lo(0), lo(1), hi(0), hi(1)]
                });
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
