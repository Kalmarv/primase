//! The install's listing: every archive and every file in it, with what each is stored as, for a consumer that
//! browses an install (a catalog) without walking [`Install`]'s tables itself. The files are the ones the install
//! indexes by path ([`Install::file`]): directories are not listed. Nothing is decoded; the numbers are the tables
//! of contents' own.
use std::path::Path;

use crate::read::archive::archive_set::{FileRef, Install};
use crate::read::archive::rpf::{Archive, Encryption, EntryKind};

/// How a listed file is stored in its archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    /// A binary file: `stored` bytes in the archive (0: stored uncompressed, `size` bytes), `size` bytes once read.
    Binary { stored: u32, size: u32, encrypted: bool },
    /// A resource (RSC7): the bytes of its system and graphics pages once inflated.
    Resource { system: u64, graphics: u64 },
}

/// One file of the install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedFile<'a> {
    /// Where the file is: pass it to [`Install::binary`] or [`Install::resource`].
    pub file: FileRef,
    /// The path of the archive that holds it (lowercase, relative to the install, `\`; a nested archive's is its
    /// parent's path to it).
    pub archive: &'a str,
    /// The file's own path: the archive's path, then the file's path in it (lowercase, `\`).
    pub path: &'a str,
    /// Its name as stored.
    pub name: &'a str,
    /// The lowercase extension of its name without the dot (`ydr`, `rpf`); empty when it has none.
    pub extension: &'a str,
    pub storage: Storage,
}

impl ListedFile<'_> {
    /// The bytes the file holds once read: a binary's size, a resource's system and graphics pages.
    pub fn size(&self) -> u64 {
        match self.storage {
            Storage::Binary { size, .. } => u64::from(size),
            Storage::Resource { system, graphics } => system.saturating_add(graphics),
        }
    }

    /// Whether the file is itself an archive (a nested `.rpf`).
    pub fn is_archive(&self) -> bool {
        matches!(self.storage, Storage::Binary { .. }) && self.extension == "rpf"
    }
}

/// One archive of the install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedArchive<'a> {
    /// Its index in [`Install::archives`] (the `archive` of its files' [`FileRef`]s).
    pub index: usize,
    /// Its path (lowercase, relative to the install, `\`; a nested archive's is its parent's path to it).
    pub path: &'a str,
    /// The physical file it lies in, where it starts there and its length.
    pub file: &'a Path,
    pub start: u64,
    pub size: u64,
    pub encryption: Encryption,
    /// How many files it holds directly, and how many of those are archives.
    pub files: usize,
    pub archives: usize,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map_or_else(String::new, |(_, ext)| ext.to_ascii_lowercase())
}

/// The storage of an entry: `None` for a directory.
fn storage(kind: &EntryKind) -> Option<Storage> {
    match kind {
        EntryKind::Directory { .. } => None,
        EntryKind::Binary {
            stored,
            size,
            encrypted,
            ..
        } => Some(Storage::Binary {
            stored: *stored,
            size: *size,
            encrypted: *encrypted,
        }),
        EntryKind::Resource { system, graphics, .. } => Some(Storage::Resource {
            system: system.size(),
            graphics: graphics.size(),
        }),
    }
}

impl Install {
    /// Every archive, the top-level ones and those nested in them, in the install's order.
    pub fn archive_listing(&self) -> impl Iterator<Item = ListedArchive<'_>> {
        self.archives.iter().enumerate().map(|(index, a)| {
            let mut files = 0;
            let mut archives = 0;
            for e in &a.entries {
                if let Some(storage) = storage(&e.kind) {
                    files += 1;
                    let nested = matches!(storage, Storage::Binary { .. }) && extension(&e.name) == "rpf";
                    archives += usize::from(nested);
                }
            }
            ListedArchive {
                index,
                path: &a.path,
                file: &a.file,
                start: a.start,
                size: a.size,
                encryption: a.encryption,
                files,
                archives,
            }
        })
    }

    /// Every file of every archive (directories are not listed), archive by archive in the install's order and in
    /// each archive's table order.
    pub fn listing(&self) -> impl Iterator<Item = ListedFile<'_>> {
        self.archives
            .iter()
            .enumerate()
            .flat_map(|(archive, a): (usize, &Archive)| {
                a.entries.iter().enumerate().filter_map(move |(entry, e)| {
                    let storage = storage(&e.kind)?;
                    // the extension is borrowed from the lowercase path, which ends with the name
                    let extension = e
                        .path
                        .rsplit_once('\\')
                        .map_or(e.path.as_str(), |(_, n)| n)
                        .rsplit_once('.')
                        .map_or("", |(_, ext)| ext);
                    Some(ListedFile {
                        file: FileRef { archive, entry },
                        archive: &a.path,
                        path: &e.path,
                        name: &e.name,
                        extension,
                        storage,
                    })
                })
            })
    }
}
