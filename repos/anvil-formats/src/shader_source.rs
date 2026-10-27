//! A shader program of an install, by name: the DXBC container of one stage's permutation of one MaterialTemplate.
//! The engine keeps its programs inside its MaterialTemplates (a template's body lists its ShaderPermutations, each a
//! ShaderKey and a program: [`template::permutations`]), so "the pixel program of template 106683 for key 0485bb" is
//! the template's entry in a forge of the install, then the permutation of that stage and key.
//!
//! A template is named as the tools that dump them do: the entry's position in its forge's tables
//! (`t106683`, in `DataPC.forge` unless a forge is stated: `DataPC_MAD_USNAT:t61916`), the key in hex and the stage:
//! `t106683-0485bb.ps.dxbc`, optionally under a directory `ps106683/`. [`ShaderName`] parses and prints that form.
//! The key is the engine's ShaderKey as a template's tables hold it ([`crate::template::ShaderTables::pixel`]), so a
//! name round-trips with a material's tables. Nothing is decoded beyond the template body; the container is returned as
//! stored.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the install's forge files"
)]

use std::fmt;
use std::path::{Path, PathBuf};

use crate::archive::{self, Archive, At};
use crate::material::MATERIAL_TEMPLATE;
use crate::template::{self, PermutationError, Stage};

/// The forge files of a Riders Republic install, base before patch, in the order they are searched.
pub const FORGES: [&str; 6] = [
    "DataPC.forge",
    "DataPC_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_WhiteRoom.forge",
    "DataPC_MAD_WhiteRoom_patch_01.forge",
];

/// The forge a name without one means.
pub const DEFAULT_FORGE: &str = "DataPC.forge";

/// Why a shader can't be opened by name.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Archive(#[from] archive::Error),
    #[error("{}: none of the install's forge files ({}) is there", .0.display(), FORGES.join(", "))]
    NoForges(PathBuf),
    #[error("forge {forge} is not among the install's: {}", .opened.join(", "))]
    NoForge { forge: String, opened: Vec<String> },
    #[error("entry {entry} of {forge} {what}")]
    NotATemplate {
        forge: String,
        entry: usize,
        what: &'static str,
    },
    #[error("template {name} (entry {entry} of {forge}): {source}")]
    Permutations {
        forge: String,
        entry: usize,
        name: String,
        #[source]
        source: PermutationError,
    },
    #[error("template {name} (entry {entry} of {forge}) has no {stage} permutation with key {key:06x}")]
    NoPermutation {
        forge: String,
        entry: usize,
        name: String,
        stage: &'static str,
        key: u32,
    },
    #[error("shader name {0:?}: {1}")]
    Name(String, &'static str),
}

/// One stage's permutation of one template: where the template is, and the stage and ShaderKey.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderName {
    /// The forge file the template's entry is in.
    pub forge: String,
    /// The template's entry: its position in the forge's tables.
    pub template: usize,
    pub stage: Stage,
    pub key: u32,
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Pixel => "ps",
        Stage::Vertex => "vs",
    }
}

impl fmt::Display for ShaderName {
    /// `t106683-0485bb.ps.dxbc`, with `<forge>:` before it for a forge other than [`DEFAULT_FORGE`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.forge != DEFAULT_FORGE {
            write!(f, "{}:", self.forge.trim_end_matches(".forge"))?;
        }
        write!(
            f,
            "t{}-{:06x}.{}.dxbc",
            self.template,
            self.key,
            stage_name(self.stage)
        )
    }
}

impl ShaderName {
    /// Read a name: `[<forge>:][<stage><template>/]t<template>-<key hex>.<ps|vs>[.dxbc]`. A directory part, if there,
    /// must agree with the file's stage and template; a forge is its file name with or without `.forge`.
    pub fn parse(text: &str) -> Result<ShaderName, Error> {
        let bad = |why| Error::Name(text.to_owned(), why);
        let (forge, rest) = match text.rsplit_once(':') {
            Some((forge, rest)) if !forge.is_empty() => (
                if forge.ends_with(".forge") {
                    forge.to_owned()
                } else {
                    format!("{forge}.forge")
                },
                rest,
            ),
            _ => (DEFAULT_FORGE.to_owned(), text),
        };
        let (directory, file) = match rest.rsplit_once(['/', '\\']) {
            Some((d, f)) => (Some(d.rsplit(['/', '\\']).next().unwrap_or(d)), f),
            None => (None, rest),
        };
        let file = file.strip_suffix(".dxbc").unwrap_or(file);
        let (name, stage) = file
            .rsplit_once('.')
            .ok_or_else(|| bad("expected t<template>-<key>.<ps|vs>"))?;
        let stage = match stage {
            "ps" => Stage::Pixel,
            "vs" => Stage::Vertex,
            _ => return Err(bad("the stage is ps or vs")),
        };
        let (template, key) = name
            .strip_prefix('t')
            .and_then(|n| n.split_once('-'))
            .ok_or_else(|| bad("expected t<template>-<key>"))?;
        let template: usize = template
            .parse()
            .map_err(|_| bad("the template is a decimal entry number"))?;
        let key = u32::from_str_radix(key, 16).map_err(|_| bad("the key is hexadecimal"))?;
        if let Some(directory) = directory
            && directory != format!("{}{template}", stage_name(stage))
        {
            return Err(bad("the directory is <stage><template>, as the file says"));
        }
        Ok(ShaderName {
            forge,
            template,
            stage,
            key,
        })
    }
}

/// The forges of an install, opened for shaders by name.
#[derive(Debug)]
pub struct ShaderSource {
    archive: Archive,
    forges: Vec<String>,
    /// The template read last: (forge position, entry, its name, its body).
    last: Option<(usize, usize, String, Vec<u8>)>,
}

impl ShaderSource {
    /// The install's forge files that are there ([`FORGES`]), in that order.
    pub fn open(install: impl AsRef<Path>) -> Result<ShaderSource, Error> {
        let dir = install.as_ref();
        let forges: Vec<String> = FORGES
            .iter()
            .filter(|f| dir.join(f).is_file())
            .map(|f| (*f).to_owned())
            .collect();
        if forges.is_empty() {
            return Err(Error::NoForges(dir.to_owned()));
        }
        let paths: Vec<PathBuf> = forges.iter().map(|f| dir.join(f)).collect();
        Ok(ShaderSource {
            archive: Archive::open(&paths)?,
            forges,
            last: None,
        })
    }

    /// The forge files opened.
    pub fn forges(&self) -> &[String] {
        &self.forges
    }

    fn forge_position(&self, forge: &str) -> Result<usize, Error> {
        self.forges
            .iter()
            .position(|f| f == forge)
            .ok_or_else(|| Error::NoForge {
                forge: forge.to_owned(),
                opened: self.forges.clone(),
            })
    }

    /// The MaterialTemplates of `forge`: entry and name.
    pub fn templates(&self, forge: &str) -> Result<Vec<(usize, &str)>, Error> {
        let at = self.forge_position(forge)?;
        let f = self.archive.forge(at).ok_or(archive::Error::NoForge(at))?;
        Ok(f.entries()
            .enumerate()
            .filter(|(_, (_, n))| n.type_id == MATERIAL_TEMPLATE)
            .map(|(entry, (_, n))| (entry, n.name.as_str()))
            .collect())
    }

    /// The body of template `entry` of `forge`, kept until another is read.
    fn body(&mut self, forge: &str, entry: usize) -> Result<(&str, &[u8]), Error> {
        let at = self.forge_position(forge)?;
        let hit = matches!(&self.last, Some((f, e, ..)) if *f == at && *e == entry);
        if !hit {
            let (name, type_id) = {
                let f = self.archive.forge(at).ok_or(archive::Error::NoForge(at))?;
                let n = f.names.get(entry).ok_or_else(|| Error::NotATemplate {
                    forge: forge.to_owned(),
                    entry,
                    what: "is past the forge's tables",
                })?;
                (n.name.clone(), n.type_id)
            };
            if type_id != MATERIAL_TEMPLATE {
                return Err(Error::NotATemplate {
                    forge: forge.to_owned(),
                    entry,
                    what: "is not a MaterialTemplate",
                });
            }
            let body = self.archive.resource(At { forge: at, entry })?.body;
            self.last = Some((at, entry, name, body));
        }
        let (_, _, name, body) = self.last.as_ref().ok_or(archive::Error::NoForge(at))?;
        Ok((name.as_str(), body.as_slice()))
    }

    /// The ShaderKeys of `stage` that template `entry` of `forge` has, sorted.
    pub fn keys(&mut self, forge: &str, entry: usize, stage: Stage) -> Result<Vec<u32>, Error> {
        let (name, body) = self.body(forge, entry)?;
        let name = name.to_owned();
        let mut keys: Vec<u32> = template::permutations(body)
            .map_err(|source| Error::Permutations {
                forge: forge.to_owned(),
                entry,
                name,
                source,
            })?
            .iter()
            .filter(|p| p.stage == stage)
            .map(|p| p.key)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }

    /// The DXBC container, as stored, of `stage`'s permutation `key` of template `entry` of `forge`.
    pub fn container(&mut self, forge: &str, entry: usize, stage: Stage, key: u32) -> Result<Vec<u8>, Error> {
        let (name, body) = self.body(forge, entry)?;
        let name = name.to_owned();
        let permutations = template::permutations(body).map_err(|source| Error::Permutations {
            forge: forge.to_owned(),
            entry,
            name: name.clone(),
            source,
        })?;
        permutations
            .iter()
            .find(|p| p.stage == stage && p.key == key)
            .map(|p| p.container.to_vec())
            .ok_or(Error::NoPermutation {
                forge: forge.to_owned(),
                entry,
                name,
                stage: stage_name(stage),
                key,
            })
    }

    /// The container a [`ShaderName`] names.
    pub fn named(&mut self, name: &ShaderName) -> Result<Vec<u8>, Error> {
        self.container(&name.forge, name.template, name.stage, name.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_and_print_the_dumped_form() {
        let name = ShaderName::parse("ps106683/t106683-0485bb.ps.dxbc").unwrap();
        assert_eq!(
            name,
            ShaderName {
                forge: DEFAULT_FORGE.into(),
                template: 106_683,
                stage: Stage::Pixel,
                key: 0x48_5bb,
            }
        );
        assert_eq!(name.to_string(), "t106683-0485bb.ps.dxbc");
        assert_eq!(ShaderName::parse(&name.to_string()).unwrap(), name);
        // the file alone, a vertex stage, another forge with or without its extension
        let other = ShaderName::parse("DataPC_MAD_USNAT:t61916-400000.vs").unwrap();
        assert_eq!(
            (other.forge.as_str(), other.template, other.stage, other.key),
            ("DataPC_MAD_USNAT.forge", 61_916, Stage::Vertex, 0x40_0000)
        );
        assert_eq!(other.to_string(), "DataPC_MAD_USNAT:t61916-400000.vs.dxbc");
        assert_eq!(
            ShaderName::parse("DataPC_MAD_USNAT.forge:t1-0.ps").unwrap().forge,
            "DataPC_MAD_USNAT.forge"
        );
    }

    #[test]
    fn malformed_names_are_refused_by_reason() {
        for (text, reason) in [
            ("t1-0485bb", "expected t<template>-<key>.<ps|vs>"),
            ("t1-0485bb.gs.dxbc", "the stage is ps or vs"),
            ("106683-0485bb.ps", "expected t<template>-<key>"),
            ("tx-0485bb.ps", "the template is a decimal entry number"),
            ("t1-xyz.ps", "the key is hexadecimal"),
            (
                "vs1/t1-0485bb.ps.dxbc",
                "the directory is <stage><template>, as the file says",
            ),
            (
                "ps2/t1-0485bb.ps.dxbc",
                "the directory is <stage><template>, as the file says",
            ),
        ] {
            let Err(Error::Name(_, why)) = ShaderName::parse(text) else {
                panic!("{text}");
            };
            assert_eq!(why, reason, "{text}");
        }
    }
}
