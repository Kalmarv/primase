//! A data folder (a Patch folder Studio writes, or the game's `Data`): its TOCs, its CAS archives and the install
//! chunks of its `layout.toc` that name the archives' directories; reading the files a bundle region points at.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use primase_compress::Decompressor;
use primase_compress::cas::Cas;
use primase_compress::oodle::Oodle;

use crate::postcompile::fb::bundle::{self, FileRef, Manifest};
use crate::postcompile::fb::{db, toc::Loc};
use crate::postcompile::{Error, Result};

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
    /// in the region, else the region file that is a manifest (raw or CAS-framed) whose asset count matches.
    pub fn manifest(&self, files: &[FileRef], inline: &[u8]) -> Result<Option<(Manifest, usize, Vec<u8>)>> {
        if !inline.is_empty() {
            return Ok(Some((bundle::read_manifest(inline)?, 0, inline.to_vec())));
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
                let Ok(blocks) = primase_compress::cas::blocks(&raw) else {
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
}
