//! Several forges read as one: a ClassID is looked up in each, in the order the caller gives them, and the first
//! that holds it answers. Which copy wins where a base and its patch both hold an id is the caller's choice
//! (the Riders Republic notes find the patch's copy differing only in some LODSelectors' flags, TextureMapSpecs and a
//! few others, and lead to "the patch wins"; that is not established from the game), so nothing here orders them.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the forge files the caller names"
)]

use std::fs::File;
use std::path::Path;

use crate::data::{Resource, read_entry};
use crate::forge::Forge;

/// A decoded resource's largest size the archive accepts (the forge's own sizes are checked).
pub const MAX_RECORD: usize = 64 << 20;

/// Why an archive can't be opened or an entry read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Forge(#[from] crate::forge::Error),
    #[error("no forge {0} in the archive")]
    NoForge(usize),
    #[error("forge {forge}, entry {entry}: {source}")]
    Data {
        forge: usize,
        entry: usize,
        #[source]
        source: crate::data::Error,
    },
}

/// Forges searched in a stated order.
#[derive(Debug)]
pub struct Archive {
    forges: Vec<(Forge, File)>,
}

/// An entry of one of an archive's forges: the forge's position in the order given, and the entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct At {
    pub forge: usize,
    pub entry: usize,
}

impl Archive {
    /// The forges at `paths`, searched in that order.
    pub fn open(paths: &[impl AsRef<Path>]) -> Result<Archive, Error> {
        let forges = paths
            .iter()
            .map(|p| Ok((Forge::open(p)?, File::open(p)?)))
            .collect::<Result<_, Error>>()?;
        Ok(Archive { forges })
    }

    /// The first forge, in the order given, holding `id`, and the entry there.
    pub fn find(&self, id: u64) -> Option<At> {
        self.forges
            .iter()
            .enumerate()
            .find_map(|(forge, (f, _))| f.find(id).map(|entry| At { forge, entry }))
    }

    /// The forge at position `forge`.
    pub fn forge(&self, forge: usize) -> Option<&Forge> {
        self.forges.get(forge).map(|(f, _)| f)
    }

    /// The resource at `at`.
    pub fn resource(&mut self, at: At) -> Result<Resource, Error> {
        let (forge, file) = self.forges.get_mut(at.forge).ok_or(Error::NoForge(at.forge))?;
        let payload = forge.payload(file, at.entry)?;
        let data = |source| Error::Data {
            forge: at.forge,
            entry: at.entry,
            source,
        };
        let block = read_entry(&payload, MAX_RECORD).map_err(data)?;
        Resource::parse(&block.resource.data).map_err(data)
    }
}
