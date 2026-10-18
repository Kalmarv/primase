//! Type tree packages (TPK, "TPK*" files): the layouts of Unity's built-in classes for every Unity version, which a
//! player build strips from its files. Format versions 1 and 2, type tree data, uncompressed or LZ4 (LZMA and
//! Brotli packages are refused: re-pack them uncompressed).

use primase_compress::{Decompressor, Lz4};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::typetree::{Node, TypeTree};
use crate::version::UnityVersion;
use crate::{Error, Reader, Result, invalid};

const MAGIC: u32 = 0x2A4B_5054;
const HAS_EDITOR_ROOT: u8 = 64;
const HAS_RELEASE_ROOT: u8 = 128;

#[derive(Clone, Debug)]
struct Class {
    name: u16,
    flags: u8,
    release_root: Option<u16>,
}

#[derive(Clone, Debug)]
struct PackedNode {
    type_name: u16,
    name: u16,
    byte_size: i32,
    version: i16,
    meta_flag: u32,
    children: Vec<u16>,
}

/// A parsed type tree package.
#[derive(Clone, Debug)]
pub struct TypeTreePackage {
    /// Per class id, the class from each listed version on (`None`: absent from that version).
    classes: BTreeMap<i32, Vec<(UnityVersion, Option<Class>)>>,
    nodes: Vec<PackedNode>,
    strings: Vec<Arc<str>>,
}

fn varint_string(r: &mut Reader) -> Result<Arc<str>> {
    let (mut len, mut shift) = (0usize, 0u32);
    loop {
        let b = r.u8()?;
        if shift > 28 {
            return Err(invalid("TPK: string length varint too long"));
        }
        len |= usize::from(b & 0x7F) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    let s = std::str::from_utf8(r.take(len)?).map_err(|_| invalid("TPK: string not UTF-8"))?;
    Ok(Arc::from(s))
}

fn count(r: &mut Reader, what: &str) -> Result<usize> {
    let n = r.i32()?;
    usize::try_from(n).map_err(|_| invalid(format!("TPK: negative {what} count {n}")))
}

impl TypeTreePackage {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes, false);
        if r.u32()? != MAGIC {
            return Err(invalid("not a TPK file"));
        }
        let format = r.u8()?;
        if !(1..=2).contains(&format) {
            return Err(Error::Unsupported(format!("TPK format version {format}")));
        }
        let compression = r.u8()?;
        let data_type = r.u8()?;
        let _reserved = r.u8()?;
        let _reserved2 = r.u32()?;
        let compressed = r.u32()? as usize;
        let uncompressed = r.u32()? as usize;
        if data_type != 0 {
            return Err(Error::Unsupported(format!(
                "TPK data type {data_type} (not type trees)"
            )));
        }
        let packed = r.take(compressed)?;
        let blob: Vec<u8> = match compression {
            0 => packed.to_vec(),
            1 => Lz4
                .decompress(packed, uncompressed)
                .map_err(|e| invalid(format!("TPK LZ4: {e}")))?,
            2 => {
                return Err(Error::Unsupported(
                    "LZMA-compressed TPK: use an uncompressed or LZ4 package".into(),
                ));
            }
            3 => {
                return Err(Error::Unsupported(
                    "Brotli-compressed TPK: use an uncompressed or LZ4 package".into(),
                ));
            }
            c => return Err(invalid(format!("TPK compression type {c}"))),
        };
        if blob.len() != uncompressed {
            return Err(invalid(format!(
                "TPK: {} bytes, header says {uncompressed}",
                blob.len()
            )));
        }
        Self::parse_blob(&blob, format)
    }

    fn parse_blob(blob: &[u8], format: u8) -> Result<Self> {
        let mut r = Reader::new(blob, false);
        let _created = r.i64()?;
        let versions = count(&mut r, "version")?;
        r.take(versions * 8)?;
        let mut classes = BTreeMap::new();
        for _ in 0..count(&mut r, "class")? {
            let id = r.i32()?;
            let mut entries = Vec::new();
            for _ in 0..count(&mut r, "class version")? {
                let version = UnityVersion(r.u64()?);
                let class = if r.u8()? != 0 {
                    let name = r.u16()?;
                    let _base = r.u16()?;
                    let flags = r.u8()?;
                    if flags & HAS_EDITOR_ROOT != 0 {
                        r.u16()?;
                    }
                    let release_root = if flags & HAS_RELEASE_ROOT != 0 {
                        Some(r.u16()?)
                    } else {
                        None
                    };
                    Some(Class {
                        name,
                        flags,
                        release_root,
                    })
                } else {
                    None
                };
                entries.push((version, class));
            }
            classes.insert(id, entries);
        }
        // the common string table (offsets into Unity's built-in strings): not needed for stripped files
        match format {
            1 => {
                let n = count(&mut r, "common string version")?;
                r.take(n * 9)?;
                let n = count(&mut r, "common string index")?;
                r.take(n * 2)?;
            }
            _ => {
                for _ in 0..count(&mut r, "common string version")? {
                    r.u64()?;
                    let n = count(&mut r, "common string entry")?;
                    r.take(n * 4)?;
                }
            }
        }
        let mut nodes = Vec::new();
        for _ in 0..count(&mut r, "node")? {
            let type_name = r.u16()?;
            let name = r.u16()?;
            let byte_size = r.i32()?;
            let version = r.i16()?;
            let _type_flags = r.u8()?;
            let meta_flag = r.u32()?;
            let n = r.u16()?;
            let children = (0..n).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
            nodes.push(PackedNode {
                type_name,
                name,
                byte_size,
                version,
                meta_flag,
                children,
            });
        }
        let strings = (0..count(&mut r, "string")?)
            .map(|_| varint_string(&mut r))
            .collect::<Result<Vec<_>>>()?;
        if r.remaining() != 0 {
            return Err(invalid(format!(
                "TPK: {} bytes after the string buffer",
                r.remaining()
            )));
        }
        let pkg = TypeTreePackage {
            classes,
            nodes,
            strings,
        };
        pkg.check()?;
        Ok(pkg)
    }

    /// Every index in range, so lookups need no checks later.
    fn check(&self) -> Result<()> {
        let (ns, nn) = (self.strings.len(), self.nodes.len());
        for n in &self.nodes {
            if usize::from(n.type_name) >= ns || usize::from(n.name) >= ns {
                return Err(invalid("TPK: node string index out of range"));
            }
            if n.children.iter().any(|&c| usize::from(c) >= nn) {
                return Err(invalid("TPK: child node index out of range"));
            }
        }
        for c in self.classes.values().flatten().filter_map(|(_, c)| c.as_ref()) {
            if usize::from(c.name) >= ns || c.release_root.is_some_and(|r| usize::from(r) >= nn) {
                return Err(invalid("TPK: class index out of range"));
            }
        }
        Ok(())
    }

    fn class(&self, class_id: i32, version: UnityVersion) -> Option<&Class> {
        self.classes
            .get(&class_id)?
            .iter()
            .take_while(|(v, _)| *v <= version)
            .last()
            .and_then(|(_, c)| c.as_ref())
    }

    /// The class's name in that Unity version.
    pub fn class_name(&self, class_id: i32, version: UnityVersion) -> Option<&str> {
        self.class(class_id, version)
            .map(|c| &*self.strings[usize::from(c.name)])
    }

    /// The class's release (player) type tree in that Unity version.
    pub fn type_tree(&self, class_id: i32, version: UnityVersion) -> Result<TypeTree> {
        let class = self.class(class_id, version).ok_or_else(|| {
            invalid(format!(
                "class {class_id} not in the type tree package for {version}"
            ))
        })?;
        let root = class.release_root.ok_or_else(|| {
            invalid(format!(
                "class {class_id} ({}) has no release type tree in {version} (flags {:#x})",
                self.strings[usize::from(class.name)],
                class.flags
            ))
        })?;
        let mut nodes: Vec<Node> = Vec::new();
        // depth first, children in order; each node's children by their index in `nodes`
        fn add(pkg: &TypeTreePackage, id: u16, depth: u32, nodes: &mut Vec<Node>) -> Result<usize> {
            if depth > 64 {
                return Err(invalid("TPK: type tree deeper than 64"));
            }
            let p = &pkg.nodes[usize::from(id)];
            let at = nodes.len();
            nodes.push(Node {
                ty: pkg.strings[usize::from(p.type_name)].clone(),
                name: pkg.strings[usize::from(p.name)].clone(),
                byte_size: p.byte_size,
                version: i32::from(p.version),
                meta_flag: p.meta_flag,
                children: Vec::new(),
            });
            for &c in &p.children {
                let ci = add(pkg, c, depth + 1, nodes)?;
                nodes[at].children.push(ci);
            }
            Ok(at)
        }
        add(self, root, 0, &mut nodes)?;
        TypeTree::new(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(compression: u8, packed: &[u8], expected: u32) -> Vec<u8> {
        let mut bytes = MAGIC.to_le_bytes().to_vec();
        bytes.extend([2, compression, 0, 0]);
        bytes.extend(0u32.to_le_bytes());
        bytes.extend((packed.len() as u32).to_le_bytes());
        bytes.extend(expected.to_le_bytes());
        bytes.extend(packed);
        bytes
    }

    /// An LZ4 block of one literal run (no matches), encoded by hand: the token's high nibble 15, then the rest of the
    /// length in bytes of 255 and a final byte.
    fn literal_lz4(blob: &[u8]) -> Vec<u8> {
        let mut out = vec![0xf0];
        let mut rest = blob.len() - 15;
        while rest >= 255 {
            out.push(255);
            rest -= 255;
        }
        out.push(rest as u8);
        out.extend(blob);
        out
    }

    /// A format-2 package encoded by hand: one listed version, class 1 named by string 0 ("Base") from version 0
    /// with release root node 0, no common strings, one node (type and name string 0, size -1, version 1), one string.
    fn one_class_blob() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(0x0102_0304_0506_0708i64.to_le_bytes()); // created
        b.extend(1i32.to_le_bytes()); // versions
        b.extend(0u64.to_le_bytes());
        b.extend(1i32.to_le_bytes()); // classes
        b.extend(1i32.to_le_bytes()); // class id
        b.extend(1i32.to_le_bytes()); // its versions
        b.extend(0u64.to_le_bytes()); // from version 0
        b.push(1); // present
        b.extend(0u16.to_le_bytes()); // name: string 0
        b.extend(0u16.to_le_bytes()); // base
        b.push(HAS_RELEASE_ROOT);
        b.extend(0u16.to_le_bytes()); // release root: node 0
        b.extend(0i32.to_le_bytes()); // common string versions
        b.extend(1i32.to_le_bytes()); // nodes
        b.extend(0u16.to_le_bytes()); // type name: string 0
        b.extend(0u16.to_le_bytes()); // name: string 0
        b.extend((-1i32).to_le_bytes()); // byte size
        b.extend(1i16.to_le_bytes()); // version
        b.push(0); // type flags
        b.extend(0u32.to_le_bytes()); // meta flag
        b.extend(0u16.to_le_bytes()); // children
        b.extend(1i32.to_le_bytes()); // strings
        b.push(4);
        b.extend(b"Base");
        b
    }

    /// The stored and the LZ4 package decode to the same classes, nodes and strings, the ones encoded; wrong sizes
    /// and a cut block are refused.
    #[test]
    fn literal_lz4_package_matches_uncompressed_and_rejects_bad_sizes() {
        let blob = one_class_blob();
        let n = blob.len() as u32;
        let packed = literal_lz4(&blob);
        let raw = TypeTreePackage::parse(&package(0, &blob, n)).unwrap();
        let compressed = TypeTreePackage::parse(&package(1, &packed, n)).unwrap();
        for p in [&raw, &compressed] {
            assert_eq!(p.class_name(1, UnityVersion(1)), Some("Base"));
            assert_eq!(p.class_name(2, UnityVersion(1)), None);
            assert_eq!(p.classes[&1][0].1.as_ref().unwrap().release_root, Some(0));
            assert_eq!(&*p.strings, &[std::sync::Arc::<str>::from("Base")]);
            let node = &p.nodes[0];
            assert_eq!(
                (node.type_name, node.name, node.byte_size, node.version),
                (0, 0, -1, 1)
            );
            assert!(p.type_tree(1, UnityVersion(1)).is_ok());
        }
        for expected in [0, n - 1, n + 1] {
            assert!(TypeTreePackage::parse(&package(1, &packed, expected)).is_err());
        }
        assert!(TypeTreePackage::parse(&package(1, &packed[..packed.len() - 1], n)).is_err());
        assert!(TypeTreePackage::parse(&package(1, &[], 0)).is_err());
    }
}
