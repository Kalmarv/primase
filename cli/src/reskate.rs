//! `--to reskate`: a native ReSkate Studio map folder (`<out>/<name>`, [`primase_reskate::write_map`]) of the whole
//! selection as one part, built from the world in memory. The part must have been through the passes the map needs
//! after the reader's (`--passes`): `bake-normal-scale` (skate. has no normal strength), `raise-skate{name=<name>}`
//! (the skate dialect; a spawn comes from `place-spawn` at selection), `lower-layered-reskate` (blends as decal
//! overlays; the target refuses an unlowered blend), then the texture choices: `cap-textures{max=N}` (the writer
//! writes textures at the size they come; the old writer's own cap was `cap-textures{max=256,normals=box}`) and
//! `prepare-reskate-textures` (material tints multiplied into texture copies; the writer writes no tint), a budget
//! if wanted (`budget-reskate-textures{max=N}`), `prepare-reskate-grinds` for grind curves (the writer builds no
//! prism), `snap-reskate-markers` (the writer snaps no marker) and, last, `prepare-reskate-overlays{step=0.004}` (the
//! writer stacks no decal of its own).
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::convert::{Report, Writer};
use primase_core::pass::Target;
use primase_reskate::{EncodingOptions, WriterChoices};

use crate::convert::Result;

struct Reskate {
    encoding: EncodingOptions,
    choices: WriterChoices,
    report: Report,
    /// What the map writer did not write, not yet taken by the conversion.
    notices: Vec<primase_core::convert::Notice>,
}

/// `[--name N] [--blend-objects N] [--min-instances N]`
pub fn writer(out: &Path, args: &[String]) -> Result<crate::convert::Destination> {
    let mut name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "map".into());
    let mut encoding = EncodingOptions {
        out_root: out.to_path_buf(),
        ..EncodingOptions::default()
    };
    let mut choices = WriterChoices::default();
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
                choices.blend_objects = Some(
                    value()?
                        .parse()
                        .map_err(|_| "reskate: --blend-objects is a number")?,
                )
            }
            "--blend-grid" => {
                choices.blend_grid = value()?
                    .parse()
                    .ok()
                    .filter(|g: &f32| g.is_finite() && *g >= 0.0)
                    .ok_or("reskate: --blend-grid is a size in metres (0: not cut)")?
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
            choices,
            report: Report::new(),
            notices: Vec::new(),
        }),
    ))
}

impl Writer for Reskate {
    /// The map builder writes the skate dialect and core and leaves other names alone; a blend (`core.layered`) it
    /// doesn't write: `lower-layered-reskate` lowers blends to decal overlays first.
    fn target(&self) -> Target {
        Target::new("reskate").illegal("core.layered").carry_unknown()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        // the world carries every input in memory: no scene folder to read from
        let report = primase_reskate::write_map(part, &PathBuf::new(), &self.encoding, &self.choices)?;
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
