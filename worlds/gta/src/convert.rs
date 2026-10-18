//! The GTA reader of [`primase_core::convert`]: a region of an install, read once to plan from and loaded part by
//! part ([`crate::pipeline::GtaSource`]). Which selection and passes a conversion runs is its caller's choice
//! ([`crate::pipeline`] documents one recipe).
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::convert::{Reader, Report, Result};
use primase_core::{Entity, World};

use crate::pipeline::{GtaSource, Map, Selection, prototype_box};

/// What to read: the install and its key folder, and the selection (region, map, hour, sizes, interiors).
#[derive(Clone, Debug)]
pub struct GtaOptions {
    pub install: PathBuf,
    pub keys: PathBuf,
    pub selection: Selection,
}

/// The GTA reader (see the module documentation).
pub struct GtaReader {
    source: GtaSource,
    selection: Selection,
}

impl std::fmt::Debug for GtaReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaReader")
            .field("selection", &self.selection)
            .finish()
    }
}

impl GtaReader {
    /// Opens the install for the selection's map.
    pub fn open(o: &GtaOptions) -> Result<GtaReader> {
        Ok(GtaReader {
            source: GtaSource::open(&o.install, &o.keys, o.selection.map)?,
            selection: o.selection.clone(),
        })
    }
}

impl Reader for GtaReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let (w, r) = self.source.read(self.selection.region, self.selection.reach)?;
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
