//! A track opened: its four GeoChunks' directories, the chunk maps and contents lists that name their entries, and the
//! index the world files resolve through ([`forzatech_formats::track::Track`]); and its `bin.zip` ([`Archive`]), the
//! materials and shaders. Entry bytes are read on demand; nothing is chosen here: the caller names the install folder
//! and the track.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the GeoChunks and the files of the track folder the caller names"
)]
use crate::{Error, Result};
use forzatech_formats::minizip::{MiniZip, parse_chunk_map, parse_contents};
use forzatech_formats::track::{Location, Track as Index};
use forzatech_formats::zip::Zip;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The GeoChunks a track has: `GeoChunk<n>.minizip` with `ChunkMap<n>.dat` and `ChunkContentsMiniZip<n>.txt`.
pub const CHUNKS: usize = 4;

/// One GeoChunk: its directory, an open handle, and each entry's name as its contents list writes it.
struct Chunk {
    zip: MiniZip,
    file: Mutex<BufReader<File>>,
    names: Vec<String>,
    /// Each entry's name's path within the build's `zipcache\pc\` folder, lower case, when it has one.
    paths: Vec<Option<String>>,
}

/// An opened track (`media\Tracks\<name>`).
pub struct Track {
    folder: PathBuf,
    chunks: Vec<Chunk>,
    index: Index,
}

impl std::fmt::Debug for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Track")
            .field("folder", &self.folder)
            .field("entries", &self.index.len())
            .field("models", &self.index.model_count())
            .finish_non_exhaustive()
    }
}

impl Track {
    /// The track in `folder` (the install's `media\Tracks\<name>`): every GeoChunk's directory read and checked, and
    /// the index of their entries built.
    pub fn open(folder: impl Into<PathBuf>) -> Result<Track> {
        let folder = folder.into();
        let mut chunks = Vec::with_capacity(CHUNKS);
        let mut maps = Vec::with_capacity(CHUNKS);
        let mut texts = Vec::with_capacity(CHUNKS);
        for n in 0..CHUNKS {
            let path = folder.join(format!("GeoChunk{n}.minizip"));
            let mut file = BufReader::with_capacity(1 << 20, File::open(&path).map_err(|e| io(&path, e))?);
            let zip = MiniZip::read(&mut file)?;
            let count = u32::try_from(zip.entries().len())
                .map_err(|_| Error::Track(format!("GeoChunk{n} has more entries than a u32 counts")))?;
            let map_path = folder.join(format!("ChunkMap{n}.dat"));
            maps.push(parse_chunk_map(&read(&map_path)?, count)?);
            let text_path = folder.join(format!("ChunkContentsMiniZip{n}.txt"));
            let text = String::from_utf8(read(&text_path)?)
                .map_err(|_| Error::Track(format!("{} is not UTF-8", text_path.display())))?;
            texts.push(text);
            chunks.push((zip, file));
        }
        let lines = texts
            .iter()
            .map(|t| parse_contents(t))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (n, (l, (zip, _))) in lines.iter().zip(&chunks).enumerate() {
            if l.len() != zip.entries().len() {
                return Err(Error::Track(format!(
                    "GeoChunk{n} has {} entries and its contents list {} lines",
                    zip.entries().len(),
                    l.len()
                )));
            }
        }
        let index = Index::new(maps.iter().zip(&lines).map(|(m, l)| (&m[..], &l[..])))?;
        let chunks = chunks
            .into_iter()
            .zip(&lines)
            .map(|((zip, file), lines)| Chunk {
                zip,
                file: Mutex::new(file),
                names: lines.iter().map(|l| l.name.to_owned()).collect(),
                paths: lines
                    .iter()
                    .map(|l| l.zipcache_path().map(str::to_ascii_lowercase))
                    .collect(),
            })
            .collect();
        Ok(Track {
            folder,
            chunks,
            index,
        })
    }

    /// The track's folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The index the world files' references resolve through.
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// An entry's name as its contents list writes it.
    pub fn name(&self, at: Location) -> Option<&str> {
        self.chunks.get(at.chunk)?.names.get(at.entry).map(String::as_str)
    }

    /// An entry's path within the build's `zipcache\pc\` folder, lower case, when its name has one.
    pub fn path(&self, at: Location) -> Option<&str> {
        self.chunks.get(at.chunk)?.paths.get(at.entry)?.as_deref()
    }

    /// Every entry with a zipcache path, with that path (lower case), chunk by chunk in entry order.
    pub fn paths(&self) -> impl Iterator<Item = (Location, &str)> + '_ {
        self.chunks.iter().enumerate().flat_map(|(chunk, c)| {
            c.paths
                .iter()
                .enumerate()
                .filter_map(move |(entry, p)| Some((Location { chunk, entry }, p.as_deref()?)))
        })
    }

    /// An entry's bytes, decoded.
    pub fn read(&self, at: Location) -> Result<Vec<u8>> {
        let chunk = self
            .chunks
            .get(at.chunk)
            .ok_or_else(|| Error::Track(format!("no GeoChunk{}", at.chunk)))?;
        let mut file = chunk
            .file
            .lock()
            .map_err(|_| Error::Track(format!("GeoChunk{}'s handle was poisoned", at.chunk)))?;
        Ok(chunk.zip.read_entry(&mut *file, at.entry)?)
    }

    /// The track's `bin.zip`.
    pub fn bin(&self) -> Result<Archive> {
        Archive::open(&self.folder.join("bin.zip"))
    }

    /// The install's texture library: `media\_library\Textures.zip` and each zip of `media\_library\TexturesPG`
    /// (sorted by name), with the folder its entries' game paths are under (`_library\texturespg\defaulttextures`: the
    /// zip's path in `media`, less `.zip`, lower case). The media folder is the track folder's grandparent.
    pub fn texture_library(&self) -> Result<Vec<(String, Archive)>> {
        let media = self
            .folder
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| Error::Track(format!("{}: not under media\\Tracks", self.folder.display())))?;
        let library = media.join("_library");
        let mut zips = vec![("_library\\textures".to_owned(), library.join("Textures.zip"))];
        let pg = library.join("TexturesPG");
        let mut names = Vec::new();
        for e in std::fs::read_dir(&pg).map_err(|e| io(&pg, e))? {
            let path = e.map_err(|e| io(&pg, e))?.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip"))
                && let Some(stem) = path.file_stem().and_then(|n| n.to_str())
            {
                names.push(stem.to_owned());
            }
        }
        names.sort();
        for n in names {
            zips.push((
                format!("_library\\texturespg\\{}", n.to_ascii_lowercase()),
                pg.join(format!("{n}.zip")),
            ));
        }
        zips.into_iter()
            .map(|(folder, path)| Ok((folder, Archive::open(&path)?)))
            .collect()
    }

    /// A file of the track's folder, by its path relative to it (`Ribbon_00/brio_00.pvs`).
    pub fn file(&self, relative: &str) -> Result<Vec<u8>> {
        read(&self.folder.join(relative))
    }

    /// The files of a folder of the track's (relative to it) with an extension (ASCII case ignored), sorted by name:
    /// each file's name and bytes.
    pub fn files_in(&self, relative: &str, extension: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let dir = self.folder.join(relative);
        let mut names = Vec::new();
        for e in std::fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
            let path = e.map_err(|e| io(&dir, e))?.path();
            if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(extension))
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        names
            .into_iter()
            .map(|n| Ok((n.clone(), read(&dir.join(&n))?)))
            .collect()
    }
}

/// An opened zip: its directory, an open handle, and its entries by name.
pub struct Archive {
    path: PathBuf,
    zip: Zip,
    file: Mutex<BufReader<File>>,
    /// Each entry's name, `/`-separated and lower case, to its index.
    by_name: HashMap<String, usize>,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("entries", &self.zip.entries().len())
            .finish_non_exhaustive()
    }
}

impl Archive {
    /// The zip at `path`, its directory read; two entries whose names differ only in ASCII case are refused.
    pub fn open(path: &Path) -> Result<Archive> {
        let mut file = BufReader::new(File::open(path).map_err(|e| io(path, e))?);
        let zip = Zip::read(&mut file)?;
        let mut by_name = HashMap::with_capacity(zip.entries().len());
        for (k, e) in zip.entries().iter().enumerate() {
            if by_name.insert(e.name.to_ascii_lowercase(), k).is_some() {
                return Err(Error::Track(format!(
                    "{}: two entries named {:?} but for case",
                    path.display(),
                    e.name
                )));
            }
        }
        Ok(Archive {
            path: path.to_path_buf(),
            zip,
            file: Mutex::new(file),
            by_name,
        })
    }

    /// The zip's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Each entry's name, `/`-separated and lower case, in no order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// The entry named `name` (`\` or `/` separated, ASCII case ignored), decoded; `None` when the zip has none.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(&k) = self.by_name.get(&name.replace('\\', "/").to_ascii_lowercase()) else {
            return Ok(None);
        };
        let stored = {
            let mut file = self
                .file
                .lock()
                .map_err(|_| Error::Track(format!("{}'s handle was poisoned", self.path.display())))?;
            self.zip.stored(&mut *file, k)?
        };
        Ok(Some(self.zip.decode(k, &stored)?))
    }
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| io(path, e))
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::Io(format!("{}: {e}", path.display()))
}
