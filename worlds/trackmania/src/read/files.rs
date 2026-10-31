//! The game's files as its own file system sees them ([`GameFiles`]): the install's `GameData` folder and the packs of
//! its `Packs` folder (decrypted with `packlist.dat`'s keys) over it, every path from the same root
//! (`Stadium\Media\Texture\Image\StadiumRoadD.dds` may be in `Game.pak`, `Stadium.pak` or a loose file).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the packs and GameData files of the install the caller names"
)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gbx_formats::Ident;
use gbx_formats::game::collector::CollectorDesc;
use gbx_formats::pak::{PackList, Pak};

use crate::{Error, Result};

/// The game's files: its packs and its `GameData` folder.
#[derive(Debug)]
pub struct GameFiles {
    /// Each pack by its name (`Stadium`), sorted.
    packs: Vec<(String, Pak)>,
    /// Every loose file under `GameData`, by its lowercased `\`-separated path.
    loose: BTreeMap<String, PathBuf>,
}

/// Where a path of the game's file system is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place<'a> {
    /// In the named pack.
    Pack(&'a str),
    /// A loose file of `GameData`.
    Loose(&'a Path),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

impl GameFiles {
    /// The files of the install at `install`: `Packs\*.pak` with `Packs\packlist.dat`'s keys, and `GameData`.
    pub fn open(install: &Path) -> Result<GameFiles> {
        let packs_dir = install.join("Packs");
        let list_path = packs_dir.join("packlist.dat");
        let list = PackList::parse(&std::fs::read(&list_path).map_err(io(&list_path))?)?;
        let mut packs = Vec::new();
        for entry in std::fs::read_dir(&packs_dir).map_err(io(&packs_dir))? {
            let path = entry.map_err(io(&packs_dir))?.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let key = list.key(&name).ok_or_else(|| {
                Error::Format(format!("{}: packlist.dat has no key for it", path.display()))
            })?;
            packs.push((
                name,
                Pak::parse(std::fs::read(&path).map_err(io(&path))?, &key.key)?,
            ));
        }
        packs.sort_by(|a, b| a.0.cmp(&b.0));
        let game_data = install.join("GameData");
        let mut loose = BTreeMap::new();
        let mut stack = vec![game_data.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).map_err(io(&dir))? {
                let entry = entry.map_err(io(&dir))?;
                let path = entry.path();
                if entry.file_type().map_err(io(&path))?.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(&game_data) {
                    loose.insert(key(&rel.to_string_lossy()), path.clone());
                }
            }
        }
        Ok(GameFiles { packs, loose })
    }

    /// Where `path` is: the first pack (by name) holding it, else `GameData`.
    pub fn find(&self, path: &str) -> Option<Place<'_>> {
        self.packs
            .iter()
            .find(|(_, p)| p.find(path).is_some())
            .map(|(name, _)| Place::Pack(name))
            .or_else(|| self.loose.get(&key(path)).map(|p| Place::Loose(p)))
    }

    /// The bytes of `path` (a pack file decrypted and decompressed), `None` when the game has no such file.
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        for (_, pak) in &self.packs {
            if let Some(file) = pak.find(path) {
                return Ok(Some(pak.read(file)?));
            }
        }
        match self.loose.get(&key(path)) {
            Some(p) => Ok(Some(std::fs::read(p).map_err(io(p))?)),
            None => Ok(None),
        }
    }

    /// Every collector (a block model, a decoration) the game's files hold, with its path: the ident its file's desc
    /// (header chunk `0x0301A003`) names, read from the file's head. A pack file stored under a name hash is left out,
    /// as no path names it; a pack's file is listed before a loose one.
    pub fn collectors(&self) -> Result<Vec<(Ident, String)>> {
        let mut out = Vec::new();
        for (_, pak) in &self.packs {
            for file in pak.files.iter().filter(|f| !hashed(&f.name)) {
                if let Some(header) = pak.header(file)?
                    && let Some(desc) = CollectorDesc::read(&header.header_chunks())?
                {
                    out.push((desc.ident, pak.path(file)));
                }
            }
        }
        for (key, path) in &self.loose {
            if !key.ends_with(".gbx") {
                continue;
            }
            let bytes = std::fs::read(path).map_err(io(path))?;
            if !bytes.starts_with(b"GBX") {
                continue;
            }
            if let Some(desc) = CollectorDesc::read(&gbx_formats::Gbx::parse(&bytes)?.header_chunks)? {
                out.push((desc.ident, key.clone()));
            }
        }
        Ok(out)
    }

    /// The paths every pack stores under their real names (not a 34-digit name hash), each with its pack.
    pub fn pack_paths(&self) -> impl Iterator<Item = (&str, String)> {
        self.packs.iter().flat_map(|(name, pak)| {
            pak.files
                .iter()
                .filter(|f| !hashed(&f.name))
                .map(move |f| (name.as_str(), pak.path(f)))
        })
    }

    /// Every loose path of `GameData` (lowercased).
    pub fn loose_paths(&self) -> impl Iterator<Item = &str> {
        self.loose.keys().map(String::as_str)
    }
}

/// A pack file's stored name is a name hash (34 hex digits), not its real name.
fn hashed(name: &str) -> bool {
    name.len() == 34 && name.bytes().all(|b| b.is_ascii_hexdigit())
}
