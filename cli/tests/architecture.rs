//! The role and layer rules of CONTRIBUTING.md ("Roles", "Layout"), checked over the workspace's manifests and
//! sources.
//!
//! Readers read, writers write, and what breaks a rule today is listed in `architecture.toml` as a break to remove,
//! each with its reason and what removes it (`removed_when`). Each check collects every violation, removes the listed
//! breaks, and fails on what is left; it also fails on a break that no longer occurs. The breaks only go:
//! `breaks_only_go` holds the identity of every one there was, so a new or replaced break fails.
//!
//! - `dependencies`: crate edges per layer (`core`, `toolkit`, `formats`, `worlds`, `cli`). A `worlds` crate depends
//!   on the toolkit and core, and a game or destination crate also on the engine crate it names; an engine crate on
//!   no game, destination or recipe.
//! - `io_follows_roles`: every library crate denies `clippy::disallowed_methods`/`disallowed_types` (file,
//!   environment and process access, listed in `clippy.toml`), and every module that opts back in names a role its
//!   crate has: reader and writer I/O in `formats` and `worlds`, toolkit I/O in the toolkit, test I/O only in test
//!   code.
//! - `game_names`: no game, engine or destination names in core, toolkit and formats code (the test-input registry
//!   aside); a `worlds` crate uses only its own (`[package.metadata.primase] names`).
//! - `entry_points`: examples, bins and the cli compose library entry points; no world data or format handling of
//!   their own.
//! - `passes_registered`: every pass is registered by name, and every public lowering returning a `PassLog` is run
//!   by a pass.
//! - `writers_read_only`: a writer's entry points take the world read-only.
//! - `real_data_variables_are_declared`: every `PRIMASE_*` name in the workspace's sources is declared in the
//!   real-data registry (`toolkit/testdata/src/inputs.rs`), every declared name is read somewhere, and
//!   CONTRIBUTING.md's table and `primase-data.example.toml` agree with the registry. Names are whole string
//!   literals, so the scan sees every one: a name a format string builds (`PRIMASE_{x}`) fails.
//! - `parse_modules_are_checked`: in the crates `architecture.toml` opts in as `parse_crate`, every module that
//!   decodes bytes (little-endian reads) denies unchecked arithmetic, indexing and truncating casts, so bytes a file
//!   says are there and are not are its error, never a panic or a wrapped size.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn manifest(path: &Path) -> toml::Table {
    toml::from_str(&read(path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A workspace member: its path relative to the root (`worlds/gta`), package name, and for a `worlds` crate what
/// its manifest's `[package.metadata.primase]` says it is.
#[derive(Clone, Debug)]
struct Member {
    path: String,
    name: String,
    /// `game` (a game or a game family: it reads), `engine` (an engine family's shared formats) or `destination`
    /// (a target it prepares for and writes).
    kind: Option<String>,
    /// The game, engine and destination names its code may use.
    names: Vec<String>,
    /// The engine crate (package name) a game or destination crate may depend on.
    engine: Option<String>,
}

impl Member {
    fn layer(&self) -> &str {
        self.path.split('/').next().unwrap()
    }
    /// A library crate: holds readers, passes, writers or mechanisms (everything below the cli but the dev-only
    /// test-data crate).
    fn library(&self) -> bool {
        matches!(self.layer(), "core" | "toolkit" | "formats" | "worlds") && self.path != "toolkit/testdata"
    }
}

fn members() -> Vec<Member> {
    let root = root();
    let ws = manifest(&root.join("Cargo.toml"));
    ws["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let path = m.as_str().unwrap().to_owned();
            let man = manifest(&root.join(&path).join("Cargo.toml"));
            let package = &man["package"];
            let name = package["name"].as_str().unwrap().to_owned();
            let meta = package.get("metadata").and_then(|m| m.get("primase"));
            let text = |k: &str| {
                meta.and_then(|m| m.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            };
            let names = meta
                .and_then(|m| m.get("names"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let (kind, engine) = (text("kind"), text("engine"));
            if path.starts_with("worlds/") {
                assert!(
                    matches!(kind.as_deref(), Some("game" | "engine" | "destination")),
                    "{path}: [package.metadata.primase] kind is game, engine or destination"
                );
                assert!(
                    kind.as_deref() != Some("engine") || engine.is_none(),
                    "{path}: an engine crate names no engine of its own"
                );
            }
            Member {
                path,
                name,
                kind,
                names,
                engine,
            }
        })
        .collect()
}

/// The exceptions file.
fn exceptions() -> toml::Table {
    manifest(&root().join("architecture.toml"))
}

/// The entries of one exception table, each as its string fields; every entry must give a `reason`.
fn entries(table: &str) -> Vec<BTreeMap<String, String>> {
    let all = exceptions();
    let Some(list) = all.get(table) else {
        return Vec::new();
    };
    list.as_array()
        .unwrap_or_else(|| panic!("architecture.toml: {table} is not a list of tables"))
        .iter()
        .map(|e| {
            let e: BTreeMap<String, String> = e
                .as_table()
                .unwrap()
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .unwrap_or_else(|| panic!("{table}.{k}: not a string"))
                            .to_owned(),
                    )
                })
                .collect();
            assert!(
                e.get("reason").is_some_and(|r| !r.is_empty()),
                "architecture.toml: a {table} entry without a reason: {e:?}"
            );
            e
        })
        .collect()
}

/// Compare what a check found with its exceptions: (violations no exception covers, exceptions nothing matches).
fn reconcile(found: &BTreeSet<String>, excused: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    (
        found.difference(excused).cloned().collect(),
        excused.difference(found).cloned().collect(),
    )
}

fn assert_reconciled(check: &str, found: BTreeSet<String>, excused: BTreeSet<String>) {
    let (new, stale) = reconcile(&found, &excused);
    let mut msg = String::new();
    for v in &new {
        msg += &format!("\n  violation: {v}");
    }
    for v in &stale {
        msg += &format!("\n  stale exception (no longer occurs; delete it from architecture.toml): {v}");
    }
    assert!(msg.is_empty(), "{check}:{msg}");
}

/// The `.rs` files under `dir`, sorted, as paths relative to the root with forward slashes.
fn rust_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries {
            let p = e.unwrap().path();
            if p.is_dir() {
                pending.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(rel(&p));
            }
        }
    }
    out.sort();
    out
}

fn rel(p: &Path) -> String {
    p.strip_prefix(root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

// ---- a small Rust lexer: identifiers, string literal contents and punctuation, comments dropped ----

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

fn lex(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && at(i + 1) == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == Some('*') {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if c[i] == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if let Some((text, next)) = raw_string(&c, i) {
            out.push(Tok::Str(text));
            i = next;
        } else if ch == '"' || (ch == 'b' && at(i + 1) == Some('"')) {
            let mut j = if ch == 'b' { i + 2 } else { i + 1 };
            let start = j;
            while j < c.len() && c[j] != '"' {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            out.push(Tok::Str(c[start..j.min(c.len())].iter().collect()));
            i = j + 1;
        } else if ch == '\'' || (ch == 'b' && at(i + 1) == Some('\'')) {
            let j = if ch == 'b' { i + 1 } else { i };
            if at(j + 1) == Some('\\') {
                // an escaped character literal
                let mut k = j + 2;
                while k < c.len() && c[k] != '\'' {
                    k += 1;
                }
                i = k + 1;
            } else if at(j + 2) == Some('\'') {
                i = j + 3;
            } else {
                // a lifetime or label
                out.push(Tok::Punct('\''));
                i = j + 1;
            }
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(c[start..i].iter().collect()));
        } else if ch.is_ascii_digit() {
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
        } else {
            out.push(Tok::Punct(ch));
            i += 1;
        }
    }
    out
}

/// A raw string (`r"..."`, `r#"..."#`, `br"..."`) starting at `i`: its content and the index after it.
fn raw_string(c: &[char], i: usize) -> Option<(String, usize)> {
    let mut j = i;
    if c.get(j) == Some(&'b') {
        j += 1;
    }
    if c.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0;
    while c.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    if c.get(j) != Some(&'"') {
        return None;
    }
    let start = j + 1;
    let mut k = start;
    while k < c.len() && !(c[k] == '"' && (0..hashes).all(|h| c.get(k + 1 + h) == Some(&'#'))) {
        k += 1;
    }
    Some((c[start..k.min(c.len())].iter().collect(), k + 1 + hashes))
}

fn ident(t: &Tok, s: &str) -> bool {
    matches!(t, Tok::Ident(x) if x == s)
}

fn punct(t: &Tok, p: char) -> bool {
    *t == Tok::Punct(p)
}

/// The index after the brace-delimited block opening at `open` (a `{`).
fn block_end(t: &[Tok], open: usize) -> usize {
    let mut depth = 0;
    for (k, tok) in t.iter().enumerate().skip(open) {
        if punct(tok, '{') {
            depth += 1;
        } else if punct(tok, '}') {
            depth -= 1;
            if depth == 0 {
                return k + 1;
            }
        }
    }
    t.len()
}

/// The tokens without items marked `#[cfg(test)]` (unit tests may do what library code may not).
fn without_tests(t: Vec<Tok>) -> Vec<Tok> {
    let mut out = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let is_cfg_test = i + 6 < t.len()
            && punct(&t[i], '#')
            && punct(&t[i + 1], '[')
            && ident(&t[i + 2], "cfg")
            && punct(&t[i + 3], '(')
            && ident(&t[i + 4], "test")
            && punct(&t[i + 5], ')')
            && punct(&t[i + 6], ']');
        if !is_cfg_test {
            out.push(t[i].clone());
            i += 1;
            continue;
        }
        let mut j = i + 7;
        while j < t.len() && !punct(&t[j], '{') && !punct(&t[j], ';') {
            j += 1;
        }
        i = if j < t.len() && punct(&t[j], '{') {
            block_end(&t, j)
        } else {
            j + 1
        };
    }
    out
}

fn code(path: &str) -> Vec<Tok> {
    without_tests(lex(&read(&root().join(path))))
}

// ---- dependencies ----

/// The test-input registry: `toolkit/testdata` resolves and declares the real-data inputs tests read. It may name
/// games and third-party tools (each game's inputs are described there, so `game_names` skips it), holds no game
/// logic (it depends on no workspace crate) and may not be a normal dependency of any crate, so nothing it says
/// reaches a library build. `dependencies` checks both, and `architecture.toml` cannot excuse either.
const TEST_INPUT_REGISTRY: &str = "toolkit/testdata";

/// Whether `from` may depend on `to`; `dev` for dev-dependencies. A game or destination crate may depend on the
/// engine crate it names, never the other way round.
fn edge_allowed(from: &Member, to: &Member, dev: bool) -> bool {
    let to_layer = to.layer();
    if to.path == TEST_INPUT_REGISTRY {
        return dev;
    }
    let normal = match from.layer() {
        "core" => false,
        "toolkit" => matches!(to_layer, "core" | "toolkit"),
        "formats" => matches!(to_layer, "core" | "toolkit") || to.path == "formats/gltf",
        "worlds" => {
            matches!(to_layer, "core" | "toolkit")
                || (to.kind.as_deref() == Some("engine") && from.engine.as_deref() == Some(to.name.as_str()))
        }
        "cli" => true,
        other => panic!("{}: unknown layer {other}", from.path),
    };
    normal || (dev && from.layer() != "core" && to_layer == "formats")
}

#[test]
fn dependencies() {
    let members = members();
    let by_name: BTreeMap<&str, &Member> = members.iter().map(|m| (m.name.as_str(), m)).collect();
    let mut found = BTreeSet::new();
    for m in &members {
        let man = manifest(&root().join(&m.path).join("Cargo.toml"));
        for (section, dev) in [
            ("dependencies", false),
            ("build-dependencies", false),
            ("dev-dependencies", true),
        ] {
            let Some(deps) = man.get(section).and_then(|d| d.as_table()) else {
                continue;
            };
            for (key, spec) in deps {
                let package = spec.get("package").and_then(|p| p.as_str()).unwrap_or(key);
                let Some(to) = by_name.get(package) else { continue };
                assert!(
                    m.path != TEST_INPUT_REGISTRY,
                    "{} depends on {}: the test-input registry depends on no workspace crate",
                    m.path,
                    to.path
                );
                assert!(
                    to.path != TEST_INPUT_REGISTRY || dev,
                    "{} has {} in [{section}]: the test-input registry is only ever a dev-dependency",
                    m.path,
                    to.path
                );
                if !edge_allowed(m, to, dev) {
                    found.insert(format!("{} -> {} ({section})", m.path, to.path));
                }
            }
        }
        assert!(
            man.get("target").is_none(),
            "{}: target-specific dependencies are not checked yet; extend this test",
            m.path
        );
    }
    let excused = entries("dependency")
        .iter()
        .map(|e| format!("{} -> {} ({})", e["from"], e["to"], e["section"]))
        .collect();
    assert_reconciled("crate dependencies", found, excused);
}

// ---- file, environment and process access ----

const DENY_IO: &str = "#![deny(clippy::disallowed_methods, clippy::disallowed_types)]";
const IO_ROLES: &[&str] = &["reader I/O", "writer I/O", "toolkit I/O", "test I/O", "exception"];

/// The I/O roles a crate of `layer` has: readers and writers are formats' and worlds', mechanisms the toolkit's,
/// and test I/O anyone's test code. `exception` is a break, listed in `architecture.toml`.
fn io_roles_of(layer: &str) -> &'static [&'static str] {
    match layer {
        "formats" | "worlds" => &["reader I/O", "writer I/O", "test I/O"],
        "toolkit" => &["toolkit I/O", "test I/O"],
        _ => &["test I/O"],
    }
}

/// The roles named by the `allow(clippy::disallowed_*)` attributes of a source text.
fn io_opt_ins(text: &str) -> Vec<String> {
    let mut roles = Vec::new();
    let mut rest = text;
    while let Some(p) = rest.find("allow(") {
        let attr_start = &rest[p..];
        let end = attr_start.find(")]").map_or(attr_start.len(), |e| e + 2);
        let attr = &attr_start[..end];
        if attr.contains("clippy::disallowed_") {
            let role = attr
                .split_once("reason = \"")
                .and_then(|(_, r)| r.split_once(':'))
                .map_or_else(|| "(no reason)".to_owned(), |(role, _)| role.to_owned());
            roles.push(role);
        }
        rest = &attr_start[end..];
    }
    roles
}

#[test]
fn io_follows_roles() {
    let mut found = BTreeSet::new();
    let mut misplaced = Vec::new();
    for m in members().iter().filter(|m| m.library()) {
        let lib = format!("{}/src/lib.rs", m.path);
        assert!(
            read(&root().join(&lib)).contains(DENY_IO),
            "{lib}: a library crate starts with {DENY_IO} (file, environment and process access only in modules that \
             opt in with their role)"
        );
        for file in rust_files(&root().join(&m.path).join("src")) {
            let text = read(&root().join(&file));
            for role in io_opt_ins(&text) {
                assert!(
                    IO_ROLES.contains(&role.as_str()),
                    "{file}: an I/O opt-in's reason starts with one of {IO_ROLES:?} and a colon, not '{role}'"
                );
                if role == "exception" {
                    found.insert(format!("{file}: {role}"));
                } else if !io_roles_of(m.layer()).contains(&role.as_str()) {
                    misplaced.push(format!("{file}: {role} in a {} crate", m.layer()));
                }
            }
            // test I/O only in test code: its reason survives in the code without the tests only if it is outside them
            let outside = without_tests(lex(&text))
                .iter()
                .any(|t| matches!(t, Tok::Str(r) if r.starts_with("test I/O:")));
            if outside {
                misplaced.push(format!("{file}: test I/O outside test code"));
            }
        }
    }
    assert!(
        misplaced.is_empty(),
        "I/O opt-ins with a role their crate doesn't have: {misplaced:#?}"
    );
    let listed = entries("io");
    let declared: Vec<_> = listed.iter().filter(|e| e["role"] != "exception").collect();
    assert!(
        declared.is_empty(),
        "architecture.toml lists I/O roles, which are a rule by crate now: delete {declared:?}"
    );
    let excused = listed
        .iter()
        .map(|e| format!("{}: {}", e["file"], e["role"]))
        .collect();
    assert_reconciled("I/O breaks", found, excused);
}

// ---- parse modules ----

/// The inner attribute a parse module starts with, whitespace taken out.
const DENY_PARSE: &str =
    "#![deny(clippy::arithmetic_side_effects,clippy::indexing_slicing,clippy::cast_possible_truncation";

/// Whether a module's code decodes bytes: it reads little-endian values (`from_le_bytes`, `from_be_bytes`, or the
/// crate's `le::` reads).
fn decodes_bytes(t: &[Tok]) -> bool {
    t.iter().enumerate().any(|(i, tok)| {
        ident(tok, "from_le_bytes")
            || ident(tok, "from_be_bytes")
            || (ident(tok, "le") && t.get(i + 1).is_some_and(|n| punct(n, ':')))
    })
}

#[test]
fn parse_modules_are_checked() {
    let mut found = BTreeSet::new();
    for c in entries("parse_crate") {
        let path = &c["path"];
        let files = rust_files(&root().join(path).join("src"));
        assert!(!files.is_empty(), "parse_crate {path}: no sources");
        for file in files {
            let squeezed: String = read(&root().join(&file)).split_whitespace().collect();
            if decodes_bytes(&code(&file)) && !squeezed.contains(DENY_PARSE) {
                found.insert(file);
            }
        }
    }
    let excused = entries("parse").into_iter().map(|e| e["file"].clone()).collect();
    assert_reconciled("parse modules", found, excused);
}

// ---- game and engine names ----

const NAMES: &[&str] = &[
    "sekiro",
    "fromsoft",
    "elden",
    "gta",
    "rage",
    "unity",
    "forza",
    "skate",
    "reskate",
    "studio",
    "frostbite",
    "insomniac",
    "sunset",
];

/// The words of an identifier or string: split at non-alphanumerics, underscores and lower-to-upper case changes.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if !ch.is_alphanumeric() {
            out.push(std::mem::take(&mut word));
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower {
            out.push(std::mem::take(&mut word));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        word.extend(ch.to_lowercase());
    }
    out.push(word);
    out.retain(|w| !w.is_empty());
    out
}

#[test]
fn game_names() {
    let mut found = BTreeSet::new();
    for m in members()
        .iter()
        .filter(|m| matches!(m.layer(), "core" | "toolkit" | "formats" | "worlds"))
        // the test-input registry describes each game's inputs by role (see TEST_INPUT_REGISTRY)
        .filter(|m| m.path != TEST_INPUT_REGISTRY)
    {
        // a worlds crate's own names (its games, its engine, its destination); shared code has none
        for file in rust_files(&root().join(&m.path).join("src")) {
            for t in code(&file) {
                let (Tok::Ident(s) | Tok::Str(s)) = t else {
                    continue;
                };
                for w in words(&s) {
                    if NAMES.contains(&w.as_str()) && !m.names.contains(&w) {
                        found.insert(format!("{}: {w}", m.path));
                    }
                }
            }
        }
    }
    let excused = entries("name")
        .iter()
        .map(|e| format!("{}: {}", e["crate"], e["word"]))
        .collect();
    assert_reconciled("game and engine names in shared code", found, excused);
}

// ---- entry points only in examples, bins and the cli ----

/// What composition code may not do, as token patterns, and how to say it.
fn data_logic(t: &[Tok]) -> BTreeSet<&'static str> {
    let mut out = BTreeSet::new();
    for (i, tok) in t.iter().enumerate() {
        let next = |k: usize| t.get(i + k);
        let turbofish =
            |k: usize| next(k).is_some_and(|x| punct(x, ':')) && next(k + 1).is_some_and(|x| punct(x, ':'));
        match tok {
            Tok::Punct('.') => {
                if let Some(Tok::Ident(m)) = next(1) {
                    if matches!(m.as_str(), "iter" | "get" | "get_mut" | "remove" | "has") && turbofish(2) {
                        out.insert("queries or edits world components");
                    }
                    if matches!(m.as_str(), "spawn" | "insert_resource" | "remove_resource") {
                        out.insert("queries or edits world components");
                    }
                    // `pass.run(world, log)`: two arguments, a pass run past the manager (the manager's own `run`
                    // takes the world, the pipeline and the report)
                    if m == "run" && next(2).is_some_and(|x| punct(x, '(')) && arguments(t, i + 2) == 2 {
                        out.insert("runs a pass outside the pass manager");
                    }
                }
            }
            Tok::Ident(s) if s == "impl" => {
                let mut j = i + 1;
                while j < t.len() && !punct(&t[j], '{') && !punct(&t[j], ';') {
                    if (ident(&t[j], "Pass") || ident(&t[j], "Component"))
                        && t.get(j + 1).is_some_and(|x| ident(x, "for"))
                    {
                        out.insert("defines a pass or component");
                    }
                    j += 1;
                }
            }
            Tok::Ident(s) if matches!(s.as_str(), "read_exact" | "from_glb" | "read_png" | "File") => {
                out.insert("reads a format itself");
            }
            _ => {}
        }
    }
    out
}

/// The number of top-level arguments of the call whose `(` is at `open`.
fn arguments(t: &[Tok], open: usize) -> usize {
    let mut depth = 0;
    let mut commas = 0;
    let mut empty = true;
    for tok in &t[open..] {
        match tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Tok::Punct(',') if depth == 1 => commas += 1,
            _ => {}
        }
        if depth >= 1 && !punct(tok, '(') {
            empty = false;
        }
    }
    if empty { 0 } else { commas + 1 }
}

#[test]
fn entry_points() {
    let mut files = Vec::new();
    for m in members() {
        let dir = root().join(&m.path);
        files.extend(rust_files(&dir.join("examples")));
        files.extend(rust_files(&dir.join("src").join("bin")));
        if m.layer() == "cli" {
            files.extend(rust_files(&dir.join("src")));
        }
    }
    let mut found = BTreeSet::new();
    for file in files {
        for what in data_logic(&code(&file)) {
            found.insert(format!("{file}: {what}"));
        }
    }
    let excused = entries("entry_point")
        .iter()
        .map(|e| format!("{}: {}", e["file"], e["what"]))
        .collect();
    assert_reconciled("composition without data logic", found, excused);
}

// ---- passes are registered and lowerings are passes ----

/// For each `impl ... Pass for` block: its token range and its `fn name` string.
fn pass_impls(t: &[Tok]) -> Vec<(std::ops::Range<usize>, Option<String>)> {
    let mut out = Vec::new();
    for i in 0..t.len() {
        if !ident(&t[i], "impl") {
            continue;
        }
        let mut j = i + 1;
        let mut is_pass = false;
        while j < t.len() && !punct(&t[j], '{') && !punct(&t[j], ';') {
            is_pass |= ident(&t[j], "Pass") && t.get(j + 1).is_some_and(|x| ident(x, "for"));
            j += 1;
        }
        if !is_pass || j >= t.len() || !punct(&t[j], '{') {
            continue;
        }
        let end = block_end(t, j);
        let name = (j..end)
            .find(|&k| ident(&t[k], "fn") && t.get(k + 1).is_some_and(|x| ident(x, "name")))
            .and_then(|k| {
                t[k..end].iter().find_map(|x| {
                    if let Tok::Str(s) = x {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
            });
        out.push((j..end, name));
    }
    out
}

/// The names given to `register("...", ...)` and `register_requiring("...", ...)` calls.
fn registered(t: &[Tok]) -> BTreeSet<String> {
    t.windows(3)
        .filter_map(|w| match (&w[0], &w[1], &w[2]) {
            (Tok::Ident(f), Tok::Punct('('), Tok::Str(s)) if f == "register" || f == "register_requiring" => {
                Some(s.clone())
            }
            _ => None,
        })
        .collect()
}

/// Public functions whose signature returns a `PassLog`: their names.
fn lowerings(t: &[Tok]) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..t.len().saturating_sub(2) {
        if !(ident(&t[i], "pub") && ident(&t[i + 1], "fn")) {
            continue;
        }
        let Some(Tok::Ident(name)) = t.get(i + 2) else {
            continue;
        };
        let mut j = i + 3;
        let mut arrow = false;
        let mut returns_log = false;
        while j < t.len() && !punct(&t[j], '{') && !punct(&t[j], ';') {
            arrow |= punct(&t[j], '>') && punct(&t[j - 1], '-');
            returns_log |= arrow && ident(&t[j], "PassLog");
            j += 1;
        }
        if returns_log {
            out.push(name.clone());
        }
    }
    out
}

#[test]
fn passes_registered() {
    let members = members();
    let mut registry = BTreeSet::new();
    let mut crates = Vec::new();
    for m in &members {
        let files: Vec<(String, Vec<Tok>)> = rust_files(&root().join(&m.path).join("src"))
            .into_iter()
            .map(|f| {
                let t = code(&f);
                (f, t)
            })
            .collect();
        for (_, t) in &files {
            registry.extend(registered(t));
        }
        crates.push((m.clone(), files));
    }
    let mut found = BTreeSet::new();
    for (m, files) in &crates {
        let mut run_by_passes = BTreeSet::new();
        for (file, t) in files {
            for (range, name) in pass_impls(t) {
                for k in range.clone() {
                    if let (Tok::Ident(f), Some(Tok::Punct('('))) = (&t[k], t.get(k + 1)) {
                        run_by_passes.insert(f.clone());
                    }
                }
                match name {
                    Some(n) if registry.contains(&n) => {}
                    Some(n) => {
                        found.insert(format!("{file}: pass {n} is not registered"));
                    }
                    None => {
                        found.insert(format!("{file}: a pass without a literal name"));
                    }
                }
            }
        }
        for (file, t) in files {
            for f in lowerings(t) {
                if !run_by_passes.contains(&f) {
                    found.insert(format!(
                        "{file}: {f} returns a PassLog but no pass of {} runs it",
                        m.path
                    ));
                }
            }
        }
    }
    let excused = entries("pass")
        .iter()
        .map(|e| format!("{}: {}", e["file"], e["what"]))
        .collect();
    assert_reconciled("passes registered, lowerings run as passes", found, excused);
}

// ---- writers take the world read-only ----

#[test]
fn writers_read_only() {
    let mut found = BTreeSet::new();
    for m in members() {
        if !(m.layer() == "formats" || m.path == "worlds/reskate") {
            continue;
        }
        for file in rust_files(&root().join(&m.path).join("src")) {
            let t = code(&file);
            for i in 0..t.len().saturating_sub(2) {
                if !(ident(&t[i], "pub") && ident(&t[i + 1], "fn")) {
                    continue;
                }
                let Some(Tok::Ident(name)) = t.get(i + 2) else {
                    continue;
                };
                if !(name.contains("write") || name.contains("prepare")) {
                    continue;
                }
                let mut j = i + 3;
                while j < t.len() && !punct(&t[j], '{') && !punct(&t[j], ';') {
                    if punct(&t[j], '&')
                        && t.get(j + 1).is_some_and(|x| ident(x, "mut"))
                        && t.get(j + 2).is_some_and(|x| ident(x, "World"))
                    {
                        found.insert(format!("{file}: {name} takes &mut World"));
                    }
                    j += 1;
                }
            }
        }
    }
    let excused = entries("writer")
        .iter()
        .map(|e| format!("{}: {}", e["file"], e["what"]))
        .collect();
    assert_reconciled("writers take the world read-only", found, excused);
}

// ---- the checks' own tools ----

#[test]
fn lexer_drops_comments_and_tests() {
    let t = code_of(
        "fn a() { let s = \"x // y\"; } // gone\n/* gone */ #[cfg(test)] mod tests { fn hidden() {} }\nfn b<'a>(c: char) { let _ = '\\''; let r = r#\"raw\"#; }",
    );
    let idents: Vec<_> = t
        .iter()
        .filter_map(|x| {
            if let Tok::Ident(s) = x {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        idents,
        [
            "fn", "a", "let", "s", "fn", "b", "a", "c", "char", "let", "_", "let", "r"
        ]
    );
    let strs: Vec<_> = t
        .iter()
        .filter_map(|x| {
            if let Tok::Str(s) = x {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(strs, ["x // y", "raw"]);
    assert_eq!(
        words("fromsoft_path GtaShader skateFNV"),
        ["fromsoft", "path", "gta", "shader", "skate", "fnv"]
    );
}

fn code_of(src: &str) -> Vec<Tok> {
    without_tests(lex(src))
}

// ---- real-data variables ----

/// Every `"PRIMASE_..."` string literal's name in a source text, and every name a format string builds
/// (`PRIMASE_{x}`, `PRIMASE_GTA_{}` in a literal), which the scan could not check.
fn variable_literals(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut names, mut built) = (Vec::new(), Vec::new());
    let mut rest = text;
    while let Some(p) = rest.find("\"PRIMASE_") {
        let tail = &rest[p + 1..];
        let end = tail
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(tail.len());
        if tail[end..].starts_with('"') && end > "PRIMASE_".len() {
            names.push(tail[..end].to_owned());
        } else if tail[end..].starts_with('{') {
            built.push(tail[..end].to_owned());
        }
        rest = &tail[end..];
    }
    (names, built)
}

#[test]
fn variable_literals_see_built_names() {
    // assembled, so this file's own text holds no such literal
    let q = '"';
    let src =
        format!("real_data({q}PRIMASE_GTA_DIR{q}); format!({q}PRIMASE_{{x}}{q}); {q}PRIMASE_* names{q}");
    let (names, built) = variable_literals(&src);
    assert_eq!(names, ["PRIMASE_GTA_DIR"]);
    assert_eq!(built, ["PRIMASE_"]);
}

#[test]
fn real_data_variables_are_declared() {
    use primase_testdata::{Kind, VARIABLES};
    let declared: BTreeSet<&str> = VARIABLES.iter().map(|v| v.name).collect();
    // names read by sources: every member crate's .rs files
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let mut built = Vec::new();
    for m in members() {
        // the registry itself names every variable: it declares, it does not read
        for file in rust_files(&root().join(&m.path))
            .into_iter()
            .filter(|f| f != "toolkit/testdata/src/inputs.rs")
        {
            let (names, templates) = variable_literals(&read(&root().join(&file)));
            for name in names {
                found.entry(name).or_insert_with(|| file.clone());
            }
            built.extend(templates.into_iter().map(|t| format!("{t} ({file})")));
        }
    }
    assert!(
        built.is_empty(),
        "PRIMASE_* names a format string completes, which the registry check cannot see (write each name as a literal): {built:?}"
    );
    let undeclared: Vec<String> = found
        .iter()
        .filter(|(n, _)| !declared.contains(n.as_str()))
        .map(|(n, f)| format!("{n} ({f})"))
        .collect();
    assert!(
        undeclared.is_empty(),
        "PRIMASE_* names read but not declared in toolkit/testdata/src/inputs.rs: {undeclared:?}"
    );
    let stale: Vec<&str> = declared
        .iter()
        .filter(|n| !found.contains_key(**n))
        .copied()
        .collect();
    assert!(
        stale.is_empty(),
        "declared but read nowhere (remove them): {stale:?}"
    );

    // CONTRIBUTING.md's table, row by row, is the registry without the test-internal kinds; `-` stands for no source
    let contributing = read(&root().join("CONTRIBUTING.md"));
    let rows: Vec<[String; 5]> = contributing
        .lines()
        .filter_map(|l| l.strip_prefix("| `PRIMASE_"))
        .map(|l| {
            let mut cells = l.trim_end_matches(" |").splitn(5, " | ").map(str::to_owned);
            let mut cell = || cells.next().unwrap_or_default();
            let name = String::from("PRIMASE_") + cell().trim_end_matches('`');
            [name, cell(), cell(), cell(), cell()]
        })
        .collect();
    let want: Vec<[String; 5]> = VARIABLES
        .iter()
        .filter(|v| !matches!(v.kind, Kind::Test | Kind::Child))
        .map(|v| {
            [
                v.name.to_owned(),
                format!("{:?}", v.kind).to_lowercase(),
                format!("{:?}", v.status).to_lowercase(),
                v.about.to_owned(),
                if v.source.is_empty() { "-" } else { v.source }.to_owned(),
            ]
        })
        .collect();
    assert_eq!(
        rows, want,
        "CONTRIBUTING.md's real-data table differs from the registry"
    );

    // the example configuration only names declared variables a person sets
    let example = read(&root().join("primase-data.example.toml"));
    for line in example.lines().filter(|l| l.starts_with("PRIMASE_")) {
        let name = line.split_whitespace().next().unwrap();
        let v = primase_testdata::declared(name)
            .unwrap_or_else(|| panic!("primase-data.example.toml names undeclared {name}"));
        assert!(
            !matches!(v.kind, Kind::Test | Kind::Child | Kind::Runtime),
            "primase-data.example.toml sets {name}, which a person does not set"
        );
    }
}

/// Every break there was, by identity (its table and every field but `reason` and `removed_when`). The list only
/// shrinks: when a break goes, delete its identity here too.
const KNOWN_BREAKS: &[&str] = &[
    "dependency: from=worlds/reskate section=dependencies to=formats/gltf",
    "entry_point: file=cli/examples/scene_to_tiles.rs what=queries or edits world components",
    "entry_point: file=cli/examples/scene_to_tiles.rs what=runs a pass outside the pass manager",
    "entry_point: file=formats/web/examples/ktx2_audit.rs what=reads a format itself",
    "entry_point: file=formats/web/examples/ktx2_compare.rs what=reads a format itself",
    "entry_point: file=formats/web/examples/synthetic_tiles.rs what=queries or edits world components",
    "io: file=formats/gltf/src/write.rs role=exception",
    "io: file=worlds/reskate/src/postcompile/vblend.rs role=exception",
    "io: file=worlds/reskate/src/postcompile/vbrecord.rs role=exception",
];

/// A break's identity: its table and every field but `reason` and `removed_when`, sorted.
fn identity(table: &str, e: &toml::Table) -> String {
    let mut fields: Vec<String> = e
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "reason" | "removed_when"))
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
        .collect();
    fields.sort();
    format!("{table}: {}", fields.join(" "))
}

/// Check a parsed `architecture.toml` against the known breaks: every entry but the `parse_crate` opt-ins is a break
/// with its `removed_when`, known by identity; a known one that went must leave the list.
fn check_breaks(all: &toml::Table, known: &[&str]) -> Result<(), String> {
    let mut present = BTreeSet::new();
    let mut new = Vec::new();
    for (table, list) in all {
        if table == "parse_crate" {
            continue;
        }
        for e in list.as_array().into_iter().flatten() {
            let e = e.as_table().unwrap();
            let id = identity(table, e);
            if !e
                .get("removed_when")
                .and_then(|v| v.as_str())
                .is_some_and(|w| !w.is_empty())
            {
                return Err(format!("{id}: a break says what removes it (removed_when)"));
            }
            if known.contains(&id.as_str()) {
                present.insert(id);
            } else {
                new.push(id);
            }
        }
    }
    if !new.is_empty() {
        return Err(format!(
            "breaks that are new, or replace a known one (fix the code instead): {new:#?}"
        ));
    }
    let gone: Vec<&&str> = known.iter().filter(|g| !present.contains(**g)).collect();
    if !gone.is_empty() {
        return Err(format!("breaks gone: delete them from KNOWN_BREAKS: {gone:#?}"));
    }
    Ok(())
}

#[test]
fn breaks_only_go() {
    if let Err(e) = check_breaks(&exceptions(), KNOWN_BREAKS) {
        panic!("architecture.toml: {e}");
    }
}

/// The ratchet counts identities, not entries: a known break replaced by another at the same count fails, as does
/// one with no `removed_when`, and one that went must leave the known list.
#[test]
fn a_replaced_break_is_a_new_one() {
    let file = |entries: &str| -> toml::Table { toml::from_str(entries).unwrap() };
    let known = "pass: file=a.rs what=x";
    let same = file("[[pass]]\nfile = \"a.rs\"\nwhat = \"x\"\nreason = \"r\"\nremoved_when = \"w\"\n");
    assert!(check_breaks(&same, &[known]).is_ok());
    let replaced = file("[[pass]]\nfile = \"b.rs\"\nwhat = \"x\"\nreason = \"r\"\nremoved_when = \"w\"\n");
    let e = check_breaks(&replaced, &[known]).unwrap_err();
    assert!(e.contains("new") && e.contains("b.rs"), "{e}");
    let forever = file("[[pass]]\nfile = \"a.rs\"\nwhat = \"x\"\nreason = \"r\"\n");
    assert!(
        check_breaks(&forever, &[known])
            .unwrap_err()
            .contains("removed_when")
    );
    assert!(check_breaks(&file(""), &[known]).unwrap_err().contains("gone"));
}
