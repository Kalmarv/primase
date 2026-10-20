//! `--to reskate`: a native ReSkate Studio map folder (`<out>/<name>`) of the whole selection as one part, or of the
//! partition plan's parts as one map (`--parts`), written by the map sink ([`primase_reskate::MapSink`], the
//! conversion's writer) from the world in memory. The part must have been through the passes the map needs
//! after the reader's (`--passes`): `bake-normal-scale` (skate. has no normal strength), `raise-skate{name=<name>,light_scale=standard}`
//! (the skate dialect; a spawn comes from `place-spawn` at selection), `lower-layered-reskate` (blends as decal
//! overlays; the target refuses an unlowered blend), then the texture choices: `cap-textures{max=N}` (the writer
//! writes textures at the size they come; the old writer's own cap was `cap-textures{max=256,normals=box}`) and
//! `prepare-reskate-textures` (material tints multiplied into texture copies; the writer writes no tint), a budget
//! if wanted (`budget-reskate-textures{max=N}`), `prepare-reskate-grinds` for grind curves (the writer builds no
//! prism), `snap-reskate-markers` (the writer snaps no marker), the pause map if wanted
//! (`prepare-reskate-pause-map{from=geometry,height=2160,extent=content,leave-out=nothing,colour=mean}`, what the old writer always drew; without it
//! the map has none) and, last, `prepare-reskate-overlays{step=0.004}` (the writer stacks no decal of its own) and
//! `prepare-reskate-blend-order{grid=200}` (the writer splits no blend chain of its own), then
//! `prepare-reskate-collision{surfaces=coalesce,weld=mm}` (how the collision is written; without it, exactly), then
//! `summarize-reskate-map` and `plan-reskate-blend-budget{objects=pool}` (how many chain objects, decided before
//! writing; a map with blend chains and no budget is refused). A map in parts plans its collision and decides its
//! budget over every part instead (`convert --gather` with `share-reskate-collision` and `summarize-reskate-map`,
//! `--group` with `prepare-reskate-cell-collision`, `--finalize` with `finalize-reskate-blend-budget`), so its parts'
//! passes take none of `prepare-reskate-collision`, `plan-reskate-blend-budget` (both refuse a part) and
//! `summarize-reskate-map` (a part is handed the map's summary already).
use std::path::{Path, PathBuf};

use primase_reskate::spool::CellSpool;
use primase_reskate::{EncodingOptions, MapSink};

use crate::convert::Result;

/// `[--name N] [--min-instances N] [--parts [--spool <folder>]]`
pub fn writer(out: &Path, args: &[String]) -> Result<crate::convert::Destination> {
    let mut parts = false;
    let mut spool: Option<PathBuf> = None;
    let mut name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "map".into());
    let mut encoding = EncodingOptions {
        out_root: out.to_path_buf(),
        ..EncodingOptions::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("reskate: {a} needs a value"));
        match a.as_str() {
            "--name" => name.clone_from(value()?),
            "--min-instances" => {
                encoding.min_instances = value()?
                    .parse()
                    .map_err(|_| "reskate: --min-instances is a number")?
            }
            "--parts" => parts = true,
            "--spool" => spool = Some(PathBuf::from(value()?)),
            "--blend-objects" => {
                return Err(
                    "reskate: the chain-object budget is plan-reskate-blend-budget's (whole: --passes \"..., \
                     plan-reskate-blend-budget{objects=N}\"; in parts: --finalize \
                     \"finalize-reskate-blend-budget{objects=N}\")"
                        .into(),
                );
            }
            "--blend-grid" => {
                return Err(
                    "reskate: the blend grid is prepare-reskate-blend-order's: --passes \"..., \
                            prepare-reskate-blend-order{grid=M}\""
                        .into(),
                );
            }
            _ => return Err(format!("reskate: unknown option {a}").into()),
        }
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("reskate: the map name {name:?} is not [A-Za-z0-9_-]+ (--name)").into());
    }
    if spool.is_some() && !parts {
        return Err("reskate: --spool is for a map in parts (--parts)".into());
    }
    // one map: the whole selection in one part, or the partition plan's parts (the map sink lays them out as the
    // whole: their layout, instancing and content summaries merged over every part, --summarize; their collision
    // cells gathered in the cell spool, --gather and --group)
    let (source, spool): (Box<dyn primase_core::convert::PartSource>, _) = if parts {
        let dir = spool.unwrap_or_else(|| out.join(format!("{name}.cells")));
        let spool: Box<dyn primase_core::convert::Spool> = Box::new(CellSpool::new(&dir)?);
        (
            Box::new(primase_core::convert::PlannedParts::default()),
            Some(spool),
        )
    } else {
        (Box::new(primase_core::convert::WholeWorld::default()), None)
    };
    encoding.name = Some(name);
    // the world carries every input in memory: no scene folder to read from
    Ok(crate::convert::Destination {
        parts: source,
        writer: Box::new(MapSink::new(&PathBuf::new(), &encoding)),
        spool,
    })
}
