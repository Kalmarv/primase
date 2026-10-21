//! The glTF writer in a conversion ([`primase_core::convert`]): what it writes ([`target`]) and [`GlbParts`], one GLB
//! per part, so a world divided by asset (`partition-by-asset`) becomes one file per object.
//!
//! A part is named by its asset: when it holds exactly one asset root that nothing in it references, the file is that
//! root's `core.source` (else its `core.name`), every character but ASCII letters, digits, `.`, `-` and `_` written as
//! `_` (`chr/c1010` -> `chr_c1010.glb`); any other part is `part-<n>.glb`, `n` its place in the order written, from 0.
//! Two parts that would share a file are refused rather than one overwriting the other.
use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::convert::{Finished, Notice, NoticeKind, Writer};
use primase_core::pass::Target;
use primase_core::state::{Condition, StateOverride};
use primase_core::{
    Asset, Clip, ClipBinding, Component, Entity, Instance, LayeredMaterial, Light, Lods, MaterialVariants,
    Mesh, MeshFile, MorphWeights, Morphs, Name, RigInstance, RigPose, Shadows, Skeleton, Skin, SkinInstance,
    Source, Texture, TextureFile, Tile, Transform, TransformKind, World, pbr::PbrMaterial,
};

use crate::{WriteOptions, WriteReport, write_file};

/// What the glTF writer writes, as a conversion checks each part against it ([`Target::check_prepared`]). Every
/// placement must have a transform glTF can hold (not sheared: `bake-nonrigid` first); a mesh or texture still only
/// in a file must be loaded; a conditional placement or a state override is refused (the glTF is one world state:
/// `resolve-state` chooses one). Levels of detail are counted, not chosen (`select-lod` chooses). Every other name is
/// carried: into the extras where it has a JSON form, counted where it has none (see [`crate::write`]).
pub fn target() -> Target {
    Target::new("gltf")
        .check::<Transform>("no transform sheared (bake-nonrigid first)", |t| {
            t.classify() != TransformKind::Sheared
        })
        .legal(Instance::NAME)
        .legal(Name::NAME)
        .legal(Mesh::NAME)
        .dynamic(MeshFile::NAME, |w, e| w.has::<Mesh>(e))
        .legal(MaterialVariants::NAME)
        .legal(Shadows::NAME)
        .legal(Light::NAME)
        .legal(PbrMaterial::NAME)
        .legal(LayeredMaterial::NAME)
        .legal(Texture::NAME)
        .dynamic(TextureFile::NAME, |w, e| w.has::<Texture>(e))
        .legal(Tile::NAME)
        .legal(Lods::NAME)
        .legal(Skeleton::NAME)
        .legal(Skin::NAME)
        .legal(RigInstance::NAME)
        .legal(RigPose::NAME)
        .legal(SkinInstance::NAME)
        .legal(Morphs::NAME)
        .legal(MorphWeights::NAME)
        .legal(Clip::NAME)
        .legal(ClipBinding::NAME)
        .illegal(Condition::NAME)
        .illegal(StateOverride::NAME)
        .carry_unknown()
}

/// One GLB per part, into a folder (see the module documentation), each written as [`crate::write_file`] writes a
/// world with the given options. What the writer leaves out is told once per reason over every part, the files as
/// examples ([`Writer::finish`]).
#[derive(Debug)]
pub struct GlbParts {
    folder: PathBuf,
    options: WriteOptions,
    /// Each file written, its bytes, in order.
    files: Vec<(String, u64)>,
    /// What every part left out: (kind, reason) -> (count, files).
    omitted: BTreeMap<(NoticeKind, String), (u64, Vec<String>)>,
}

impl GlbParts {
    /// Parts written into `folder` (made if it is not there) with `options`, which may not hold a selection (a tiled
    /// writer's: a part is written whole).
    #[allow(
        clippy::disallowed_methods,
        reason = "writer I/O: makes the folder the files are written into"
    )]
    pub fn new(folder: impl Into<PathBuf>, options: WriteOptions) -> crate::Result<GlbParts> {
        let folder = folder.into();
        if options.selection.is_some() {
            return Err(crate::Error::Invalid {
                item: "glb parts".into(),
                detail: "a selection is a tiled writer's; each part is written whole".into(),
            });
        }
        std::fs::create_dir_all(&folder).map_err(|source| crate::Error::Io {
            path: folder.clone(),
            source,
        })?;
        Ok(GlbParts {
            folder,
            options,
            files: Vec::new(),
            omitted: BTreeMap::new(),
        })
    }
}

/// `text` as a file stem: ASCII letters, digits, `.`, `-` and `_` kept, every other character `_`.
fn stem(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The part's file name (see the module documentation): its one asset's, else `part-<ordinal>`.
fn file_name(part: &World, ordinal: usize) -> String {
    let independent = part.unreferenced();
    let assets: Vec<Entity> = independent
        .into_iter()
        .filter(|&e| part.has::<Asset>(e))
        .collect();
    let named = match assets[..] {
        [root] => part
            .get::<Source>(root)
            .map(|s| s.0.as_str())
            .or_else(|| part.get::<Name>(root).map(|n| n.0.as_str()))
            .filter(|s| !s.is_empty())
            .map(stem),
        _ => None,
    };
    format!("{}.glb", named.unwrap_or_else(|| format!("part-{ordinal}")))
}

/// What `report` says the writer left out, chose or drew approximately, by reason (none with a count of 0):
/// `include_unplaced` tells whether unplaced prototypes were written.
fn omissions(report: &WriteReport, include_unplaced: bool) -> Vec<(NoticeKind, String, u64)> {
    use NoticeKind::{Approximation, Selection, Unsupported};
    let mut out: Vec<(NoticeKind, String, u64)> = vec![
        (
            Unsupported,
            "placements with a degenerate transform, not written".into(),
            report.placements_degenerate_skipped,
        ),
        (
            Unsupported,
            "placements whose prototype has no mesh: nothing drawn".into(),
            report.placements_without_mesh,
        ),
        (
            Unsupported,
            "placements of prototypes with levels of detail, not written: the writer chooses no form (select-lod)"
                .into(),
            report.placements_with_lods,
        ),
        (
            Selection,
            "prototypes nothing places, not written (include_unplaced writes them at the origin)".into(),
            if include_unplaced { 0 } else { report.prototypes_unplaced },
        ),
        (
            Unsupported,
            "materials without pbr.material or core.layered, not written: their primitives drawn with no material \
             (prepare-gltf-materials states one)"
                .into(),
            report.materials_without_pbr,
        ),
        (
            Unsupported,
            "layer slots whose material has no pbr.material, written empty".into(),
            report.layers_without_pbr,
        ),
        (
            Unsupported,
            "primitives with no material: glTF's default material".into(),
            report.primitives_without_material,
        ),
        (
            Approximation,
            "primitives drawn with a layered material but lacking its weight attribute: they render as its host"
                .into(),
            report.primitives_layered_without_weights,
        ),
        (
            Selection,
            "primitives drawn without their COLOR_0 stream: their material ignores vertex colour".into(),
            report.primitives_color0_ignored,
        ),
        (
            Unsupported,
            "material texture slots left empty: their texture was not written".into(),
            report.texture_slots_dropped,
        ),
        (
            Unsupported,
            "texture filters glTF cannot express (a minification filter without a mip filter, or the reverse): no \
             minFilter written"
                .into(),
            report.filters_unexpressed,
        ),
        (
            Unsupported,
            "placements of a skinned prototype without a rig: drawn in the bind pose, without the skin".into(),
            report.skinned_without_rig,
        ),
        (
            Unsupported,
            "meshes whose morph targets have in-between frames: targets not written (no glTF form)".into(),
            report.morphs_with_frames,
        ),
        (
            Unsupported,
            "additive clips, not written (no glTF form)".into(),
            report.clips_additive,
        ),
        (
            Unsupported,
            "clips' root motion, not written (no glTF form)".into(),
            report.root_motions,
        ),
        (
            Unsupported,
            "source control channels, not written (no glTF form)".into(),
            report.control_channels,
        ),
        (
            Unsupported,
            "morph channels of one occurrence with different key times or interpolation, not written".into(),
            report.morph_channels_unaligned,
        ),
        (
            Unsupported,
            "the environment's ambient light: glTF has none, carried in the scene's extras lighting nothing".into(),
            report.ambient_unlit,
        ),
    ];
    let by = |out: &mut Vec<(NoticeKind, String, u64)>,
              map: &BTreeMap<String, u64>,
              reason: &dyn Fn(&str) -> String| {
        out.extend(map.iter().map(|(k, &n)| (Unsupported, reason(k), n)));
    };
    by(&mut out, &report.textures_skipped, &|k| {
        format!("textures not written ({k}): their slots left empty")
    });
    by(&mut out, &report.textures_unrepresentable, &|k| {
        format!(
            "{k} textures an 8-bit PNG cannot hold exactly, not written (lower-textures-rgba8 converts them)"
        )
    });
    by(&mut out, &report.streams_skipped, &|k| {
        format!("{k} vertex streams not written: skin attributes of a mesh drawn without its skin")
    });
    by(&mut out, &report.extras_unencoded, &|k| {
        format!("{k} (a component or resource) without a JSON form, not in the extras")
    });
    by(&mut out, &report.components_unwritten, &|k| {
        format!("{k} components on entities the glTF does not write")
    });
    out.retain(|o| o.2 > 0);
    out
}

impl Writer for GlbParts {
    fn target(&self) -> Target {
        target()
    }

    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        let name = file_name(part, self.files.len());
        if self.files.iter().any(|(f, _)| *f == name) {
            return Err(format!(
                "two parts are written to {name}: their assets' sources (or names) make one file name"
            )
            .into());
        }
        let report = write_file(part, &self.options, &self.folder.join(&name))?;
        for (kind, reason, n) in omissions(&report, self.options.include_unplaced) {
            let (count, files) = self.omitted.entry((kind, reason)).or_default();
            *count += n;
            if files.len() < primase_core::convert::NOTICE_EXAMPLES {
                files.push(name.clone());
            }
        }
        self.files.push((name, report.glb_bytes));
        Ok(())
    }

    fn finish(self: Box<Self>) -> primase_core::convert::Result<Finished> {
        let total: u64 = self.files.iter().map(|(_, b)| b).sum();
        let mut report = vec![(
            "glb parts".to_owned(),
            format!(
                "{} files, {total} bytes, in {}",
                self.files.len(),
                self.folder.display()
            ),
        )];
        report.extend(
            self.files
                .iter()
                .map(|(name, bytes)| ("glb part".to_owned(), format!("{name}: {bytes} bytes"))),
        );
        let notices = self
            .omitted
            .into_iter()
            .map(|((kind, reason), (count, files))| Notice {
                stage: "gltf".into(),
                kind,
                reason,
                count,
                examples: files,
            })
            .collect();
        Ok(Finished { report, notices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stems_keep_only_safe_characters() {
        assert_eq!(stem("chr/c1010"), "chr_c1010");
        assert_eq!(stem("a b\\c:d*e.f-g_h"), "a_b_c_d_e.f-g_h");
        assert_eq!(stem("über"), "_ber");
    }

    /// Named by its one independent asset (source, else name), else by its place.
    #[test]
    fn a_part_is_named_by_its_one_asset() {
        let mut w = World::new();
        let root = w.spawn();
        w.insert(root, Asset::default());
        w.insert(root, Name("Wolf".into()));
        assert_eq!(file_name(&w, 3), "Wolf.glb");
        w.insert(root, Source("chr/c1010".into()));
        assert_eq!(file_name(&w, 3), "chr_c1010.glb");
        let other = w.spawn();
        w.insert(other, Asset::default());
        assert_eq!(file_name(&w, 3), "part-3.glb");
    }
}
