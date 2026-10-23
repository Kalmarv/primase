//! What a test asks the real-data configuration for ([`Query`]): an install or a file in it, a game's keys, a tool, a
//! file of an oracle set, the scratch folder or a debug key.
use std::path::PathBuf;

use crate::registry::{Game, OracleSet, Tool};

/// What a test asks the configuration for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// A game's install, or a file or folder inside it (relative, `/`-separated).
    Install(Game, Option<&'static str>),
    Keys(Game),
    Tool(Tool),
    /// A file or folder of an oracle set's folder for a game (relative, `/`-separated).
    Oracle(OracleSet, Game, &'static str),
    /// The scratch root, or a folder under it.
    Scratch(Option<&'static str>),
    /// A debug key ([`crate::registry::DEBUG`]).
    Debug(&'static str),
    /// A name whose obligation its owners retired: never set.
    Retired,
}

impl Query {
    /// The query's key: the configuration key and, for a file or folder inside, its relative path
    /// (`installs.riders/DataPC_MAD_USNAT.forge`, `oracles.codewalker.gta/listing.jsonl`).
    pub fn key(self) -> String {
        match self {
            Query::Install(g, None) => format!("installs.{}", g.key()),
            Query::Install(g, Some(rel)) => format!("installs.{}/{rel}", g.key()),
            Query::Keys(g) => format!("keys.{}", g.key()),
            Query::Tool(t) => format!("tools.{}", t.key()),
            Query::Oracle(s, g, rel) => format!("oracles.{}.{}/{rel}", s.key(), g.key()),
            Query::Scratch(None) => "scratch.root".to_owned(),
            Query::Scratch(Some(rel)) => format!("scratch.root/{rel}"),
            Query::Debug(k) => format!("debug.{k}"),
            Query::Retired => "retired".to_owned(),
        }
    }

    /// The structured configuration key it is resolved from, and the path below it (`oracles.root` and
    /// `codewalker/gta/listing.jsonl`).
    pub(crate) fn root(self) -> Option<(String, PathBuf)> {
        let rel = |parts: &[&str]| -> PathBuf { parts.iter().flat_map(|p| p.split('/')).collect() };
        match self {
            Query::Install(g, rel_path) => Some((
                format!("installs.{}", g.key()),
                rel(&rel_path.into_iter().collect::<Vec<_>>()),
            )),
            Query::Keys(g) => Some((format!("keys.{}", g.key()), PathBuf::new())),
            Query::Tool(t) => Some((format!("tools.{}", t.key()), PathBuf::new())),
            Query::Oracle(s, g, r) => Some(("oracles.root".to_owned(), rel(&[s.key(), g.key(), r]))),
            Query::Scratch(r) => Some(("scratch.root".to_owned(), rel(&r.into_iter().collect::<Vec<_>>()))),
            Query::Debug(_) | Query::Retired => None,
        }
    }
}
