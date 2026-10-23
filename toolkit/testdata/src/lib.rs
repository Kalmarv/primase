//! Dev-only real-data configuration: where a test finds the games' installs, keys, the tools it runs, the oracle sets
//! it compares against and where it may write. `PRIMASE_DATA` names one TOML file ([`config`] shows its layout);
//! tests ask through typed queries ([`install`], [`keys`], [`tool`], [`oracle`], [`scratch`], [`debug`]) declared in
//! [`registry`]. An unset entry skips the test with one line naming its configuration key, unless `[run] require`
//! says the gate must have it; a set entry whose path is gone always fails.
//!
//! For one release the older `PRIMASE_*` names still work ([`legacy`]): asked through [`real_data`], set in the
//! environment or in the older flat file. Add this crate only in dev-dependencies. It has no dependency on core.
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod config;
mod inputs;
pub mod legacy;
pub mod registry;
pub use inputs::{Kind, Status, VARIABLES, Variable, declared};
pub use legacy::Query;
pub use registry::{Game, OracleSet, Tool};

use config::Config;

/// Refuse a name the registry ([`VARIABLES`]) does not declare: a typo must not skip quietly.
#[track_caller]
fn checked(name: &str) -> &'static Variable {
    declared(name).unwrap_or_else(|| {
        panic!("{name} is not declared in toolkit/testdata/src/inputs.rs: declare it there (and in CONTRIBUTING.md)")
    })
}

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The configuration, read afresh: a named but unreadable or invalid file fails the test.
#[track_caller]
fn load() -> Config {
    Config::load(&environment, |path| std::fs::read_to_string(path)).unwrap_or_else(|e| panic!("{e}"))
}

/// The registry entry a query's configuration key belongs to, for its gate flag and description.
fn entry_of(query: Query) -> Option<&'static registry::Entry> {
    let key = match query {
        Query::Install(g, _) => format!("installs.{}", g.key()),
        Query::Keys(g) => format!("keys.{}", g.key()),
        Query::Tool(t) => format!("tools.{}", t.key()),
        Query::Oracle(s, _, _) => format!("oracles.{}", s.key()),
        Query::Scratch(_) => return Some(&registry::SCRATCH),
        Query::Debug(_) | Query::Retired => return None,
    };
    registry::INSTALLS
        .iter()
        .chain(registry::KEYS)
        .chain(registry::TOOLS)
        .chain(registry::ORACLE_SETS)
        .find(|e| e.key == key)
}

/// The older names already said to be deprecated in this process.
static WARNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// Where `query` points, if configured: what an older name gave it (said once), else its configuration key's path
/// with the query's relative path below it.
fn configured(config: &Config, query: Query) -> Option<PathBuf> {
    if let Some((value, old)) = config.legacy.get(&query.key()) {
        if WARNED.lock().is_ok_and(|mut w| w.insert(old)) {
            eprintln!(
                "primase-testdata: DEPRECATED {old}: configure {} instead",
                query.key()
            );
        }
        return Some(PathBuf::from(value));
    }
    let (key, rel) = query.root()?;
    config.roots.get(&key).map(|root| root.join(rel))
}

/// Resolve `query` to an existing path, or skip: unset prints one SKIP line naming the test and the configuration key,
/// unless the configuration's `[run] require` is `all`, or `gate` and the entry is one the landing gate must have;
/// set but missing always fails.
#[track_caller]
pub fn resolve(query: Query) -> Option<PathBuf> {
    let config = load();
    let gate = entry_of(query).is_some_and(|e| e.gate);
    let require = match config.require.as_deref() {
        Some("all") => true,
        Some("gate") => gate,
        _ => false,
    };
    match (query, configured(&config, query)) {
        (_, Some(path)) if path.exists() => Some(path),
        (_, Some(path)) => panic!("{} = {} is set but does not exist", query.key(), path.display()),
        (Query::Retired, None) => None,
        (_, None) if require => panic!("{} is not configured and [run] require needs it", query.key()),
        (_, None) => {
            let test = std::thread::current().name().unwrap_or("?").to_owned();
            let file = config
                .file
                .as_ref()
                .map_or_else(|| "PRIMASE_DATA unset".to_owned(), |f| f.display().to_string());
            eprintln!(
                "primase-testdata: SKIP {test} ({} not configured in {file})",
                query.key()
            );
            None
        }
    }
}

/// A game's install folder.
#[track_caller]
pub fn install(game: Game) -> Option<PathBuf> {
    resolve(Query::Install(game, None))
}

/// A game's archive key material.
#[track_caller]
pub fn keys(game: Game) -> Option<PathBuf> {
    resolve(Query::Keys(game))
}

/// A tool's executable or file.
#[track_caller]
pub fn tool(tool: Tool) -> Option<PathBuf> {
    resolve(Query::Tool(tool))
}

/// A file or folder of an oracle set for a game: `<oracles.root>/<set>/<game>/<rel>` (`rel` relative,
/// `/`-separated). The relative path must be a literal so the registry check can see it.
#[track_caller]
pub fn oracle(set: OracleSet, game: Game, rel: &'static str) -> Option<PathBuf> {
    resolve(Query::Oracle(set, game, rel))
}

/// Where a real-data test writes what it keeps: the configured scratch root, else the OS temporary folder.
#[track_caller]
pub fn scratch() -> PathBuf {
    let config = load();
    configured(&config, Query::Scratch(None)).unwrap_or_else(std::env::temp_dir)
}

/// A debug key's value (`PRIMASE_DEBUG=key=value;key=value`, [`registry::DEBUG`]); `None` when unset.
#[track_caller]
pub fn debug(key: &str) -> Option<String> {
    load().debug(&environment, key)
}

/// Resolve an older name to a raw value: a switch's or filter's value, an output folder, a configured path (not
/// checked to exist), or a child process's handoff from the environment. Fails on a broken configuration.
#[track_caller]
pub fn real_data_value(name: &str) -> Option<OsString> {
    let variable = checked(name);
    match legacy::translate(variable.name) {
        Some(Query::Debug(key)) => debug(key).map(OsString::from),
        Some(Query::Retired) => None,
        Some(query) => configured(&load(), query).map(PathBuf::into_os_string),
        None => environment(variable.name),
    }
}

/// [`real_data_value`], the configuration's errors returned instead of failing.
#[track_caller]
pub fn try_real_data_value(name: &str) -> Result<Option<OsString>, config::ConfigError> {
    let variable = checked(name);
    let config = Config::load(&environment, |path| std::fs::read_to_string(path))?;
    Ok(match legacy::translate(variable.name) {
        Some(Query::Debug(key)) => config.debug(&environment, key).map(OsString::from),
        Some(Query::Retired) => None,
        Some(query) => configured(&config, query).map(PathBuf::into_os_string),
        None => environment(variable.name),
    })
}

/// Resolve an older name to an existing path ([`resolve`] of what it stands for; a debug key naming a file is
/// checked to exist the same way). See CONTRIBUTING.md for the configuration.
#[track_caller]
pub fn real_data(name: &str) -> Option<PathBuf> {
    let variable = checked(name);
    match legacy::translate(variable.name) {
        Some(Query::Debug(key)) => debug(key).map(PathBuf::from).inspect(|p| {
            assert!(p.exists(), "debug {key} = {} does not exist", p.display());
        }),
        Some(query) => resolve(query),
        None => environment(variable.name).map(PathBuf::from),
    }
}

/// The bytes of a real-data input file, for a unit test inside a library crate, which has no file access of its own:
/// the file `real_data` resolved, or one its input names.
pub fn read_input(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// A configured input that yields nothing is a failure, not a pass: call it after enumerating what `name` gave
/// (`what`: "scene folders", "drawables"), with how many were found.
#[track_caller]
pub fn expect_items(name: &str, what: &str, n: usize) {
    let variable = checked(name);
    if n == 0 {
        panic!(
            "{name} is set but yielded 0 {what} (expected: {})",
            variable.about
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The configuration a fake environment (`env`) and file (`text`, named `data.toml`) give.
    fn config(env: &[(&str, &str)], text: Option<&str>) -> Result<Config, config::ConfigError> {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .chain(text.map(|_| ("PRIMASE_DATA".to_owned(), "data.toml".to_owned())))
            .collect();
        Config::load(&|name| env.get(name).map(OsString::from), |_| {
            Ok(text.unwrap_or_default().to_owned())
        })
    }

    #[test]
    fn the_structured_file_gives_each_query_its_path() {
        let c = config(
            &[],
            Some(
                "[installs]\nriders = 'games/rr'\n[keys]\ngta = 'keys/gta'\n[tools]\nwarp = 'bin/warp'\n\
                 [oracles]\nroot = 'oracles'\n[scratch]\nroot = 'scratch'\n[run]\nrequire = 'gate'\n",
            ),
        )
        .unwrap();
        assert_eq!(c.require.as_deref(), Some("gate"));
        let at = |q| configured(&c, q);
        assert_eq!(
            at(Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge"))),
            Some(PathBuf::from("games/rr").join("DataPC_MAD_USNAT.forge"))
        );
        assert_eq!(at(Query::Keys(Game::Gta)), Some(PathBuf::from("keys/gta")));
        assert_eq!(at(Query::Tool(Tool::Warp)), Some(PathBuf::from("bin/warp")));
        assert_eq!(
            at(Query::Oracle(OracleSet::CodeWalker, Game::Gta, "res/a.json")),
            Some(["oracles", "codewalker", "gta", "res", "a.json"].iter().collect())
        );
        assert_eq!(
            at(Query::Scratch(Some("warp"))),
            Some(["scratch", "warp"].iter().collect())
        );
        assert_eq!(at(Query::Install(Game::Sekiro, None)), None);
        // an empty file, or none, configures nothing
        assert_eq!(config(&[], Some("")).unwrap().roots.len(), 0);
        assert!(config(&[], None).unwrap().file.is_none());
    }

    #[test]
    fn unknown_sections_keys_and_values_are_errors() {
        for text in [
            "[install]\nsekiro = 'x'",
            "[installs]\nsekiro2 = 'x'",
            "[tools]\nwarp = 3",
            "[run]\nrequire = 'yes'",
            "[oracles]\nsoulsformats = 'x'",
            "not valid = [",
            "PRIMASE_NOT_DECLARED_ANYWHERE = 'x'",
        ] {
            let e = config(&[], Some(text))
                .err()
                .unwrap_or_else(|| panic!("{text}: accepted"));
            assert!(e.to_string().contains("data.toml"), "{e}");
        }
        let unreadable = Config::load(
            &|name| (name == "PRIMASE_DATA").then(|| OsString::from("gone.toml")),
            |_| Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        )
        .unwrap_err();
        assert!(unreadable.to_string().contains("gone.toml"));
    }

    #[test]
    fn the_older_flat_file_and_environment_names_set_what_they_stand_for() {
        let c = config(
            &[("PRIMASE_GTA_DIR", "env/gta")],
            Some(
                "PRIMASE_GTA_DIR = 'file/gta'\nPRIMASE_GTA_LISTING = 'cw/listing.jsonl'\n\
                 PRIMASE_ANVIL_FORGE_USNAT = 'rr/usnat.forge'\nPRIMASE_SHADER_DIFF_LIMIT = '7'\n\
                 PRIMASE_REQUIRE_REAL_DATA = '1'\n",
            ),
        )
        .unwrap();
        assert_eq!(c.require.as_deref(), Some("all"));
        // the environment wins over the file, as it did
        assert_eq!(
            configured(&c, Query::Install(Game::Gta, None)),
            Some(PathBuf::from("env/gta"))
        );
        assert_eq!(
            configured(
                &c,
                Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl")
            ),
            Some(PathBuf::from("cw/listing.jsonl"))
        );
        assert_eq!(
            configured(&c, Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge"))),
            Some(PathBuf::from("rr/usnat.forge"))
        );
        assert_eq!(c.debug(&|_| None, "shader.diff_limit").as_deref(), Some("7"));
        // PRIMASE_DEBUG wins over an older switch
        let env = |n: &str| {
            (n == "PRIMASE_DEBUG").then(|| OsString::from("shader.diff_limit=9; gta.region=legion"))
        };
        assert_eq!(c.debug(&env, "shader.diff_limit").as_deref(), Some("9"));
        assert_eq!(c.debug(&env, "gta.region").as_deref(), Some("legion"));
    }

    #[test]
    fn every_older_name_stands_for_a_declared_entry() {
        for v in VARIABLES {
            match (v.kind, legacy::translate(v.name)) {
                (Kind::Config | Kind::Child | Kind::Test, None) => {}
                (Kind::Config | Kind::Child | Kind::Test, Some(q)) => {
                    panic!("{}: not configuration, but translated to {q:?}", v.name)
                }
                (_, None) => panic!("{}: no translation", v.name),
                (_, Some(Query::Retired)) => assert_eq!(v.status, Status::Retired, "{}", v.name),
                (_, Some(Query::Debug(key))) => assert!(
                    registry::DEBUG.iter().any(|(k, _)| *k == key),
                    "{}: debug key {key} undeclared",
                    v.name
                ),
                (_, Some(q)) => assert!(entry_of(q).is_some(), "{}: {q:?} has no registry entry", v.name),
            }
        }
        // the registry's own rules: unique keys, something to say for each
        let mut keys: Vec<&str> = registry::INSTALLS
            .iter()
            .chain(registry::KEYS)
            .chain(registry::TOOLS)
            .chain(registry::ORACLE_SETS)
            .map(|e| e.key)
            .collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n);
        assert!(registry::DEBUG.iter().all(|(_, about)| !about.is_empty()));
    }

    #[test]
    fn undeclared_names_and_empty_inputs_fail() {
        let undeclared = std::panic::catch_unwind(|| real_data_value(concat!("PRIMASE", "_NOT_DECLARED")));
        assert!(undeclared.is_err());
        let empty = std::panic::catch_unwind(|| expect_items("PRIMASE_GTA_DIR", "archives", 0));
        assert!(empty.is_err());
        expect_items("PRIMASE_GTA_DIR", "archives", 1);
        let mut names: Vec<&str> = VARIABLES.iter().map(|v| v.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), VARIABLES.len());
        assert!(
            VARIABLES
                .iter()
                .all(|v| v.name.starts_with("PRIMASE_") && !v.about.is_empty())
        );
        assert!(VARIABLES.iter().all(|v| !v.gate || v.kind == Kind::Input));
    }

    #[test]
    fn every_input_states_its_source_and_retired_ones_leave_the_gate() {
        for v in VARIABLES {
            if v.kind == Kind::Input {
                assert!(!v.source.is_empty(), "{}: say where its data comes from", v.name);
            } else {
                assert!(
                    v.source.is_empty() && v.status == Status::Live,
                    "{}: only inputs have a source and a status",
                    v.name
                );
            }
            assert!(
                !(v.status == Status::Retired && v.gate),
                "{}: a retired obligation is not a landing-gate input",
                v.name
            );
        }
    }
}
