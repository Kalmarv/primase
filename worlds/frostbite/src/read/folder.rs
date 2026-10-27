//! A data folder (a Patch folder Studio writes, or the game's `Data`): its TOCs, its CAS archives and the install
//! chunks of its `layout.toc` that name the archives' directories; reading the files a bundle region points at.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the container files of a data folder the caller names"
)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::Cas;
use primase_foundations::compress::Decompressor;
use primase_foundations::compress::oodle::Oodle;

use crate::bundle::{self, FileRef, Manifest};
use crate::db;
use crate::toc::{self, Loc, Toc};
use crate::{Error, Result};

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug)]
pub struct Folder {
    pub root: PathBuf,
    /// TOC paths relative to the folder, `/`-separated, sorted.
    pub tocs: Vec<String>,
    /// (lowercase directory relative to `Win32`, archive number) -> path.
    archives: BTreeMap<(String, u32), PathBuf>,
    /// install chunk id -> lowercase directory relative to `Win32`.
    chunk_dirs: BTreeMap<u32, String>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(io(dir))?
        .collect::<Result<_, _>>()
        .map_err(io(dir))?;
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() { walk(&p, out)? } else { out.push(p) }
    }
    Ok(())
}

impl Folder {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let win = root.join("Win32");
        let mut files = Vec::new();
        walk(&win, &mut files)?;
        let mut tocs = Vec::new();
        let mut archives = BTreeMap::new();
        for p in files {
            let rel = |p: &Path, base: &Path| {
                p.strip_prefix(base)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default()
            };
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.ends_with(".toc") {
                tocs.push(rel(&p, &root));
            } else if let Some(stem) = name.strip_suffix(".cas") {
                let digits: String = stem
                    .chars()
                    .rev()
                    .take_while(char::is_ascii_digit)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if let Ok(num) = digits.parse::<u32>() {
                    let dir = p
                        .parent()
                        .map(|d| rel(d, &win))
                        .unwrap_or_default()
                        .to_lowercase();
                    archives.insert((dir, num), p.clone());
                }
            }
        }
        tocs.sort();
        let layout_path = root.join("layout.toc");
        let layout = fs::read(&layout_path).map_err(io(&layout_path))?;
        let (_, layout, _) = db::read_file(&layout)?;
        let mut chunk_dirs = BTreeMap::new();
        if let Some(chunks) = layout
            .field("installManifest")
            .and_then(|m| m.field("installChunks"))
        {
            for e in &chunks.children {
                if let (Some(name), Some(index)) = (e.field("name"), e.field("persistentIndex"))
                    && name.ty == 7
                    && let Some(i) = index.u32()
                {
                    chunk_dirs.insert(i, name.text().to_lowercase());
                }
            }
        }
        Ok(Self {
            root,
            tocs,
            archives,
            chunk_dirs,
        })
    }

    pub fn toc_path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The TOC at `rel` (relative to the folder, as in [`Folder::tocs`]), read and parsed.
    pub fn toc(&self, rel: &str) -> Result<Toc> {
        let path = self.toc_path(rel);
        toc::read(&fs::read(&path).map_err(io(&path))?)
    }

    pub fn archive(&self, loc: Loc) -> Option<&Path> {
        let dir = self.chunk_dirs.get(&loc.chunk)?;
        self.archives
            .get(&(dir.clone(), u32::from(loc.archive)))
            .map(PathBuf::as_path)
    }

    /// The next free archive number in `loc`'s directory and its path (for a writer appending new data).
    pub fn new_archive(&self, loc: Loc) -> Result<(u16, PathBuf)> {
        let dir = self
            .chunk_dirs
            .get(&loc.chunk)
            .ok_or_else(|| Error::Missing(format!("install chunk {}", loc.chunk)))?;
        let next = self
            .archives
            .keys()
            .filter(|(d, _)| d == dir)
            .map(|(_, n)| n + 1)
            .max()
            .unwrap_or(1);
        let mut p = self.root.join("Win32");
        for part in dir.split('/') {
            p.push(part);
        }
        p.push(format!("cas_{next:02}.cas"));
        let next = u16::try_from(next).map_err(|_| Error::Missing("archive number overflow".into()))?;
        Ok((next, p))
    }

    pub fn read(&self, (loc, off, size): FileRef) -> Result<Vec<u8>> {
        let path = self
            .archive(loc)
            .ok_or_else(|| Error::Missing(format!("{}: no archive for {loc:?}", self.root.display())))?;
        let mut f = fs::File::open(path).map_err(io(path))?;
        let len = f.metadata().map_err(io(path))?.len();
        if u64::from(off) + u64::from(size) > len {
            return Err(Error::Missing(format!(
                "{}: {size} bytes at {off} lie past the end ({len} bytes)",
                path.display()
            )));
        }
        f.seek(SeekFrom::Start(u64::from(off))).map_err(io(path))?;
        let mut buf = vec![0; size as usize];
        f.read_exact(&mut buf).map_err(io(path))?;
        Ok(buf)
    }
}

/// A Patch folder with the game's data behind it: files a Patch TOC places in the base game are read there, and CAS
/// payloads are decoded with the game's own Oodle when they use it.
#[derive(Debug)]
pub struct Source<'a> {
    pub folder: &'a Folder,
    pub base: Option<&'a Folder>,
    pub oodle: Option<&'a Oodle>,
}

impl Source<'_> {
    pub fn raw(&self, f: FileRef) -> Result<Vec<u8>> {
        if f.0.patch {
            self.folder.read(f)
        } else {
            self.base
                .ok_or_else(|| Error::Missing(format!("base game data is needed for {:?}", f.0)))?
                .read(f)
        }
    }

    /// A CAS-framed file decoded to exactly `expected` bytes.
    pub fn payload(&self, f: FileRef, expected: usize) -> Result<Vec<u8>> {
        let raw = self.raw(f)?;
        Ok(Cas { oodle: self.oodle }.decompress(&raw, expected)?)
    }

    /// The bundle's manifest and the index of its first asset file, read the way ReSkate's merge reads it: inline
    /// in the region (its assets then no more than the region's files: [`fits`]), else the region file that is a
    /// manifest (raw or CAS-framed) whose asset count matches.
    pub fn manifest(&self, files: &[FileRef], inline: &[u8]) -> Result<Option<(Manifest, usize, Vec<u8>)>> {
        if !inline.is_empty() {
            let m = bundle::read_manifest(inline)?;
            fits(&m, files.len())?;
            return Ok(Some((m, 0, inline.to_vec())));
        }
        let expected = files.len().saturating_sub(1);
        for &f in files {
            if f.2 > 32 << 20 || (!f.0.patch && self.base.is_none()) {
                continue;
            }
            let raw = self.raw(f)?;
            let m = if bundle::is_manifest(&raw) {
                raw
            } else {
                let Ok(blocks) = crate::cas::blocks(&raw) else {
                    continue;
                };
                let n: usize = blocks.iter().map(|b| b.decoded_size).sum();
                match (Cas { oodle: self.oodle }).decompress(&raw, n) {
                    Ok(d) if bundle::is_manifest(&d) => d,
                    _ => continue,
                }
            };
            let b = bundle::read_manifest(&m)?;
            if b.count() == expected {
                return Ok(Some((b, 1, m)));
            }
        }
        Ok(None)
    }

    /// The EBX document named `name` (ASCII case-insensitive, as bundle manifests name them: `characters/.../x`) in
    /// the first bundle of the TOC at `toc` (relative to the folder, as in [`Folder::tocs`]) that holds it, read; with
    /// the bundle's name. An error when no bundle of that TOC holds it.
    pub fn ebx(&self, toc: &str, name: &str) -> Result<(String, crate::ebx::Document)> {
        let t = self.folder.toc(toc)?;
        for (bundle_name, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = self.manifest(&files, &inline)? else {
                continue;
            };
            if let Some(i) = m.ebx.iter().position(|e| e.name.eq_ignore_ascii_case(name)) {
                let bytes = self.payload(files[first + i], m.ebx[i].size as usize)?;
                return Ok((bundle_name.clone(), crate::ebx::read(&bytes)?));
            }
        }
        Err(Error::Missing(format!("no bundle of {toc} holds the EBX {name}")))
    }

    /// Every EBX document of the bundle named `bundle_name` in the TOC at `toc`, by its file GUID: its name and where
    /// it lies (read it with [`Source::read_ebx`]). Each document's payload is read for its GUID, not parsed.
    pub fn ebx_index(&self, toc: &str, bundle_name: &str) -> Result<BTreeMap<[u8; 16], EbxEntry>> {
        let t = self.folder.toc(toc)?;
        let (_, region) = t
            .bundles
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(bundle_name))
            .ok_or_else(|| Error::Missing(format!("{toc} has no bundle {bundle_name}")))?;
        let (files, inline) = bundle::read_region(region)?;
        let (m, first, _) = self
            .manifest(&files, &inline)?
            .ok_or_else(|| Error::Missing(format!("{bundle_name} has no manifest")))?;
        let mut out = BTreeMap::new();
        for (i, e) in m.ebx.iter().enumerate() {
            let entry = EbxEntry {
                name: e.name.clone(),
                file: files[first + i],
                size: e.size as usize,
            };
            let guid = crate::ebx::read_file_guid(&self.payload(entry.file, entry.size)?)?;
            out.insert(guid, entry);
        }
        Ok(out)
    }

    /// The resource with id `rid` of the bundle named `bundle_name` in the TOC at `toc`.
    pub fn resource(&self, toc: &str, bundle_name: &str, rid: u64) -> Result<Resource> {
        let t = self.folder.toc(toc)?;
        let (_, region) = t
            .bundles
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(bundle_name))
            .ok_or_else(|| Error::Missing(format!("{toc} has no bundle {bundle_name}")))?;
        let (files, inline) = bundle::read_region(region)?;
        let (m, first, _) = self
            .manifest(&files, &inline)?
            .ok_or_else(|| Error::Missing(format!("{bundle_name} has no manifest")))?;
        let (i, a) = m
            .res
            .iter()
            .enumerate()
            .find(|(_, a)| a.rid == rid)
            .ok_or_else(|| Error::Missing(format!("{bundle_name} has no resource {rid:#x}")))?;
        Ok(Resource {
            name: a.name.clone(),
            rtype: a.rtype,
            meta: a.rmeta,
            bytes: self.payload(files[first + m.ebx.len() + i], a.size as usize)?,
        })
    }

    /// The EBX document at `entry`, read.
    pub fn read_ebx(&self, entry: &EbxEntry) -> Result<crate::ebx::Document> {
        crate::ebx::read(&self.payload(entry.file, entry.size)?)
    }
}

/// A resource of a bundle ([`Source::resource`]): its name, type and metadata as its manifest states them, and its
/// bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub rtype: u32,
    pub meta: [u8; 16],
    pub bytes: Vec<u8>,
}

/// Where an EBX document of a bundle lies ([`Source::ebx_index`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EbxEntry {
    /// Its name in the bundle's manifest.
    pub name: String,
    pub file: FileRef,
    /// Its payload's size.
    pub size: usize,
}

/// An inline manifest's assets are the region's files from the first: an error, naming both counts, when it lists more
/// than the region holds (the entries' file references would lie past the table).
fn fits(m: &Manifest, files: usize) -> Result<()> {
    if m.count() > files {
        return Err(crate::bad(
            "bundle manifest",
            0,
            format!("{} assets for a region of {files} files", m.count()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    fn an_inline_manifest_with_more_assets_than_files_is_refused() {
        let asset = |kind| bundle::Asset {
            kind,
            name: String::new(),
            sha1: [0; 20],
            size: 0,
            rtype: 0,
            rmeta: [0; 16],
            rid: 0,
            guid: [0; 16],
            loff: 0,
            lsize: 0,
        };
        let m = Manifest {
            ebx: vec![asset(bundle::Kind::Ebx), asset(bundle::Kind::Ebx)],
            res: vec![asset(bundle::Kind::Res)],
            chunks: Vec::new(),
            meta: Vec::new(),
            big_endian: false,
        };
        assert!(fits(&m, 3).is_ok());
        let e = fits(&m, 2).unwrap_err().to_string();
        assert!(e.contains("3 assets for a region of 2 files"), "{e}");
    }
}
