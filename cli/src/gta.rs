//! `--from gta`: a region of a GTA V install ([`primase_gta::pipeline`]).
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::{Entity, World};
use primase_gta::pipeline::{
    DEFAULT_PASSES, GtaSource, Map, REACH, REGIONS, Selection, named_region, prototype_box, select_passes,
};

use crate::convert::{Reader, Result};

struct Gta {
    source: GtaSource,
    selection: Selection,
}

fn region_names() -> String {
    REGIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// `--gta <install> --keys <key folder> --region <name>|x0,y0,x1,y1 [--map online|story] [--hour H] [--min-size M]
/// [--max-priority N] [--interiors none|all]`
pub fn reader(args: &[String]) -> Result<Box<dyn Reader>> {
    let mut interiors = false;
    let (mut dir, mut keys, mut region, mut map, mut hour, mut min_size, mut max_priority) =
        (None, None, None, Map::Online, None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("gta: {a} needs a value"));
        match a.as_str() {
            "--gta" => dir = Some(PathBuf::from(value()?)),
            "--keys" => keys = Some(PathBuf::from(value()?)),
            "--region" => {
                let v = value()?;
                let bad = || {
                    format!(
                        "gta: --region is a name ({names}) or x0,y0,x1,y1 in metres",
                        names = region_names()
                    )
                };
                let r = match named_region(v) {
                    Some(r) => r,
                    None => {
                        let n: Vec<f32> = v
                            .split(',')
                            .map(str::parse)
                            .collect::<std::result::Result<_, _>>()
                            .map_err(|_| bad())?;
                        let [x0, y0, x1, y1] = n[..] else {
                            return Err(bad().into());
                        };
                        [x0, y0, x1, y1]
                    }
                };
                if !(r[0] < r[2] && r[1] < r[3]) {
                    return Err("gta: --region needs x0 < x1 and y0 < y1".into());
                }
                region = Some(r);
            }
            "--map" => {
                map = match value()?.as_str() {
                    "online" => Map::Online,
                    "story" => Map::Story,
                    m => return Err(format!("gta: --map {m}: online or story").into()),
                }
            }
            "--hour" => {
                hour = Some(
                    value()?
                        .parse::<u8>()
                        .ok()
                        .filter(|h| *h < 24)
                        .ok_or("gta: --hour is 0-23")?,
                )
            }
            "--interiors" => {
                interiors = match value()?.as_str() {
                    "none" => false,
                    "all" => true,
                    v => return Err(format!("gta: --interiors {v}: none or all").into()),
                }
            }
            "--min-size" => {
                min_size = Some(value()?.parse::<f32>().map_err(|_| "gta: --min-size is metres")?)
            }
            "--max-priority" => {
                max_priority = Some(
                    value()?
                        .parse::<u8>()
                        .ok()
                        .filter(|p| *p <= 3)
                        .ok_or("gta: --max-priority is 0-3")?,
                )
            }
            _ => return Err(format!("gta: unknown option {a}").into()),
        }
    }
    let (Some(dir), Some(keys), Some(region)) = (dir, keys, region) else {
        return Err("gta: needs --gta, --keys and --region".into());
    };
    let mut selection = Selection::new(region);
    selection.map = map;
    selection.hour = hour;
    selection.interiors = interiors;
    if let Some(m) = min_size {
        selection.min_size = m;
    }
    if let Some(p) = max_priority {
        selection.max_priority = p;
    }
    Ok(Box::new(Gta {
        source: GtaSource::open(&dir, &keys, map)?,
        selection,
    }))
}

impl Reader for Gta {
    fn read(&mut self) -> Result<World> {
        let (w, report) = self.source.read(self.selection.region, REACH)?;
        println!("gta region: {:?}", report.region);
        println!("gta lights: {:?}", report.lights);
        println!("gta bounds: {:?}", report.bounds);
        Ok(w)
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<World> {
        let (w, report) = self.source.load(world, roots)?;
        if !report.textures_not_found.is_empty() || report.asset_missing > 0 {
            println!(
                "gta assets: {} prototypes without a live file, textures not found {:?}",
                report.asset_missing, report.textures_not_found
            );
        }
        Ok(w)
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "gta"]
    }
    fn default_select(&self) -> String {
        select_passes(&self.selection)
    }
    fn default_passes(&self) -> &'static str {
        DEFAULT_PASSES
    }
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        prototype_box(world, prototype)
    }
}
