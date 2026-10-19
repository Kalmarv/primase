//! `--from skate-scene`: a skate-scene interchange folder ([`primase_reskate::SceneReader`]); this module parses its
//! options.
use std::path::PathBuf;

use primase_core::convert::Reader;

use crate::convert::Result;

/// `--scene <folder>`: the folder holding `scene.json`.
pub fn reader(args: &[String]) -> Result<Box<dyn Reader>> {
    let mut dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--scene" => {
                dir = Some(PathBuf::from(
                    it.next().ok_or("skate-scene: --scene needs a folder")?,
                ))
            }
            _ => return Err(format!("skate-scene: unknown option {a}").into()),
        }
    }
    let dir = dir.ok_or("skate-scene: needs --scene <folder>")?;
    Ok(Box::new(primase_reskate::SceneReader::new(dir)))
}
