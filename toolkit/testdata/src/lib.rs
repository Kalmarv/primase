//! Dev-only real-data configuration: where a test finds the games' installs, keys, the tools it runs, the oracle sets
//! it compares against and where it may write. `PRIMASE_DATA` names one TOML file ([`config`] shows its layout);
//! tests ask through typed queries ([`install`], [`install_file`], [`keys`], [`tool`], [`oracle`], [`scratch`],
//! [`debug`]) declared in [`registry`], every file of an install or an oracle set one by one. An unset entry skips the
//! test with one line naming its configuration key, unless `[run] require` says the gate must have it; a set entry
//! whose path is gone always fails, and so does a file the registry does not declare. CONTRIBUTING.md's tables are
//! made from the registry ([`doc`]).
//!
//! For this release the older `PRIMASE_*` names still configure ([`legacy`]): set in the environment or in the older
//! flat file, each stands for its key and says once that it is deprecated; the next release removes them. Add this
//! crate only in dev-dependencies. It has no dependency on core.
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod config;
pub mod doc;
pub mod legacy;
mod query;
pub mod registry;
pub use query::Query;
pub use registry::Status;
pub use registry::{Game, OracleSet, Tool};

use config::Config;

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The configuration, read afresh: a named but unreadable or invalid file fails the test.
#[track_caller]
fn load() -> Config {
    Config::load(&environment, |path| std::fs::read_to_string(path)).unwrap_or_else(|e| panic!("{e}"))
}

/// Whether the landing gate must have what `query` names: a declared file's own flag ([`registry::DECLARED`]), else
/// its configuration key's ([`registry::INSTALLS`], [`registry::KEYS`], [`registry::TOOLS`]). A file in an install or
/// an oracle set that is not declared fails: a mistyped path must not skip quietly.
#[track_caller]
fn gate_of(query: Query) -> bool {
    let key = match query {
        Query::Install(_, Some(_)) | Query::Oracle(..) => {
            return registry::declaration(query)
                .unwrap_or_else(|| {
                    panic!(
                        "{} is not declared in toolkit/testdata/src/registry.rs (DECLARED): declare it there, with \
                         what it holds and where it comes from",
                        query.key()
                    )
                })
                .gate;
        }
        Query::Install(g, None) => format!("installs.{}", g.key()),
        Query::Keys(g) => format!("keys.{}", g.key()),
        Query::Tool(t) => format!("tools.{}", t.key()),
        Query::Scratch(_) | Query::Debug(_) | Query::Retired => return false,
    };
    registry::INSTALLS
        .iter()
        .chain(registry::KEYS)
        .chain(registry::TOOLS)
        .any(|e| e.key == key && e.gate)
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
    // an install, a tool or keys configured as a file is that file: joining an empty path would add a separator
    config.roots.get(&key).map(|root| {
        if rel.as_os_str().is_empty() {
            root.clone()
        } else {
            root.join(rel)
        }
    })
}

/// What [`resolve`] does with a query: the path, a skip or a failure, with why.
#[derive(Debug, PartialEq)]
enum Decision {
    Found(PathBuf),
    /// Not configured on this machine (and not required): the reason the SKIP line gives.
    Skip(String),
    Fail(String),
    /// A retired name: never set, nothing said.
    Retired,
}

/// The decision for `query` under `config`: a configured path that exists is found and one that does not fails, except
/// an oracle file whose set this machine does not hold (no `<oracles.root>/<set>` folder in a root that exists): that
/// set is not configured here, as an unset key is. What is not configured skips, or fails where `[run] require` needs it (`all`, or `gate`
/// and `gate` says the landing gate must have it).
fn decide(config: &Config, query: Query, gate: bool) -> Decision {
    // an input no machine here holds: used where it is there, else skipped naming why, whatever require says
    if let Some(registry::Declared {
        status: Status::Unheld { why },
        ..
    }) = registry::declaration(query)
    {
        return match configured(config, query).filter(|p| p.exists()) {
            Some(path) => Decision::Found(path),
            None => Decision::Skip(format!("{} is held by no machine here: {why}", query.key())),
        };
    }
    let require = match config.require.as_deref() {
        Some("all") => true,
        Some("gate") => gate,
        _ => false,
    };
    if query == Query::Retired {
        return Decision::Retired;
    }
    let file = || {
        config
            .file
            .as_ref()
            .map_or_else(|| "PRIMASE_DATA unset".to_owned(), |f| f.display().to_string())
    };
    let unset = match (query, configured(config, query)) {
        (_, None) => format!("{} not configured in {}", query.key(), file()),
        (Query::Oracle(set, ..), Some(path)) if !config.legacy.contains_key(&query.key()) => {
            let root = config.roots.get("oracles.root");
            if let Some(root) = root.filter(|r| !r.exists()) {
                return Decision::Fail(format!(
                    "oracles.root = {} is set but does not exist",
                    root.display()
                ));
            }
            match root.map(|r| r.join(set.key())) {
                Some(folder) if !folder.exists() => format!(
                    "oracles.{} not on this machine: no {}",
                    set.key(),
                    folder.display()
                ),
                _ if path.exists() => return Decision::Found(path),
                _ => {
                    return Decision::Fail(format!(
                        "{} = {} is set but does not exist",
                        query.key(),
                        path.display()
                    ));
                }
            }
        }
        (_, Some(path)) if path.exists() => return Decision::Found(path),
        (_, Some(path)) => {
            return Decision::Fail(format!(
                "{} = {} is set but does not exist",
                query.key(),
                path.display()
            ));
        }
    };
    if require {
        Decision::Fail(format!("{unset}, and [run] require needs it"))
    } else {
        Decision::Skip(unset)
    }
}

/// Resolve `query` to an existing path, or skip: what is not configured prints one SKIP line naming the test and the
/// configuration key, unless the configuration's `[run] require` is `all`, or `gate` and the entry is one the landing
/// gate must have (a blocked file always is). An oracle set this machine does not hold (no `<oracles.root>/<set>`
/// folder) counts as not configured; a configured path that does not exist fails (inside a set that is there: a typo
/// or a broken tree), and so does an undeclared file.
#[track_caller]
pub fn resolve(query: Query) -> Option<PathBuf> {
    let gate = gate_of(query);
    match decide(&load(), query, gate) {
        Decision::Found(path) => Some(path),
        Decision::Fail(why) => panic!("{why}"),
        Decision::Retired => None,
        Decision::Skip(why) => {
            let test = std::thread::current().name().unwrap_or("?").to_owned();
            eprintln!("primase-testdata: SKIP {test} ({why})");
            None
        }
    }
}

/// A game's install folder.
#[track_caller]
pub fn install(game: Game) -> Option<PathBuf> {
    resolve(Query::Install(game, None))
}

/// A file or folder inside a game's install (`rel` relative, `/`-separated): the install's own files are not
/// configuration, a test names them.
#[track_caller]
pub fn install_file(game: Game, rel: &'static str) -> Option<PathBuf> {
    resolve(Query::Install(game, Some(rel)))
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

/// A folder `name` of [`scratch`] that only this process uses ([`own_scratch`]), removed when it is dropped unless the test
/// is failing (what it wrote stays to be looked at).
#[derive(Debug)]
pub struct OwnScratch(PathBuf);

impl std::ops::Deref for OwnScratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for OwnScratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            // best effort: a folder another process still holds open is left for the next sweep of the scratch root
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A folder `name` under [`scratch`] this call alone writes to (`<scratch>/<name>/p<pid>-<call>`, made): a test that
/// exchanges files with an external program (WARP, a compiler) under fixed names writes them here, so two runs of it at
/// once (a gate beside a developer's run, two checkouts) cannot overwrite or truncate each other's files while the program
/// reads them.
#[track_caller]
pub fn own_scratch(name: &str) -> OwnScratch {
    // one folder a call: two tests of one process each own theirs, and one's drop does not take the other's files
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = scratch()
        .join(name)
        .join(format!("p{}-{call}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    OwnScratch(dir)
}

/// The machine-wide lock on the WARP oracle ([`warp_exclusive`]), released when it is dropped.
#[derive(Debug)]
pub struct WarpLock(std::fs::File);

impl Drop for WarpLock {
    fn drop(&mut self) {
        // closing the file would release it too; this says so
        let _ = self.0.unlock();
    }
}

/// Takes the lock on the WARP oracle, waiting for whoever holds it: a test runs `warp.exe` only while it holds it.
///
/// Several `warp.exe` runs at once (two test binaries, a gate beside a developer's run, an agent's run beside another's)
/// do not give each their own results: four copies of `exec_warp_rq` started together failed their comparison against
/// the executor in most rounds (the gate's `hits > 0 && mismatches.is_empty()`, a read of an empty results file, a
/// parse of a cut line), where one copy alone passed 28 of 28 runs, under heavy CPU load too. The lock is a file in the
/// scratch root held with the operating system's own file lock, so it is released if the holder dies.
#[track_caller]
pub fn warp_exclusive() -> WarpLock {
    let dir = scratch().join("warp");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let path = dir.join(".oracle.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    file.lock()
        .unwrap_or_else(|e| panic!("locking {}: {e}", path.display()));
    WarpLock(file)
}

/// A debug key's value (`PRIMASE_DEBUG=key=value;key=value`, [`registry::DEBUG`]); `None` when unset.
#[track_caller]
pub fn debug(key: &str) -> Option<String> {
    load().debug(&environment, key)
}

/// How much a heavy real-data test covers (`[run] gate`): a smoke sample on every batch, or everything (the overnight
/// gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// A sample, drawn with [`seed`]: the default.
    Smoke,
    /// The whole input.
    Full,
}

/// `[run] gate`: [`Depth::Full`] for `full`, else [`Depth::Smoke`].
pub fn depth() -> Depth {
    if load().gate.as_deref() == Some("full") {
        Depth::Full
    } else {
        Depth::Smoke
    }
}

/// The seed a smoke sample is drawn with: `[run] seed` when given (to rerun one sample exactly), else the day (days
/// since 1970-01-01, UTC), so the sample rotates and its coverage accumulates across runs. A test prints the seed it
/// used and names it in its failures.
pub fn seed() -> u64 {
    load().seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400)
    })
}

/// The threads a heavy real-data test's work may use (`[run] threads`, one by default): what a test states for its
/// world (`core.threads`), so a machine opts into the load. Work whose result is the same on any count only.
pub fn threads() -> std::num::NonZeroUsize {
    load().threads.unwrap_or(std::num::NonZeroUsize::MIN)
}

/// The bytes of a real-data input file, for a unit test inside a library crate, which has no file access of its own:
/// the file a query resolved, or one its input names.
pub fn read_input(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// What the registry says `query` names: a declared file's description, else its configuration key's.
fn about_of(query: Query) -> &'static str {
    if let Some(d) = registry::declaration(query) {
        return d.about;
    }
    let key = query.root().map(|(key, _)| key).unwrap_or_default();
    registry::INSTALLS
        .iter()
        .chain(registry::KEYS)
        .chain(registry::TOOLS)
        .chain([&registry::SCRATCH])
        .find(|e| e.key == key)
        .map_or("", |e| e.about)
}

/// A configured input that yields nothing is a failure, not a pass: call it after enumerating what `query` gave
/// (`what`: "scene folders", "drawables"), with how many were found.
#[track_caller]
pub fn expect_found(query: Query, what: &str, n: usize) {
    if n == 0 {
        let about = about_of(query);
        panic!("{} is set but yielded 0 {what} (expected: {about})", query.key());
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

    /// A tool, keys or an install configured as a file is that path exactly: no separator after it (a path ending in
    /// one names a folder, so a file's never exists; PathBuf's comparison ignores it, the string does not).
    #[test]
    fn a_key_configured_as_a_file_is_that_path() {
        let c = config(&[], Some("[tools]\nunity_tpk = 'data/typetrees.tpk'\n")).unwrap();
        let tpk = configured(&c, Query::Tool(Tool::UnityTpk)).unwrap();
        assert_eq!(tpk.as_os_str(), "data/typetrees.tpk");
    }

    /// An oracle set this machine does not hold skips (or fails where required); a file missing inside a set that is
    /// there fails; a file that is there is found.
    /// An input no machine here holds skips, naming why, even where every input is required; one that is there is
    /// found.
    #[test]
    fn an_unheld_input_skips_naming_why_whatever_require_says() {
        let legacy = Query::Oracle(OracleSet::Corpus, Game::Gta, "legacy-effects");
        let c = config(&[], Some("[oracles]\nroot = 'nowhere'\n[run]\nrequire = 'all'\n")).unwrap();
        assert!(matches!(decide(&c, legacy, false), Decision::Skip(why) if why.contains("GTA V Legacy")));
        let root = std::env::temp_dir().join(format!("primase-testdata-unheld-{}", std::process::id()));
        std::fs::create_dir_all(root.join("corpus/gta/legacy-effects")).unwrap();
        let held = config(&[], Some(&format!("[oracles]\nroot = '{}'\n", root.display()))).unwrap();
        assert_eq!(
            decide(&held, legacy, false),
            Decision::Found(root.join("corpus/gta/legacy-effects"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_absent_oracle_set_skips_and_a_missing_file_in_a_present_one_fails() {
        let root = std::env::temp_dir().join(format!("primase-testdata-sets-{}", std::process::id()));
        std::fs::create_dir_all(root.join("corpus").join("general")).unwrap();
        std::fs::write(root.join("corpus").join("general").join("codecs.tsv"), b"").unwrap();
        let text = format!("[oracles]\nroot = '{}'\n", root.display());
        let c = config(&[], Some(&text)).unwrap();
        let codecs = Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv");
        let typo = Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx");
        let elsewhere = Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl");
        assert_eq!(
            decide(&c, codecs, true),
            Decision::Found(root.join("corpus/general/codecs.tsv"))
        );
        assert!(matches!(decide(&c, typo, false), Decision::Fail(why) if why.contains("does not exist")));
        assert!(
            matches!(decide(&c, elsewhere, false), Decision::Skip(why) if why.contains("oracles.codewalker not on this machine"))
        );
        let required = config(&[], Some(&format!("{text}[run]\nrequire = 'all'\n"))).unwrap();
        assert!(
            matches!(decide(&required, elsewhere, false), Decision::Fail(why) if why.contains("require"))
        );
        // a root that is gone is a configuration that is wrong, not sets this machine lacks
        let gone = config(
            &[],
            Some(&format!("[oracles]\nroot = '{}'\n", root.join("gone").display())),
        )
        .unwrap();
        assert!(
            matches!(decide(&gone, elsewhere, false), Decision::Fail(why) if why.contains("oracles.root"))
        );
        std::fs::remove_dir_all(&root).unwrap();
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
            "[run]\ngate = 'quick'",
            "[run]\nseed = 'today'",
            "[run]\nthreads = '0'",
            "[oracles]\nsoulsformats = 'x'",
            "not valid = [",
            "[installs]\nsekiro = 'x'\nPRIMASE_SEKIRO_DIR = 'y'",
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

    /// `[run] gate` and `[run] seed` are read as given; unset, a run is a smoke run drawn with the day's seed.
    #[test]
    fn the_gate_depth_and_seed_are_read() {
        let c = config(&[], Some("[run]\ngate = 'full'\nseed = '20384'\nthreads = '6'\n")).unwrap();
        assert_eq!(
            (c.gate.as_deref(), c.seed, c.threads.map(|t| t.get())),
            (Some("full"), Some(20384), Some(6))
        );
        let unset = config(&[], Some("[run]\nrequire = 'gate'\n")).unwrap();
        assert_eq!((unset.gate, unset.seed, unset.threads), (None, None, None));
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
        // the older switch in the environment, over a structured file
        let gate = config(
            &[("PRIMASE_REQUIRE_REAL_DATA", "gate")],
            Some("[run]\nrequire = 'all'\n"),
        )
        .unwrap();
        assert_eq!(gate.require.as_deref(), Some("gate"));
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
        // a name this build does not declare (work not merged yet) is passed over in the older format
        let other = config(
            &[],
            Some("PRIMASE_NOT_MERGED_YET = 'x'\nPRIMASE_SEKIRO_DIR = 's'\n"),
        )
        .unwrap();
        assert_eq!(
            configured(&other, Query::Install(Game::Sekiro, None)),
            Some(PathBuf::from("s"))
        );
        // PRIMASE_DEBUG wins over an older switch
        let env = |n: &str| {
            (n == "PRIMASE_DEBUG").then(|| OsString::from("shader.diff_limit=9; gta.region=legion"))
        };
        assert_eq!(c.debug(&env, "shader.diff_limit").as_deref(), Some("9"));
        assert_eq!(c.debug(&env, "gta.region").as_deref(), Some("legion"));
    }

    #[test]
    fn every_older_name_stands_for_a_declared_entry() {
        let mut names: Vec<&str> = legacy::OLDER.iter().map(|(n, _)| *n).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "an older name twice");
        for (name, q) in legacy::OLDER {
            assert!(name.starts_with("PRIMASE_") && *name != config::REQUIRE, "{name}");
            match *q {
                Query::Retired => {}
                Query::Debug(key) => assert!(
                    registry::DEBUG.iter().any(|(k, _)| *k == key),
                    "{name}: debug key {key} undeclared"
                ),
                Query::Install(_, Some(_)) | Query::Oracle(..) => {
                    assert!(
                        registry::declaration(*q).is_some(),
                        "{name}: {} is not declared",
                        q.key()
                    );
                }
                q => assert!(!about_of(q).is_empty(), "{name}: {q:?} has no registry entry"),
            }
        }
        // the registry's own rules: unique keys, something to say for each
        let mut keys: Vec<&str> = registry::INSTALLS
            .iter()
            .chain(registry::KEYS)
            .chain(registry::TOOLS)
            .map(|e| e.key)
            .chain(registry::ORACLE_SETS.iter().map(|s| s.key))
            .collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n);
        assert!(registry::DEBUG.iter().all(|(_, about)| !about.is_empty()));
    }

    #[test]
    fn every_declared_file_is_one_of_an_install_or_oracle_set_and_says_what_it_is() {
        let mut keys: Vec<String> = registry::DECLARED.iter().map(|d| d.query.key()).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "a file declared twice");
        for d in registry::DECLARED {
            let key = d.query.key();
            assert!(
                matches!(d.query, Query::Install(_, Some(_)) | Query::Oracle(..)),
                "{key}: only files in an install or an oracle set are declared"
            );
            assert!(
                !d.about.is_empty() && !d.source.is_empty(),
                "{key}: say what it is and where it comes from"
            );
            assert!(
                d.status != Status::Retired,
                "{key}: a retired obligation is removed, not declared"
            );
            // a blocked file is a landing-gate input: a gate run without it fails, it never passes by skipping
            assert!(
                d.status != Status::Blocked || d.gate,
                "{key}: blocked, so a landing-gate input"
            );
            if let Status::Unheld { why } = d.status {
                assert!(
                    !d.gate && !why.is_empty(),
                    "{key}: unheld, so never a gate input, and why"
                );
            }
            let rel = match d.query {
                Query::Install(_, Some(rel)) | Query::Oracle(_, _, rel) => rel,
                _ => unreachable!(),
            };
            assert!(
                !rel.is_empty()
                    && !rel.starts_with('/')
                    && !rel.contains('\\')
                    && !rel.split('/').any(|p| p == ".."),
                "{key}: a relative, /-separated path inside its folder"
            );
        }
    }

    #[test]
    fn an_undeclared_file_fails_and_a_required_blocked_one_never_skips() {
        let typo = std::panic::catch_unwind(|| {
            resolve(Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsvx"))
        });
        assert!(typo.is_err(), "a mistyped oracle path fails");
        let typo = std::panic::catch_unwind(|| resolve(Query::Install(Game::Skate, Some("Date"))));
        assert!(typo.is_err(), "a mistyped install path fails");
        for d in registry::DECLARED.iter().filter(|d| d.status == Status::Blocked) {
            assert!(gate_of(d.query), "{}", d.query.key());
        }
    }

    #[test]
    fn an_empty_configured_input_fails() {
        let gta = Query::Install(Game::Gta, None);
        let empty = std::panic::catch_unwind(|| expect_found(gta, "archives", 0));
        assert!(empty.is_err());
        expect_found(gta, "archives", 1);
    }
}
