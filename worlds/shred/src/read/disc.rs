//! A level read from a disc folder: the files `rme_formats::disc` locates, opened for the program's choice of base
//! level and layers, and the listings a program chooses them from.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the disc files of the level the caller names"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::frame::SourceFrame;
use rme_formats::database::Database;
use rme_formats::disc::Disc;
use rme_formats::manifest::{self, LayerPak, Manifest};
use rme_formats::{pak::Pak, surfaces::SurfaceTable};

use super::collision::{CollisionReport, read_collision};
use super::convert::ShredReader;
use super::material_join::{MaterialInputs, TexturePreference};
use crate::{Result, invalid};

fn database(bytes: &[u8]) -> Result<Database<'_>> {
    Database::open(bytes).map_err(|e| invalid(0, format!("database: {e}")))
}

/// The base levels on the disc in `folder` (the folder holding `PS3_GAME`), by lower-case name, with their archives'
/// paths: `MAIN.DB`'s level folders whose base archive is there ([`Disc::base_levels`]).
pub fn base_levels(folder: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let disc = Disc::new(folder);
    let bytes = std::fs::read(disc.database())?;
    let rows = manifest::levels(&database(&bytes)?)?;
    Ok(disc.base_levels(&rows)?)
}

/// One layer archive of the disc as it serves one base level: an archive holding quartets of several levels is listed
/// once for each.
#[derive(Clone, Debug)]
pub struct LayerListing {
    /// The base level, lower case.
    pub level: String,
    /// The archive's file name, lower case.
    pub file: String,
    pub path: PathBuf,
    pub layer: LayerPak,
}

/// Every layer archive of the disc in `folder` for each base level it holds quartets of, by archive name then level.
/// Each archive is opened in turn for its quartet names; none is kept.
pub fn layer_listings(folder: &Path) -> Result<Vec<LayerListing>> {
    let disc = Disc::new(folder);
    let paths = disc.layer_archives()?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(disc.database())?;
    let db = database(&bytes)?;
    let rows = manifest::levels(&db)?;
    let levels = disc.base_levels(&rows)?;
    let mut out = Vec::new();
    for (file, path) in paths {
        let pak = Pak::open(&path)?;
        let manifest = Manifest::new(&db, &[(&file, &pak)])?;
        for layer in &manifest.layers {
            for level in levels.keys() {
                if manifest.layers_naming(level).is_empty() {
                    continue;
                }
                out.push(LayerListing {
                    level: level.clone(),
                    file: file.clone(),
                    path: path.clone(),
                    layer: layer.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// A base level and the layers the program chose, with every file of the disc reading it needs, opened.
#[derive(Debug)]
pub struct DiscLevel {
    pub level: String,
    /// `levels/<level>.pak`, then `levels/layers/<file>` of each layer in the order given, with its archive.
    pub archives: Vec<(String, Pak)>,
    /// `MAIN.DB`'s collision surfaces.
    pub surfaces: SurfaceTable,
    /// `MATERIALS.PAK`.
    pub banks: Pak,
    /// The disc's shared archives ([`rme_formats::disc::SHARED_ARCHIVES`]).
    pub shared: Vec<(&'static str, Pak)>,
}

impl DiscLevel {
    /// Open base level `level` (lower case, as [`base_levels`] names it) of the disc in `folder` with the layer archives
    /// `layers` (lower-case file names, as [`layer_listings`] names them). Refused: a level or layer not on the disc,
    /// and a layer set that does not consist of archives holding this level's quartets.
    pub fn open(folder: &Path, level: &str, layers: &[String]) -> Result<Self> {
        let disc = Disc::new(folder);
        let bytes = std::fs::read(disc.database())?;
        let db = database(&bytes)?;
        let rows = manifest::levels(&db)?;
        let path = disc
            .base_levels(&rows)?
            .remove(level)
            .ok_or_else(|| invalid(0, format!("shred: base level {level} not found on disc")))?;
        let mut archives = vec![(format!("levels/{level}.pak"), open(&path)?)];
        if !layers.is_empty() {
            let mut available = disc.layer_archives()?;
            for name in layers {
                let path = available
                    .remove(name)
                    .ok_or_else(|| invalid(0, format!("shred: layer {name} not found")))?;
                archives.push((format!("levels/layers/{name}"), open(&path)?));
            }
        }
        let surfaces = SurfaceTable::from_main_db(&bytes)?;
        let files: Vec<_> = archives
            .iter()
            .map(|(label, pak)| (label.rsplit('/').next().unwrap_or(label), pak))
            .collect();
        let manifest = Manifest::new(&db, &files)?;
        if manifest.layers_naming(level).len() != layers.len() {
            return Err(invalid(
                0,
                "shred: selected layer does not contain this level's quartets",
            ));
        }
        let banks = open(&disc.material_banks())?;
        let shared = disc
            .shared_archives()
            .into_iter()
            .map(|(name, path)| open(&path).map(|pak| (name, pak)))
            .collect::<Result<_>>()?;
        Ok(DiscLevel {
            level: level.into(),
            archives,
            surfaces,
            banks,
            shared,
        })
    }

    /// The archives as the readers take them: `(label, archive)`.
    pub fn paks(&self) -> Vec<(&str, &Pak)> {
        self.archives
            .iter()
            .map(|(label, pak)| (label.as_str(), pak))
            .collect()
    }

    /// The level's reader in the program's `frame`, with the material banks and shared archives and the program's
    /// texture copy preference.
    pub fn reader(&self, frame: SourceFrame, textures: TexturePreference) -> ShredReader<'_> {
        ShredReader::new(self.paks(), frame).with_materials(MaterialInputs {
            banks: &self.banks,
            assets: self.shared.iter().map(|(name, pak)| (*name, pak)).collect(),
            textures,
        })
    }

    /// The level's collision read into `world` ([`read_collision`] of its archives).
    pub fn read_collision(&self, world: &mut World, frame: SourceFrame) -> Result<CollisionReport> {
        read_collision(world, &self.paks(), &self.surfaces, frame)
    }
}

fn open(path: &Path) -> Result<Pak> {
    Pak::open(path).map_err(|e| invalid(0, format!("opening {}: {e}", path.display())))
}
