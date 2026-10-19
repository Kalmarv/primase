//! `--to reskate`: a native ReSkate Studio map folder (`<out>/<name>`, [`primase_reskate::write_map`]) of the whole
//! selection as one part, built from the world in memory. The part must have been through the passes the map needs
//! after the reader's (`--passes`): `bake-normal-scale` (skate. has no normal strength), `raise-skate{name=<name>}`
//! (the skate dialect; a spawn comes from `place-spawn` at selection), `lower-layered-reskate` (blends as decal
//! overlays; the target refuses an unlowered blend), then the texture choices: `cap-textures{max=N}` (the writer
//! writes textures at the size they come; the old writer's own cap was `cap-textures{max=256,normals=box}`) and
//! `prepare-reskate-textures` (material tints multiplied into texture copies; the writer writes no tint), a budget
//! if wanted (`budget-reskate-textures{max=N}`), `prepare-reskate-grinds` for grind curves (the writer builds no
//! prism), `snap-reskate-markers` (the writer snaps no marker), the pause map if wanted
//! (`prepare-reskate-pause-map{from=geometry,height=2160,extent=content}`, what the old writer always drew; without it
//! the map has none) and, last, `prepare-reskate-overlays{step=0.004}` (the writer stacks no decal of its own) and
//! `prepare-reskate-blend-order{grid=200}` (the writer splits no blend chain of its own), then
//! `plan-reskate-blend-budget{objects=pool}` (how many chain objects; without it every tile's chains are plain).
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::convert::{Report, Writer};
use primase_core::pass::Target;
use primase_reskate::EncodingOptions;

use crate::convert::Result;

struct Reskate {
    encoding: EncodingOptions,
    report: Report,
    /// What the map writer did not write, not yet taken by the conversion.
    notices: Vec<primase_core::convert::Notice>,
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
            encoding,
            report: Report::new(),
            notices: Vec::new(),
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
        // the world carries every input in memory: no scene folder to read from
        let report = primase_reskate::write_map(part, &PathBuf::new(), &self.encoding)?;
        self.report.push((
            "reskate".into(),
            format!(
                "{}: tiles={} merged_tris={} instances={} prototypes={} materials={} textures={} collision_tris={}",
                report.out_dir.display(),
                report.tiles,
                report.merged_tris,
                report.instances,
                report.prototypes,
                report.materials,
                report.textures,
                report.collision_tris
            ),
        ));
        self.report
            .push(("blend order".into(), format!("{:?}", report.blend)));
        for (what, n) in report.unsupported {
            self.notices.push(primase_core::convert::Notice::new(
                "reskate",
                primase_core::convert::NoticeKind::Unsupported,
                format!("{what}: not written"),
                n,
            ));
        }
        Ok(())
    }
    fn notices(&mut self) -> Vec<primase_core::convert::Notice> {
        std::mem::take(&mut self.notices)
    }
    fn finish(self: Box<Self>) -> Result<primase_core::convert::Finished> {
        Ok(self.report.into())
    }
}
