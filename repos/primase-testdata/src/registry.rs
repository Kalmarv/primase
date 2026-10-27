//! What a consumer's tests may ask the real-data configuration for ([`Registry`]): the games' installs and keys, the
//! tools, the oracle sets and the debug keys, each declared once with what it is, where it comes from and whether the
//! landing gate needs it. Every file a test asks for inside an install or an oracle set is declared too
//! ([`Registry::declared`]), with what it holds, where it comes from and whether its obligation stands: a query for an
//! undeclared file fails, so a mistyped path never skips quietly.
//!
//! A consumer declares its own registry in its test-support module with [`crate::inputs!`]; the configuration file
//! names its entries by their keys (`installs.sekiro`, `tools.warp`, `oracles.root`), and one file serves every
//! consumer.

use crate::query::Query;

/// One declared configuration entry: an install or a game's keys (named by the game's key), or a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its key within its section (`sekiro` of `installs.sekiro`, `warp` of `tools.warp`).
    pub name: &'static str,
    /// What it is and the layout it must have.
    pub about: &'static str,
    /// Where its data comes from, and so how far it is independent of what it checks.
    pub source: &'static str,
    /// The landing gate must have it (`[run] require = 'gate'`).
    pub gate: bool,
}

/// An oracle set: one producer's outputs, under `<oracles.root>/<set>/<game>/`; its files are declared one by one
/// ([`Registry::declared`]), each with whether the landing gate needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set {
    /// Its folder under `oracles.root`.
    pub name: &'static str,
    /// What its files are.
    pub about: &'static str,
    /// Its producer, and so how far its files are independent of what they check.
    pub source: &'static str,
}

/// Whether a declared file's obligation stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The obligation stands and the input it names is acceptable.
    Live,
    /// The obligation stands, but no acceptable input exists yet (the existing data is not independent, or its
    /// producer is gone): `source` names the replacement. It is a landing-gate input, so a gate run without it fails:
    /// its tests never pass by skipping.
    Blocked,
    /// The obligation stands and the input is acceptable, but no machine here holds it (`why`: an install the owner
    /// does not have). Never a landing-gate input: where it is not there, its tests skip naming `why`, whatever
    /// `[run] require` says; a machine that comes to hold it configures it as any other.
    Unheld { why: &'static str },
}

/// A file or folder a test asks for inside an install ([`Query::Install`] with a path) or an oracle set
/// ([`Query::Oracle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declared {
    pub query: Query,
    pub status: Status,
    /// The landing gate must have it (`[run] require = 'gate'`).
    pub gate: bool,
    /// What it is, its layout or schema, and the tests that read it.
    pub about: &'static str,
    /// Where its data comes from, and so how far it is independent of what it checks.
    pub source: &'static str,
}

/// How a consumer's typed queries name its games, tools and oracle sets: each as (its enum variant, its key). The
/// self-check ([`crate::check_declared`]) reads the queries a crate's sources write with them.
#[derive(Clone, Copy, Debug)]
pub struct Names {
    pub games: &'static [(&'static str, &'static str)],
    pub tools: &'static [(&'static str, &'static str)],
    pub oracle_sets: &'static [(&'static str, &'static str)],
}

/// Everything one consumer's tests may ask for. A consumer declares it with [`crate::inputs!`], which also gives it
/// typed queries over it; every function of this crate that resolves a query takes it.
#[derive(Clone, Copy, Debug)]
pub struct Registry {
    /// The names its typed queries give its games, tools and oracle sets.
    pub names: Names,
    /// Every install, by its game's key (`installs.<game>`).
    pub installs: &'static [Entry],
    /// Every game's archive key material, by its game's key (`keys.<game>`).
    pub keys: &'static [Entry],
    /// Every tool, by its key (`tools.<tool>`).
    pub tools: &'static [Entry],
    /// Every oracle set, by its folder under `oracles.root`, which holds one folder per game.
    pub oracle_sets: &'static [Set],
    /// Every file of an install or an oracle set a test asks for.
    pub declared: &'static [Declared],
    /// Every debug key `GAME_DATA_DEBUG` may set (`key=value;key=value`), with what it does: switches and parameters
    /// of the real-data tests, none of them data.
    pub debug: &'static [(&'static str, &'static str)],
}

/// The scratch root: where real-data tests write what they keep (`[scratch] root`); the OS temporary folder when unset.
/// Every consumer has it.
pub const SCRATCH: Entry = Entry {
    name: "root",
    about: "where real-data tests write their outputs (the OS temporary folder when unset)",
    source: "",
    gate: false,
};

impl Registry {
    /// The declaration of `query`, a file in an install or an oracle set; `None` for an undeclared one, and for
    /// queries the configuration's keys name whole (an install, keys, a tool, the scratch folder, a debug key).
    pub fn declaration(&self, query: Query) -> Option<&'static Declared> {
        self.declared.iter().find(|d| d.query == query)
    }

    /// The entry a configuration key names whole (`installs.sekiro`, `keys.gta`, `tools.warp`, `scratch.root`).
    pub fn entry(&self, key: &str) -> Option<&'static Entry> {
        let (section, name) = key.split_once('.')?;
        let entries = match section {
            "installs" => self.installs,
            "keys" => self.keys,
            "tools" => self.tools,
            "scratch" => return (name == SCRATCH.name).then_some(&SCRATCH),
            _ => return None,
        };
        entries.iter().find(|e| e.name == name)
    }

    /// The configuration keys of its entries, with each entry: installs, keys, tools and the scratch root.
    pub fn entries(&self) -> impl Iterator<Item = (String, &'static Entry)> + '_ {
        let of = |section: &'static str, entries: &'static [Entry]| {
            entries.iter().map(move |e| (format!("{section}.{}", e.name), e))
        };
        of("installs", self.installs)
            .chain(of("keys", self.keys))
            .chain(of("tools", self.tools))
            .chain([("scratch.root".to_owned(), &SCRATCH)])
    }

    /// The oracle set named `name` (its folder under `oracles.root`).
    pub fn oracle_set(&self, name: &str) -> Option<&'static Set> {
        self.oracle_sets.iter().find(|s| s.name == name)
    }

    /// What it fails to say about itself: a key declared twice, an entry or a debug key that does not say what it
    /// is, a declared file that is not in an install or an oracle set, is declared twice, has a path that is not a
    /// relative `/`-separated one, names an install or oracle set it does not declare, or whose status and gate flag
    /// disagree. Empty when it is whole.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut keys: Vec<String> = self
            .entries()
            .map(|(key, _)| key)
            .chain(self.oracle_sets.iter().map(|s| format!("oracles.{}", s.name)))
            .chain(self.declared.iter().map(|d| d.query.key()))
            .chain(self.debug.iter().map(|(k, _)| format!("debug.{k}")))
            .collect();
        keys.sort_unstable();
        problems.extend(
            keys.windows(2)
                .filter(|w| w[0] == w[1])
                .map(|w| format!("{}: declared twice", w[0])),
        );
        for (key, e) in self.entries().filter(|(k, _)| k != "scratch.root") {
            if e.about.is_empty() || e.source.is_empty() {
                problems.push(format!("{key}: say what it is and where it comes from"));
            }
        }
        for s in self.oracle_sets {
            if s.about.is_empty() || s.source.is_empty() {
                problems.push(format!(
                    "oracles.{}: say what it is and where it comes from",
                    s.name
                ));
            }
        }
        for (k, about) in self.debug {
            if about.is_empty() {
                problems.push(format!("debug.{k}: say what it does"));
            }
        }
        for d in self.declared {
            let key = d.query.key();
            let rel = match d.query {
                Query::Install(game, Some(rel)) => {
                    if !self.installs.iter().any(|e| e.name == game) {
                        problems.push(format!("{key}: installs.{game} is not declared"));
                    }
                    rel
                }
                Query::Oracle(set, _, rel) => {
                    if self.oracle_set(set).is_none() {
                        problems.push(format!("{key}: oracles.{set} is not declared"));
                    }
                    rel
                }
                _ => {
                    problems.push(format!(
                        "{key}: only files in an install or an oracle set are declared"
                    ));
                    continue;
                }
            };
            if d.about.is_empty() || d.source.is_empty() {
                problems.push(format!("{key}: say what it is and where it comes from"));
            }
            // a blocked file is a landing-gate input: a gate run without it fails, it never passes by skipping
            if d.status == Status::Blocked && !d.gate {
                problems.push(format!("{key}: blocked, so a landing-gate input"));
            }
            if let Status::Unheld { why } = d.status
                && (d.gate || why.is_empty())
            {
                problems.push(format!("{key}: unheld, so never a gate input, and say why"));
            }
            if rel.is_empty()
                || rel.starts_with('/')
                || rel.contains('\\')
                || rel.split('/').any(|p| p == "..")
            {
                problems.push(format!("{key}: a relative, /-separated path inside its folder"));
            }
        }
        problems
    }
}
