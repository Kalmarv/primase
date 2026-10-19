//! `--from gta`: a region of a GTA V install ([`primase_gta::convert::GtaReader`]); this module parses its options.
use std::path::PathBuf;

use primase_core::convert::Reader;
use primase_gta::convert::{GtaOptions, GtaReader, map_named};
use primase_gta::pipeline::Map;

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

/// How far beyond the region entities are read unless `--reach` says otherwise, metres: large pieces of the world reach
/// in.
const REACH: f32 = 600.0;

use crate::convert::Result;

fn region_names() -> String {
    REGIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// `--gta <install> --keys <key folder> --region <name>|x0,y0,x1,y1 [--map online|story] [--reach M]`: what to read
/// and which map's assets to load (`--map online` and `--reach` 600 unless given). Which map state, hour, sizes and
/// interiors the conversion keeps are its `--select` passes.
pub fn reader(args: &[String]) -> Result<Box<dyn Reader>> {
    let mut reach = REACH;
    let (mut dir, mut keys, mut region, mut map) = (None, None, None, Map::Online);
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
            "--reach" => {
                reach = value()?
                    .parse::<f32>()
                    .ok()
                    .filter(|r| r.is_finite() && *r >= 0.0)
                    .ok_or("gta: --reach is metres")?
            }
            _ => return Err(format!("gta: unknown option {a}").into()),
        }
    }
    let (Some(install), Some(keys), Some(region)) = (dir, keys, region) else {
        return Err("gta: needs --gta, --keys and --region".into());
    };
    Ok(Box::new(GtaReader::open(&GtaOptions {
        install,
        keys,
        region,
        reach,
        map,
    })?))
}
