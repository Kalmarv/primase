//! What the scene loader keeps of a file it decoded (`skate.loaded_file`): the file's bytes and a fingerprint of the
//! payload they decoded to. The decoded payload is the authority: a writer emits the original bytes only while the
//! payload still has that fingerprint (an unchanged texture or mesh passes through byte for byte), encodes the payload
//! otherwise, and never reopens the file.

use std::sync::Arc;

use primase_core::{Component, JsonError, Mesh, Stream, Texture};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A file's bytes as the loader read them, and the fingerprint of what they decoded to ([`texture_fingerprint`],
/// [`mesh_fingerprint`], [`image_fingerprint`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedFile {
    pub bytes: Arc<[u8]>,
    pub decoded: [u8; 32],
}

impl Component for LoadedFile {
    const NAME: &'static str = "skate.loaded_file";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// None: the loader's copy of a file's bytes is bulk data kept for writing the file through, not content, so it
    /// has no view (a writer that shows components, as glTF extras, leaves it out).
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(None)
    }
}

impl LoadedFile {
    /// The original bytes, while `decoded` (the payload's fingerprint now) is still what they decoded to.
    pub fn unchanged(&self, decoded: [u8; 32]) -> Option<&Arc<[u8]>> {
        (self.decoded == decoded).then_some(&self.bytes)
    }
}

/// A texture's pixels: its size, format and every stored level (not its colour space, role or normal convention, which
/// the scene states beside the file).
pub fn texture_fingerprint(t: &Texture) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(t.width.to_le_bytes());
    h.update(t.height.to_le_bytes());
    h.update(format!("{:?}", t.format).as_bytes());
    for l in &t.levels {
        h.update((l.len() as u64).to_le_bytes());
        h.update(l);
    }
    h.finalize().into()
}

/// A mesh's geometry: each primitive's topology, streams and indices (not its material, which the scene links beside
/// the file).
pub fn mesh_fingerprint(m: &Mesh) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in m.primitives() {
        h.update(format!("{:?}", p.topology).as_bytes());
        for (semantic, stream) in &p.streams {
            h.update(format!("{semantic:?}").as_bytes());
            h.update((stream.len() as u64).to_le_bytes());
            match stream {
                Stream::F32x2(v) => v
                    .iter()
                    .flatten()
                    .for_each(|x| h.update(x.to_bits().to_le_bytes())),
                Stream::F32x3(v) => v
                    .iter()
                    .flatten()
                    .for_each(|x| h.update(x.to_bits().to_le_bytes())),
                Stream::F32x4(v) => v
                    .iter()
                    .flatten()
                    .for_each(|x| h.update(x.to_bits().to_le_bytes())),
                Stream::U8x4(v) => v.iter().for_each(|x| h.update(x)),
                Stream::U16x4(v) => v.iter().flatten().for_each(|x| h.update(x.to_le_bytes())),
            }
        }
        match &p.indices {
            None => h.update([0]),
            Some(i) => {
                h.update([1]);
                h.update((i.len() as u64).to_le_bytes());
                i.iter().for_each(|x| h.update(x.to_le_bytes()));
            }
        }
    }
    h.finalize().into()
}

/// An RGBA8 picture's size and pixels (the scene's pause-map picture).
pub fn image_fingerprint(width: u32, height: u32, rgba: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(width.to_le_bytes());
    h.update(height.to_le_bytes());
    h.update(rgba);
    h.finalize().into()
}
