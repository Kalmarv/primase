//! `--to reskate`: a native ReSkate Studio map folder (`<out>/<name>`, [`primase_reskate::build`]) of the whole
//! selection as one part, built from the world in memory. The part must have been through the passes the map needs
//! after the reader's (`--passes`): `bake-normal-scale` (skate. has no normal strength), `raise-skate{name=<name>,
//! spawn=centre}` (the skate dialect, a spawn at the centre of the placements snapped to the ground) and
//! `lower-layered-reskate` (blends as decal overlays); the target refuses an unlowered blend.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use primase_core::convert::{Report, Writer};
use primase_core::pass::Target;
use primase_core::{Entity, World};
use primase_reskate::BuildOptions;

use crate::convert::Result;

struct Reskate {
    options: BuildOptions,
    roots: Option<BTreeSet<Entity>>,
    report: Report,
}

/// `[--name N] [--blend-objects N] [--min-instances N]`
pub fn writer(out: &Path, args: &[String]) -> Result<Box<dyn Writer>> {
    let mut name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "map".into());
    let mut options = BuildOptions {
        out_root: out.to_path_buf(),
        ..BuildOptions::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("reskate: {a} needs a value"));
        match a.as_str() {
            "--name" => name.clone_from(value()?),
            "--min-instances" => {
                options.min_instances = value()?
                    .parse()
                    .map_err(|_| "reskate: --min-instances is a number")?
            }
            "--blend-objects" => {
                options.blend_objects = value()?
                    .parse()
                    .map_err(|_| "reskate: --blend-objects is a number")?
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
    options.name = Some(name);
    Ok(Box::new(Reskate {
        options,
        roots: None,
        report: Report::new(),
    }))
}

impl Writer for Reskate {
    /// Blends are lowered to decal overlays (`lower-layered-reskate`); the map builder reads the skate dialect and
    /// core and leaves other names alone.
    fn target(&self) -> Target {
        Target::new("reskate").illegal("core.layered").carry_unknown()
    }
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        // one part: everything selected
        self.roots = Some(world.entities().collect());
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>> {
        Ok(self.roots.take())
    }
    fn write(&mut self, part: &World) -> Result<()> {
        // the world carries every input in memory: no scene folder to read from
        let report = primase_reskate::build(part, &PathBuf::new(), &self.options)?;
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
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Report> {
        Ok(self.report)
    }
}
