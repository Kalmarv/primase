//! The GTA reader of [`primase_core::convert`]: a region of an install, read once to plan from and loaded part by
//! part ([`crate::pipeline::GtaSource`]). Which selection and passes a conversion runs is its caller's choice: the
//! reader reads every entity of both maps within reach, with their state as conditions, and chooses none of it.
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::convert::{Reader, Report, Result};
use primase_core::{Entity, World};

use crate::pipeline::{GtaSource, Map, prototype_box};

/// What to read: the install and its key folder, the region, how far beyond it entities are read, and which map's
/// archive set drawables and textures are loaded from (both maps' entities are read either way, with `gta.map`
/// conditions).
#[derive(Clone, Debug)]
pub struct GtaOptions {
    pub install: PathBuf,
    pub keys: PathBuf,
    /// `[x0, y0, x1, y1]`, GTA ground coordinates.
    pub region: [f32; 4],
    /// Metres outside the region entities are read, so large pieces of the world that reach in are seen.
    pub reach: f32,
    pub map: Map,
}

/// The GTA reader (see the module documentation).
pub struct GtaReader {
    source: GtaSource,
    region: [f32; 4],
    reach: f32,
}

impl std::fmt::Debug for GtaReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaReader")
            .field("region", &self.region)
            .field("reach", &self.reach)
            .finish_non_exhaustive()
    }
}

impl GtaReader {
    /// Opens the install, its assets loaded from `o.map`'s archive set.
    pub fn open(o: &GtaOptions) -> Result<GtaReader> {
        Ok(GtaReader {
            source: GtaSource::open(&o.install, &o.keys, o.map)?,
            region: o.region,
            reach: o.reach,
        })
    }
}

impl Reader for GtaReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let (w, r) = self.source.read(self.region, self.reach)?;
        Ok((
            w,
            vec![
                ("gta region".into(), format!("{:?}", r.region)),
                ("gta lights".into(), format!("{:?}", r.lights)),
                ("gta bounds".into(), format!("{:?}", r.bounds)),
                (
                    "gta water (each file: water, calming, wave quads)".into(),
                    format!("{:?}", r.water),
                ),
            ],
        ))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        let (w, r) = self.source.load(world, roots)?;
        let mut report = Report::new();
        if !r.textures_not_found.is_empty() || r.asset_missing > 0 {
            report.push((
                "gta assets".into(),
                format!(
                    "{} prototypes without a live file, textures not found {:?}",
                    r.asset_missing, r.textures_not_found
                ),
            ));
        }
        Ok((w, report))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "gta"]
    }
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        prototype_box(world, prototype)
    }
}

/// The map a `--map` value names.
pub fn map_named(name: &str) -> Option<Map> {
    match name {
        "online" => Some(Map::Online),
        "story" => Some(Map::Story),
        _ => None,
    }
}
