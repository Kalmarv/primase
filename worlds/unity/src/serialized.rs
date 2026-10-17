//! The SerializedFile container: what `globalgamemanagers`, `*.assets` and `levelN` are.
//!
//! Layout (container version 22): a big-endian header (legacy fields zero, the version, an endianness byte, then
//! the metadata size, file size and data offset as 32/64/64-bit numbers), then the metadata in the file's
//! endianness: Unity version, target platform, whether type trees are embedded, the types (class id, stripped,
//! script index, hashes), the object table (path id, data offset, size, type index), script references, external
//! files, reference types and a user string. Objects' bytes follow at the data offset.
//!
//! Only versions verified against real data are read; others are refused, as are embedded type trees (player
//! builds strip them; decode with a [`TypeTreePackage`] instead).

use std::collections::BTreeMap;

use crate::tpk::TypeTreePackage;
use crate::typetree::TypeTree;
use crate::value::Value;
use crate::version::UnityVersion;
use crate::{Error, Reader, Result, invalid};

/// Container versions read (verified on real files).
pub const VERSIONS: std::ops::RangeInclusive<u32> = 22..=22;

/// The MonoBehaviour class: its type tree covers the base fields; the script's own fields follow.
pub const MONO_BEHAVIOUR: i32 = 114;

#[derive(Clone, Debug, PartialEq)]
pub struct SerializedType {
    pub class_id: i32,
    pub stripped: bool,
    /// Index into the script references (-1: none).
    pub script_type_index: i16,
    pub script_id: Option<[u8; 16]>,
    pub old_type_hash: [u8; 16],
}

/// One entry of the object table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
    pub path_id: i64,
    /// The object's bytes: offset in the file and size.
    pub offset: u64,
    pub size: u32,
    pub type_index: u32,
    pub class_id: i32,
}

/// Another file this one's pointers can name (file id = index + 1).
#[derive(Clone, Debug, PartialEq)]
pub struct External {
    pub guid: [u8; 16],
    pub kind: i32,
    pub path: String,
}

/// A decoded object: its value, and the bytes after what its type tree covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub value: Value,
    pub rest: usize,
}

impl Decoded {
    /// Whether the tree covered the object: every byte, or a MonoBehaviour's base fields (its script's fields follow,
    /// and a stripped build has no type trees for them).
    pub fn covered(&self, class_id: i32) -> bool {
        self.rest == 0 || class_id == MONO_BEHAVIOUR
    }
}

#[derive(Clone, Debug)]
pub struct SerializedFile {
    pub version: u32,
    pub unity_version: String,
    pub unity: UnityVersion,
    pub target_platform: i32,
    pub big_endian: bool,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    /// Script references: (file index, path id).
    pub script_types: Vec<(i32, i64)>,
    pub externals: Vec<External>,
    pub ref_types: Vec<SerializedType>,
    pub user_information: String,
    data: Vec<u8>,
    by_path_id: BTreeMap<i64, usize>,
}

fn hash(r: &mut Reader) -> Result<[u8; 16]> {
    Ok(r.take(16)?.try_into().expect("16 bytes"))
}

fn count(r: &mut Reader, what: &str) -> Result<usize> {
    let n = r.i32()?;
    let n = usize::try_from(n).map_err(|_| invalid(format!("negative {what} count {n}")))?;
    if n > r.remaining() {
        return Err(invalid(format!("{what} count {n} larger than the metadata")));
    }
    Ok(n)
}

fn serialized_type(r: &mut Reader, reference: bool) -> Result<SerializedType> {
    let class_id = r.i32()?;
    let stripped = r.bool()?;
    let script_type_index = r.i16()?;
    let script_id = if (reference && script_type_index >= 0) || (!reference && class_id == MONO_BEHAVIOUR) {
        Some(hash(r)?)
    } else {
        None
    };
    let old_type_hash = hash(r)?;
    Ok(SerializedType {
        class_id,
        stripped,
        script_type_index,
        script_id,
        old_type_hash,
    })
}

impl SerializedFile {
    /// Whether `bytes` start like a SerializedFile of a supported version (to tell them from other files).
    pub fn looks_like(bytes: &[u8]) -> bool {
        bytes.len() >= 48
            && bytes[..4] == [0; 4]
            && VERSIONS.contains(&u32::from_be_bytes(bytes[8..12].try_into().expect("4")))
    }

    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let mut h = Reader::new(&data, true);
        let _legacy_metadata = h.u32()?;
        let _legacy_size = h.u32()?;
        let version = h.u32()?;
        let _legacy_offset = h.u32()?;
        if !VERSIONS.contains(&version) {
            return Err(Error::Unsupported(format!(
                "SerializedFile version {version} (read: {}..={})",
                VERSIONS.start(),
                VERSIONS.end()
            )));
        }
        let big_endian = h.bool()?;
        h.take(3)?;
        let metadata_size = h.u32()? as usize;
        let file_size = h.u64()?;
        let data_offset = h.u64()?;
        let _reserved = h.u64()?;
        if file_size != data.len() as u64 {
            return Err(invalid(format!(
                "header file size {file_size}, file has {} bytes",
                data.len()
            )));
        }
        let meta_end = h.pos + metadata_size;
        if meta_end as u64 > data_offset || data_offset > file_size {
            return Err(invalid(format!(
                "metadata ends at {meta_end}, data at {data_offset}, file size {file_size}"
            )));
        }
        let mut r = Reader::new(&data[..meta_end], big_endian);
        r.pos = h.pos;
        let unity_version = r.cstr()?;
        let unity = UnityVersion::parse(&unity_version)?;
        let target_platform = r.i32()?;
        if r.bool()? {
            return Err(Error::Unsupported(
                "embedded type trees (decode stripped player files with a type tree package)".into(),
            ));
        }
        let types = (0..count(&mut r, "type")?)
            .map(|_| serialized_type(&mut r, false))
            .collect::<Result<Vec<_>>>()?;
        let mut objects = Vec::new();
        for _ in 0..count(&mut r, "object")? {
            r.align(4)?;
            let path_id = r.i64()?;
            let start = r.i64()?;
            let size = r.u32()?;
            let type_index = r.u32()?;
            let ty = types.get(type_index as usize).ok_or_else(|| {
                invalid(format!(
                    "object {path_id}: type index {type_index} of {}",
                    types.len()
                ))
            })?;
            let offset = u64::try_from(start)
                .ok()
                .and_then(|s| s.checked_add(data_offset))
                .filter(|&o| o + u64::from(size) <= file_size)
                .ok_or_else(|| invalid(format!("object {path_id}: bytes {start}+{size} outside the file")))?;
            objects.push(ObjectInfo {
                path_id,
                offset,
                size,
                type_index,
                class_id: ty.class_id,
            });
        }
        let mut script_types = Vec::new();
        for _ in 0..count(&mut r, "script")? {
            let file = r.i32()?;
            r.align(4)?;
            script_types.push((file, r.i64()?));
        }
        let mut externals = Vec::new();
        for _ in 0..count(&mut r, "external")? {
            let _empty = r.cstr()?;
            let guid = hash(&mut r)?;
            let kind = r.i32()?;
            let path = r.cstr()?;
            externals.push(External { guid, kind, path });
        }
        let ref_types = (0..count(&mut r, "reference type")?)
            .map(|_| serialized_type(&mut r, true))
            .collect::<Result<Vec<_>>>()?;
        let user_information = r.cstr()?;
        if r.pos != meta_end {
            return Err(invalid(format!(
                "metadata read to {}, header says it ends at {meta_end}",
                r.pos
            )));
        }
        let mut by_path_id = BTreeMap::new();
        for (i, o) in objects.iter().enumerate() {
            if by_path_id.insert(o.path_id, i).is_some() {
                return Err(invalid(format!("path id {} twice", o.path_id)));
            }
        }
        Ok(SerializedFile {
            version,
            unity_version,
            unity,
            target_platform,
            big_endian,
            types,
            objects,
            script_types,
            externals,
            ref_types,
            user_information,
            data,
            by_path_id,
        })
    }

    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.by_path_id.get(&path_id).map(|&i| &self.objects[i])
    }

    pub fn bytes(&self, o: &ObjectInfo) -> &[u8] {
        &self.data[o.offset as usize..o.offset as usize + o.size as usize]
    }

    /// Decode an object's bytes from its start with `tree` (its class's type tree), reporting the bytes after what the
    /// tree covers in [`Decoded::rest`] (see [`Decoded::covered`]).
    pub fn decode_with(&self, o: &ObjectInfo, tree: &TypeTree) -> Result<Decoded> {
        let bytes = self.bytes(o);
        let (value, used) = tree
            .decode(bytes, self.big_endian)
            .map_err(|e| invalid(format!("object {} (class {}): {e}", o.path_id, o.class_id)))?;
        Ok(Decoded {
            value,
            rest: bytes.len() - used,
        })
    }

    /// Decode an object with its class's type tree from `package`. The tree must cover the object's bytes, but for a
    /// MonoBehaviour, whose script fields follow its base fields.
    pub fn decode(&self, o: &ObjectInfo, package: &TypeTreePackage) -> Result<Decoded> {
        let tree = package.type_tree(o.class_id, self.unity)?;
        let d = self.decode_with(o, &tree)?;
        if !d.covered(o.class_id) {
            return Err(invalid(format!(
                "object {} (class {}): {} bytes after its type tree",
                o.path_id, o.class_id, d.rest
            )));
        }
        Ok(d)
    }
}
