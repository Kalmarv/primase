//! One STUNTBOOST model from the install ([`read_model`]): a prop, a creature or an animated object, as the game
//! stores it, through primase's glTF reader, so its skeleton, skin, morph targets and clips are read as they are for
//! any glTF ([`primase_gltf::read`]).
//!
//! Files (uncompressed XNB v5 under `Content/Models`): `Export/<Name>.xnb` (`BytingPipeline.GLTFReader`: the Blender
//! exporter's glTF 2.0 JSON) with its buffer `Export/<buffer uri>.xnb` (`ArrayReader<Byte>`) and each image
//! `<image uri relative to Export, .png -> .xnb>` (QOI or XNA's `Texture2D`, as in [`super::level`]). A level's parts
//! (`Level_*`) are read by [`super::read_level`], which keeps what the game's name grammar says of their nodes; the
//! derived `<Name>Collision.xnb` and `<Name>Cuts.xnb` are not glTF (another reader) and are not models.
//!
//! The units are the file's, as a glTF's are (metres by the format): STUNTBOOST states no unit of its own, so
//! [`read_level`](super::read_level)'s scale choice has no counterpart here; a program scales the world after reading.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the model's files from the install"
)]

use std::path::Path;

use primase_core::World;
use primase_glb::Gltf;
use primase_image::{Image, Samples};
use primase_monogame::xnb::Xnb;

use super::level::{bytes_of, content, export_dir, format, image_levels, string_of};
use crate::{Error, Result};

/// The reader name of a glTF the game stores.
const GLTF_READER: &str = "BytingPipeline.GLTFReader";

/// What [`read_model`] read: the glTF reader's report (what it read and what it did not, by name).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReport {
    pub gltf: primase_gltf::ReadReport,
}

/// Every model the install ships, by name in byte order: a file of `Content/Models/Export` that is not a level's part,
/// a buffer or an image, and whose one object the glTF reader wrote. Each file is opened as far as its header.
pub fn models(install: &Path) -> Result<Vec<String>> {
    let export = export_dir(install);
    let io = |source| Error::Io {
        path: export.clone(),
        source,
    };
    let mut names = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&export).map_err(io)? {
        let file = entry.map_err(io)?.file_name();
        let Some(name) = file
            .to_str()
            .and_then(|f| f.strip_suffix(".xnb"))
            .filter(|n| !n.is_empty() && !n.ends_with(".bin") && !n.starts_with("Level_"))
        else {
            continue;
        };
        let bytes = content(&export.join(format!("{name}.xnb")))?;
        if Xnb::parse(&bytes)?
            .primary()
            .is_ok_and(|(reader, _)| reader.type_name() == GLTF_READER)
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names.into_iter().collect())
}

/// An image file of the install as the PNG a glTF reader decodes (its largest level, RGBA8).
fn png_of(path: &Path) -> Result<Vec<u8>> {
    let (width, height, levels) = image_levels(path)?;
    let rgba = levels
        .into_iter()
        .next()
        .ok_or_else(|| format(&path.display().to_string(), "an image of no levels"))?;
    Ok(primase_image::write_png(&Image {
        width,
        height,
        samples: Samples::U8(rgba),
    })?)
}

/// Model `name` (`Dragonfly` for `Export/Dragonfly.xnb`) of the install at `install` into `w`, which must be empty, as
/// one asset ([`primase_gltf::read`]'s: its skeletons, skins, morphs, placements and clips; `core.source` is
/// `Export/<name>.xnb`). The images it names are read from the install's own files (an image the install does not ship
/// is refused by name in the report, its slot left unbound).
pub fn read_model(install: &Path, name: &str, w: &mut World) -> Result<ModelReport> {
    let export = export_dir(install);
    let file = format!("{name}.xnb");
    let path = export.join(&file);
    let text = string_of(&path, Some(GLTF_READER))?;
    let gltf = Gltf::from_gltf(text.as_bytes(), |_, uri| {
        bytes_of(&export.join(format!("{uri}.xnb")), None).map_err(|e| e.to_string())
    })?;
    let gltf = primase_gltf::read_with_images(&gltf, &format!("Export/{file}"), w, &mut |uri| {
        let image = export.join(uri).with_extension("xnb");
        if !image.is_file() {
            return Err(format!("{} is not in the install", image.display()));
        }
        png_of(&image).map_err(|e| e.to_string())
    })?;
    Ok(ModelReport { gltf })
}

#[cfg(test)]
mod tests {
    use primase_core::{Clip, Skeleton};

    use super::*;

    fn seven_bit(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    /// An uncompressed XNB v5 whose one object (`body`) a reader named `reader` wrote.
    fn xnb(reader: &str, body: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        seven_bit(1, &mut content);
        seven_bit(reader.len(), &mut content);
        content.extend_from_slice(reader.as_bytes());
        content.extend_from_slice(&0i32.to_le_bytes());
        seven_bit(0, &mut content);
        seven_bit(1, &mut content);
        content.extend_from_slice(body);
        let mut out = b"XNBd\x05\x00".to_vec();
        out.extend_from_slice(&u32::try_from(10 + content.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&content);
        out
    }

    fn string(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        seven_bit(s.len(), &mut out);
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn bytes(b: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(b.len()).unwrap().to_le_bytes().to_vec();
        out.extend_from_slice(b);
        out
    }

    fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    const STRING_READER: &str = "Microsoft.Xna.Framework.Content.StringReader, MonoGame.Framework";
    const BYTES_READER: &str =
        "Microsoft.Xna.Framework.Content.ArrayReader`1[[System.Byte, System.Private.CoreLib]]";

    /// A two-joint creature as the game ships one: a skinned triangle on joints `a` and `b` (`b` under `a`), and a clip
    /// moving `b` up by one unit over a second. Its buffer holds the vertices, the joints and weights, the inverse
    /// binds and the clip's times and values.
    fn ship(dir: &Path) {
        let export = dir.join("Content").join("Models").join("Export");
        std::fs::create_dir_all(&export).unwrap();
        let mut buffer = floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        buffer.extend([0u8, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]); // JOINTS_0: three u8x4 vertices
        buffer.extend(floats(&[1., 0., 0., 0., 0.5, 0.5, 0., 0., 1., 0., 0., 0.]));
        let ibm = |ty: f32| floats(&[1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., -ty, 0., 1.]);
        buffer.extend(ibm(0.0));
        buffer.extend(ibm(1.0));
        buffer.extend(floats(&[0., 1.])); // times
        buffer.extend(floats(&[0., 1., 0., 0., 2., 0.])); // b's translation
        let gltf = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0, 2]}],
            "nodes": [
                {"name": "a", "children": [1]},
                {"name": "b", "translation": [0.0, 1.0, 0.0]},
                {"name": "skin", "mesh": 0, "skin": 0}
            ],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2}}]}],
            "skins": [{"joints": [0, 1], "inverseBindMatrices": 3}],
            "animations": [{"name": "rise", "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}],
                "samplers": [{"input": 4, "output": 5, "interpolation": "LINEAR"}]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
                {"bufferView": 1, "componentType": 5121, "count": 3, "type": "VEC4"},
                {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
                {"bufferView": 3, "componentType": 5126, "count": 2, "type": "MAT4"},
                {"bufferView": 4, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0], "max": [1]},
                {"bufferView": 5, "componentType": 5126, "count": 2, "type": "VEC3"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 12},
                {"buffer": 0, "byteOffset": 48, "byteLength": 48},
                {"buffer": 0, "byteOffset": 96, "byteLength": 128},
                {"buffer": 0, "byteOffset": 224, "byteLength": 8},
                {"buffer": 0, "byteOffset": 232, "byteLength": 24}
            ],
            "buffers": [{"uri": "Bug.bin", "byteLength": 256}]
        });
        std::fs::write(
            export.join("Bug.xnb"),
            xnb(
                "BytingPipeline.GLTFReader, BytingPipeline",
                &string(&gltf.to_string()),
            ),
        )
        .unwrap();
        std::fs::write(export.join("Bug.bin.xnb"), xnb(BYTES_READER, &bytes(&buffer))).unwrap();
        // not models: a level's part, a derived file, a sidecar
        std::fs::write(
            export.join("Level_X_0.xnb"),
            xnb("BytingPipeline.GLTFReader, BytingPipeline", &string("{}")),
        )
        .unwrap();
        std::fs::write(
            export.join("BugCuts.xnb"),
            xnb("BytingPipeline.CutsReader, BytingPipeline", &[0]),
        )
        .unwrap();
        std::fs::write(export.join("Level_X_.xnb"), xnb(STRING_READER, &string("{}"))).unwrap();
    }

    /// A model is a file of the export folder that the glTF reader wrote and is not a level's: the listing holds it
    /// and nothing else.
    #[test]
    fn the_listing_holds_the_gltf_models_only() {
        let dir = std::env::temp_dir().join(format!("primase-stuntboost-models-{}", std::process::id()));
        ship(&dir);
        assert_eq!(models(&dir).unwrap(), ["Bug"]);
        assert!(models(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Its skeleton, skin and clip are read as any glTF's are: two joints, a binding on the mesh, and the one clip
    /// with one joint channel of two keys.
    #[test]
    fn a_shipped_creature_reads_with_its_skeleton_skin_and_clip() {
        let dir = std::env::temp_dir().join(format!("primase-stuntboost-model-{}", std::process::id()));
        ship(&dir);
        let mut w = World::new();
        let r = read_model(&dir, "Bug", &mut w).unwrap();
        assert_eq!(
            (
                r.gltf.skeletons,
                r.gltf.joints,
                r.gltf.bindings,
                r.gltf.clips,
                r.gltf.channels
            ),
            (1, 2, 1, 1, 1)
        );
        let skeleton = w.iter::<Skeleton>().next().map(|(_, s)| s).unwrap();
        assert_eq!(skeleton.joints.len(), 2);
        let clip = w.iter::<Clip>().next().map(|(_, c)| c).unwrap();
        assert_eq!((clip.name.as_str(), clip.duration), ("rise", 1.0));
        assert!(read_model(&dir, "Level_X_0", &mut World::new()).is_err());
        assert!(read_model(&dir, "Missing", &mut World::new()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
