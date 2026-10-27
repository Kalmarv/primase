//! What a test asks the real-data configuration for ([`Query`]): an install or a file in it, a game's keys, a tool, a
//! file of an oracle set, the scratch folder or a debug key. Names are the configuration's keys; a consumer's typed
//! `Query` ([`crate::inputs!`]) gives one with `raw()`.
use std::path::PathBuf;

/// What a test asks the configuration for, by the configuration's own names: a game's key (`sekiro`), a tool's
/// (`warp`), an oracle set's folder (`codewalker`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Query {
    /// A game's install, or a file or folder inside it (relative, `/`-separated).
    Install(&'static str, Option<&'static str>),
    /// A game's archive key material.
    Keys(&'static str),
    /// A tool's executable or file.
    Tool(&'static str),
    /// A file or folder of an oracle set's folder for a game: the set, the game, the path (relative, `/`-separated).
    Oracle(&'static str, &'static str, &'static str),
    /// The scratch root, or a folder under it.
    Scratch(Option<&'static str>),
    /// A debug key ([`crate::Registry::debug`]).
    Debug(&'static str),
}

impl Query {
    /// The query's key: the configuration key and, for a file or folder inside, its relative path
    /// (`installs.riders/DataPC_MAD_USNAT.forge`, `oracles.codewalker.gta/listing.jsonl`).
    pub fn key(self) -> String {
        match self {
            Query::Install(g, None) => format!("installs.{g}"),
            Query::Install(g, Some(rel)) => format!("installs.{g}/{rel}"),
            Query::Keys(g) => format!("keys.{g}"),
            Query::Tool(t) => format!("tools.{t}"),
            Query::Oracle(s, g, rel) => format!("oracles.{s}.{g}/{rel}"),
            Query::Scratch(None) => "scratch.root".to_owned(),
            Query::Scratch(Some(rel)) => format!("scratch.root/{rel}"),
            Query::Debug(k) => format!("debug.{k}"),
        }
    }

    /// The structured configuration key it is resolved from, and the path below it (`oracles.root` and
    /// `codewalker/gta/listing.jsonl`).
    pub(crate) fn root(self) -> Option<(String, PathBuf)> {
        let rel = |parts: &[&str]| -> PathBuf { parts.iter().flat_map(|p| p.split('/')).collect() };
        match self {
            Query::Install(g, rel_path) => Some((
                format!("installs.{g}"),
                rel(&rel_path.into_iter().collect::<Vec<_>>()),
            )),
            Query::Keys(g) => Some((format!("keys.{g}"), PathBuf::new())),
            Query::Tool(t) => Some((format!("tools.{t}"), PathBuf::new())),
            Query::Oracle(s, g, r) => Some(("oracles.root".to_owned(), rel(&[s, g, r]))),
            Query::Scratch(r) => Some(("scratch.root".to_owned(), rel(&r.into_iter().collect::<Vec<_>>()))),
            Query::Debug(_) => None,
        }
    }
}
