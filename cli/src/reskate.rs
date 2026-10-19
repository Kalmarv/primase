//! `--to reskate`: a native ReSkate Studio map folder (`<out>/<name>`, [`primase_reskate::write_map`]) of the whole
//! selection as one part, built from the world in memory. The part must have been through the passes the map needs
//! after the reader's (`--passes`): `bake-normal-scale` (skate. has no normal strength), `raise-skate{name=<name>,light_scale=standard}`
//! (the skate dialect; a spawn comes from `place-spawn` at selection), `lower-layered-reskate` (blends as decal
//! overlays; the target refuses an unlowered blend), then the texture choices: `cap-textures{max=N}` (the writer
//! writes textures at the size they come; the old writer's own cap was `cap-textures{max=256,normals=box}`) and
//! `prepare-reskate-textures` (material tints multiplied into texture copies; the writer writes no tint), a budget
//! if wanted (`budget-reskate-textures{max=N}`), `prepare-reskate-grinds` for grind curves (the writer builds no
//! prism), `snap-reskate-markers` (the writer snaps no marker), the pause map if wanted
//! (`prepare-reskate-pause-map{from=geometry,height=2160,extent=content}`, what the old writer always drew; without it
//! the map has none) and, last, `prepare-reskate-overlays{step=0.004}` (the writer stacks no decal of its own) and
//! `prepare-reskate-blend-order{grid=200}` (the writer splits no blend chain of its own), then
//! `prepare-reskate-collision{surfaces=coalesce,weld=mm}` (how the collision is written; without it, exactly), then
//! `summarize-reskate-map` and `plan-reskate-blend-budget{objects=pool}` (how many chain objects, decided before
//! writing; a map with blend chains and no budget is refused).
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::convert::{Report, Writer};
use primase_core::pass::Target;
use primase_reskate::{EncodingOptions, MapSink};

use crate::convert::Result;

struct Reskate {
    /// The map writer: each part's payloads written as the part comes, the index files at the end.
    sink: MapSink,
}

/// `[--name N] [--min-instances N]`
pub fn writer(out: &Path, args: &[String]) -> Result<crate::convert::Destination> {
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
            "--blend-objects" => {
                return Err(
                    "reskate: the chain-object budget is plan-reskate-blend-budget's: --passes \"..., \
                     plan-reskate-blend-budget{objects=N}\""
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
    encoding.name = Some(name);
    // one map: the whole selection in one part
    Ok((
        Box::new(primase_core::convert::WholeWorld::default()),
        Box::new(Reskate {
            // the world carries every input in memory: no scene folder to read from
            sink: MapSink::new(&PathBuf::new(), &encoding),
        }),
    ))
}

impl Writer for Reskate {
    /// The map builder's names (`primase_reskate::target`): the skate dialect and the core names it reads; any other
    /// name is reported as not written.
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        self.sink.part(part)?;
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<primase_core::convert::Finished> {
        let report = self.sink.finish()?;
        let mut lines: Report = vec![
            (
                "reskate".into(),
                format!(
                    "{}: tiles={} merged_tris={} instances={} prototypes={} materials={} textures={} collision_tris={} \
                     peak_mesh_bytes={}",
                    report.out_dir.display(),
                    report.tiles,
                    report.merged_tris,
                    report.instances,
                    report.prototypes,
                    report.materials,
                    report.textures,
                    report.collision_tris,
                    report.peak_mesh_bytes
                ),
            ),
            ("blend order".into(), format!("{:?}", report.blend)),
        ];
        // the map's warnings (map.json's), each counted below when it is a loss
        for w in report.warnings {
            lines.push(("reskate warning".into(), w));
        }
        // what it left out, and what it wrote approximately; the names the target does not take the conversion
        // reported itself before writing (report.unsupported_names, for direct callers)
        let mut notices = Vec::new();
        for (what, n) in report.omitted {
            notices.push(primase_core::convert::Notice::new(
                "reskate",
                primase_core::convert::NoticeKind::Unsupported,
                format!("{what}: not written"),
                n,
            ));
        }
        for (what, n) in report.approximated {
            notices.push(primase_core::convert::Notice::new(
                "reskate",
                primase_core::convert::NoticeKind::Approximation,
                what,
                n,
            ));
        }
        Ok(primase_core::convert::Finished {
            report: lines,
            notices,
        })
    }
}
