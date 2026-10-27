//! Real-data configuration for tests: where a test finds the games' installs, keys, the tools it runs, the oracle sets
//! it compares against and where it may write. `GAME_DATA` names one TOML file ([`config`] shows its layout), and one
//! file serves every repository that uses this crate. Each consumer declares what its tests read in its own registry
//! ([`inputs!`], in `tests/inputs/mod.rs`), and its tests ask through the typed queries that gives it (`install`,
//! `install_file`, `keys`, `tool`, `oodle`, `oracle`, `debug`), every file of an install or an oracle set declared one
//! by one. An unset entry skips the test with one line naming its configuration key, unless `[run] require` says the
//! gate must have it; a set entry whose path is gone always fails, and so does a file the registry does not declare.
//! Each consumer runs [`check_declared`] as one test, and its README lists its inputs ([`doc`]). What a test compares
//! against its oracle it counts in a [`gate::GateReport`].
//!
//! Add this crate only in dev-dependencies.
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod check;
pub mod config;
pub mod doc;
pub mod gate;
mod inputs;
mod query;
mod registry;
pub use check::{check_declared, problems};
pub use query::Query;
pub use registry::{Declared, Entry, Names, Registry, SCRATCH, Set, Status};

use config::Config;

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The configuration, read afresh: a named but unreadable or invalid file fails the test.
#[track_caller]
fn load() -> Config {
    Config::load(&environment, |path| std::fs::read_to_string(path)).unwrap_or_else(|e| panic!("{e}"))
}

/// Whether the landing gate must have what `query` names: a declared file's own flag ([`Registry::declared`]), else
/// its configuration key's ([`Registry::installs`], [`Registry::keys`], [`Registry::tools`]). A file in an install or
/// an oracle set that is not declared fails, and so does an install, keys or a tool the registry does not declare: a
/// mistyped path must not skip quietly.
#[track_caller]
fn gate_of(registry: &Registry, query: Query) -> bool {
    match query {
        Query::Install(_, Some(_)) | Query::Oracle(..) => {
            registry
                .declaration(query)
                .unwrap_or_else(|| {
                    panic!(
                        "{} is not declared in this crate's tests/inputs/mod.rs (declared): declare it there, with \
                         what it holds and where it comes from",
                        query.key()
                    )
                })
                .gate
        }
        Query::Install(..) | Query::Keys(_) | Query::Tool(_) => {
            let key = query.key();
            registry
                .entry(&key)
                .unwrap_or_else(|| panic!("{key} is not declared in this crate's tests/inputs/mod.rs"))
                .gate
        }
        Query::Scratch(_) | Query::Debug(_) => false,
    }
}

/// Where `query` points, if configured: its configuration key's path with the query's relative path below it.
fn configured(config: &Config, query: Query) -> Option<PathBuf> {
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
}

/// The decision for `query` under `config`: a configured path that exists is found and one that does not fails, except
/// an oracle file whose set this machine does not hold (no `<oracles.root>/<set>` folder in a root that exists): that
/// set is not configured here, as an unset key is. What is not configured skips, or fails where `[run] require` needs
/// it (`all`, or `gate` and `gate` says the landing gate must have it).
fn decide(registry: &Registry, config: &Config, query: Query, gate: bool) -> Decision {
    // an input no machine here holds: used where it is there, else skipped naming why, whatever require says
    if let Some(Declared {
        status: Status::Unheld { why },
        ..
    }) = registry.declaration(query)
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
    let file = || {
        config.file.as_ref().map_or_else(
            || " (GAME_DATA unset)".to_owned(),
            |f| format!(" in {}", f.display()),
        )
    };
    let unset = match (query, configured(config, query)) {
        (_, None) => format!("{} not configured{}", query.key(), file()),
        (Query::Oracle(set, ..), Some(path)) => {
            let root = config.roots.get("oracles.root");
            if let Some(root) = root.filter(|r| !r.exists()) {
                return Decision::Fail(format!(
                    "oracles.root = {} is set but does not exist",
                    root.display()
                ));
            }
            match root.map(|r| r.join(set)) {
                Some(folder) if !folder.exists() => {
                    format!("oracles.{set} not on this machine: no {}", folder.display())
                }
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

/// Resolve `query`, which `registry` must declare, to an existing path, or skip: what is not configured prints one SKIP
/// line naming the test and the configuration key, unless the configuration's `[run] require` is `all`, or `gate` and
/// the entry is one the landing gate must have (a blocked file always is). An oracle set this machine does not hold
/// (no `<oracles.root>/<set>` folder) counts as not configured; a configured path that does not exist fails (inside a
/// set that is there: a typo or a broken tree), and so does an undeclared query.
#[track_caller]
pub fn resolve(registry: &Registry, query: Query) -> Option<PathBuf> {
    let gate = gate_of(registry, query);
    match decide(registry, &load(), query, gate) {
        Decision::Found(path) => Some(path),
        Decision::Fail(why) => panic!("{why}"),
        Decision::Skip(why) => {
            let test = std::thread::current().name().unwrap_or("?").to_owned();
            eprintln!("primase-testdata: SKIP {test} ({why})");
            None
        }
    }
}

/// Where a test gets its Oodle codec: the game's library where the platform loads one, else nothing to configure.
#[derive(Clone, Debug)]
pub struct OodleLibrary(Option<PathBuf>);

impl OodleLibrary {
    /// The configured game library: `Some` on Windows, where it is the codec; `None` elsewhere, where oozextract is
    /// (`primase_compress::oodle::Oodle::open` takes it as it is).
    pub fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }
}

/// The Oodle codec for a game's data (`version`: the tool's key, `oodle5`, `oodle6` or `oodle9`): on Windows the game's
/// library, which `tools.<version>` names (a test skips, or fails under `[run] require`, where it is not configured);
/// elsewhere oozextract decodes, so there is nothing to configure and nothing to skip for.
#[track_caller]
pub fn oodle(registry: &Registry, version: &'static str) -> Option<OodleLibrary> {
    if cfg!(windows) {
        resolve(registry, Query::Tool(version)).map(|path| OodleLibrary(Some(path)))
    } else {
        Some(OodleLibrary(None))
    }
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

/// A debug key's value (`GAME_DATA_DEBUG=key=value;key=value`); `None` when unset. The key must be one `registry`
/// declares ([`Registry::debug`]).
#[track_caller]
pub fn debug(registry: &Registry, key: &str) -> Option<String> {
    assert!(
        registry.debug.iter().any(|(k, _)| *k == key),
        "debug key {key} is not declared in this crate's tests/inputs/mod.rs"
    );
    config::debug(&environment, key)
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
fn about_of(registry: &Registry, query: Query) -> &'static str {
    if let Some(d) = registry.declaration(query) {
        return d.about;
    }
    let key = query.root().map(|(key, _)| key).unwrap_or_default();
    registry.entry(&key).map_or("", |e| e.about)
}

/// A configured input that yields nothing is a failure, not a pass: call it after enumerating what `query` gave
/// (`what`: "scene folders", "drawables"), with how many were found.
#[track_caller]
pub fn expect_found(registry: &Registry, query: Query, what: &str, n: usize) {
    if n == 0 {
        let about = about_of(registry, query);
        panic!("{} is set but yielded 0 {what} (expected: {about})", query.key());
    }
}

#[cfg(test)]
mod tests;
