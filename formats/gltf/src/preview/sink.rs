//! [`PreviewParts`]: the preview writer in a conversion.

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::convert::{Finished, Writer};
use primase_core::pass::Target;
use primase_core::state::StateAxis;
use primase_core::{Asset, Component, Name, Source, World};

use super::describe;
use super::manifest::{self as m, Manifest};
use crate::parts::preview_target;
use crate::{GlbParts, WriteOptions};

/// What the manifest carries of entities the glTF does not write: an asset root and a state axis, with the name and
/// source the manifest states as their label and id. Their other components stay told as left out.
fn carried(part: &World) -> BTreeMap<&'static str, u64> {
    let mut out = BTreeMap::new();
    let carriers: Vec<_> = part
        .entities()
        .filter(|&e| part.has::<Asset>(e) || part.has::<StateAxis>(e))
        .collect();
    for e in carriers {
        for name in [Asset::NAME, StateAxis::NAME, Name::NAME, Source::NAME] {
            if part.entity_has_named(e, name) {
                *out.entry(name).or_default() += 1;
            }
        }
    }
    out
}

/// One GLB per part ([`GlbParts`]) and, at the end, `manifest.json` in the same folder (see the module
/// documentation). The options are [`GlbParts`]' with [`WriteOptions::all_variants`] on, since a preview offers the
/// variants a placement may take.
///
/// The manifest's fidelity list holds what the writer left out; what the passes approximated is added by the program
/// ([`Manifest::add_notices`], then [`Manifest::write_file`]) once the conversion's report is in, since the passes run
/// before the writer sees a part but the report is the conversion's.
#[derive(Debug)]
pub struct PreviewParts {
    parts: GlbParts,
    folder: PathBuf,
    manifest: Manifest,
}

impl PreviewParts {
    /// Parts written into `folder` (made if it is not there) with `options`, which may not hold a selection.
    pub fn new(folder: impl Into<PathBuf>, mut options: WriteOptions) -> crate::Result<PreviewParts> {
        let folder = folder.into();
        options.all_variants = true;
        Ok(PreviewParts {
            parts: GlbParts::new(&folder, options)?.carrying(carried),
            folder,
            manifest: Manifest::new(),
        })
    }
}

impl Writer for PreviewParts {
    fn target(&self) -> Target {
        preview_target()
    }

    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        let (name, report) = self.parts.write_part(part)?;
        let described = describe::describe(part, &name, &report.layout)?;
        describe::merge(&mut self.manifest, described)?;
        Ok(())
    }

    fn finish(self: Box<Self>) -> primase_core::convert::Result<Finished> {
        let PreviewParts {
            parts,
            folder,
            mut manifest,
        } = *self;
        manifest.files = parts
            .files()
            .iter()
            .map(|(name, bytes)| m::File {
                name: name.clone(),
                bytes: *bytes,
            })
            .collect();
        let mut finished = Box::new(parts).finish()?;
        manifest.add_notices(&finished.notices);
        manifest.write_file(&folder.join(m::FILE_NAME))?;
        finished.report.push((
            "preview manifest".to_owned(),
            format!(
                "{}: {} choices, {} assets, {} fidelity entries",
                m::FILE_NAME,
                manifest.choices.len(),
                manifest.assets.len(),
                manifest.fidelity.len()
            ),
        ));
        Ok(finished)
    }
}
