//! `--from gta`: a region of a GTA V install ([`primase_gta::convert::GtaReader`]); this module parses its options.
use std::path::PathBuf;

use primase_core::convert::Reader;
use primase_gta::convert::{GtaOptions, GtaReader, map_named};
use primase_gta::pipeline::{Map, Selection};

/// Named regions, `[x0, y0, x1, y1]` in GTA ground coordinates (metres east, north): the areas every writer is checked
/// on.
/// - `legion`: Legion Square, a downtown plaza (the earlier skate export's gate region).
/// - `vespucci`: Vespucci Beach from the skatepark (`vb_34_skate`, about -1374, -1396) along the boardwalk to Muscle
///   Beach (`vb_34_musclebeach`, about -1205, -1572), with the sand to the water: sand, grass and pavement terrain
///   blends, palms, decals and water edges.
const REGIONS: &[(&str, [f32; 4])] = &[
    ("legion", [95.0, -1034.0, 295.0, -834.0]),
    ("vespucci", [-1500.0, -1650.0, -1150.0, -1330.0]),
];

/// The region named `name` in [`REGIONS`].
fn named_region(name: &str) -> Option<[f32; 4]> {
    REGIONS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

/// The cli's selection, unless options say otherwise: GTA Online's map, entities read 600 m beyond the region (large
/// pieces of the world reach in), placements smaller than 0.5 m left out (the earlier skate export's rules), interiors
/// left out (the game draws an interior only from inside it).
const REACH: f32 = 600.0;
const MIN_SIZE: f32 = 0.5;

use crate::convert::Result;

fn region_names() -> String {
    REGIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// `--gta <install> --keys <key folder> --region <name>|x0,y0,x1,y1 [--map online|story] [--hour H | --keep-hour]
/// [--min-size M] [--max-priority N] [--interiors none|all] [--reach M]`; `--map online`, `--min-size` 0.5, `--reach`
/// 600 and `--interiors none` unless given. `--keep-hour` keeps the hour as state (night by `gta.hour`).
pub fn reader(args: &[String]) -> Result<Box<dyn Reader>> {
    let mut interiors = false;
    let mut keep_hour = false;
    let mut reach = REACH;
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
                let m = value()?;
                map = map_named(m).ok_or_else(|| format!("gta: --map {m}: online or story"))?;
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
            "--keep-hour" => keep_hour = true,
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
            "--reach" => {
                reach = value()?
                    .parse::<f32>()
                    .ok()
                    .filter(|r| r.is_finite() && *r >= 0.0)
                    .ok_or("gta: --reach is metres")?
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
    let (Some(install), Some(keys), Some(region)) = (dir, keys, region) else {
        return Err("gta: needs --gta, --keys and --region".into());
    };
    let mut selection = Selection::new(region, map);
    selection.reach = reach;
    selection.min_size = MIN_SIZE;
    if keep_hour && hour.is_some() {
        return Err("gta: --hour and --keep-hour: the hour is either given or kept".into());
    }
    selection.hour = hour;
    selection.keep_hour = keep_hour;
    selection.interiors = interiors;
    if let Some(m) = min_size {
        selection.min_size = m;
    }
    if let Some(p) = max_priority {
        selection.max_priority = p;
    }
    Ok(Box::new(GtaReader::open(&GtaOptions {
        install,
        keys,
        selection,
    })?))
}
