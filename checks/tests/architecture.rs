//! The role and layer rules of CONTRIBUTING.md ("Roles", "Layout"), checked over the workspace's manifests and
//! sources.
//!
//! Readers read, writers write, and what breaks a rule today is listed in `architecture.toml` as a break to remove,
//! each with its reason and what removes it (`removed_when`). Each check collects every violation, removes the listed
//! breaks, and fails on what is left; it also fails on a break that no longer occurs. The breaks only go:
//! `breaks_only_go` holds the identity of every one there was, so a new or replaced break fails.
//!
//! - `dependencies`: crate edges per layer (`core`, `toolkit`, `formats`, `worlds`; `checks` and `examples` hold no library). A `worlds` crate depends
//!   on the toolkit and core, and a game or destination crate also on the engine crate it names; an engine crate on
//!   no game, destination or recipe.
//! - `io_follows_roles`: every library crate denies `clippy::disallowed_methods`/`disallowed_types` (file,
//!   environment and process access, listed in `clippy.toml`), and every module that opts back in names a role its
//!   crate has: reader and writer I/O in `formats` and `worlds`, toolkit I/O in the toolkit, test I/O only in test
//!   code.
//! - `worlds_layout`: a worlds crate is arranged by role: reader I/O only under `read/`, writer I/O only under
//!   `write/`, a later stage's I/O under its own folder (`[package.metadata.primase] stages`); allowances cover no
//!   module beneath them; passes, components and other modules name no I/O path (any module that opts in to I/O, a
//!   `read`/`write`/stage module, or a re-export of one). A guardrail by name, not a proof. A module not yet arranged
//!   carries a migration status in `architecture.toml` (`[[layout]]`, per module, counted like a break). The rules
//!   are checked on `layout_rules`' and `layout_paths`' fixtures.
//! - `game_names`: no game, engine or destination names in core, toolkit and formats code (the test-input registry
//!   aside); a `worlds` crate uses only its own (`[package.metadata.primase] names`). A game crate's source names
//!   that hold another crate's word (SkateBIRD's script class `SkateCollectible`) are declared in its manifest
//!   (`source-names`), each matched as a whole string literal and each used.
//! - `entry_points`: examples and bins compose library entry points; no world data or format handling of their own.
//! - `passes_registered`: every pass is registered by name, and every public lowering returning a `PassLog` is run
//!   by a pass.
//! - `writers_read_only`: a writer's entry points take the world read-only.
//! - `real_data_variables_are_declared`: every `PRIMASE_*` name in the workspace's sources is declared in the
//!   real-data registry (`toolkit/testdata/src/inputs.rs`), every declared name is read somewhere, and
//!   CONTRIBUTING.md's table and `primase-data.example.toml` agree with the registry. Names are whole string
//!   literals, so the scan sees every one: a name a format string builds (`PRIMASE_{x}`) fails.
//! - `component_geometry_hooks`: every component's stated geometry (`const GEOMETRY`) matches the remap hooks it
//!   implements, in every crate, tests included.
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
    /// A game crate's source names (whole string literals, as its game stores them) that hold another crate's word.
    source_names: Vec<String>,
    /// The engine crate (package name) a game or destination crate may depend on.
    engine: Option<String>,
    /// The later stages a worlds crate holds, each in its own folder (`postcompile`): a stage reads and writes its
    /// own artifacts.
    stages: Vec<String>,
}

impl Member {
    fn layer(&self) -> &str {
        self.path.split('/').next().unwrap()
    }
    /// A library crate: holds readers, passes, writers or mechanisms (not `checks` or `examples`, nor the dev-only
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
            let list = |k: &str| -> Vec<String> {
                meta.and_then(|m| m.get(k))
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default()
            };
            let (names, stages, source_names) = (list("names"), list("stages"), list("source-names"));
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
            // any crate: only a game crate's source names are exempt
            assert!(
                kind.as_deref() == Some("game") || source_names.is_empty(),
                "{path}: only a game crate declares source names"
            );
            Member {
                path,
                name,
                kind,
                names,
                source_names,
                engine,
                stages,
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
        // they hold no library: tests and example programs may compose anything
        "checks" | "examples" => true,
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

// ---- worlds crate layout ----

/// Where a file of a worlds crate sits, from its path under `src/`: the first folder (or module file) that names a
/// role or one of the crate's stages. Game-first subtrees use the same names (`sekiro/read/msb.rs` is in `read`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    Read,
    Write,
    Passes,
    Components,
    Stage(String),
    /// Elsewhere: the crate root, named format modules and pure codecs.
    Other,
}

fn place(src_path: &str, stages: &[String]) -> Place {
    for part in src_path.split('/') {
        let part = part.strip_suffix(".rs").unwrap_or(part);
        match part {
            "read" => return Place::Read,
            "write" => return Place::Write,
            "passes" => return Place::Passes,
            "components" => return Place::Components,
            s if stages.iter().any(|st| st == s) => return Place::Stage(s.to_owned()),
            _ => {}
        }
    }
    Place::Other
}

/// Whether a file is the root of a role tree, which may re-export its read and pass API for consumers: the crate's
/// `lib.rs`, or a game folder's `mod.rs` (one declaring role modules: `sekiro/mod.rs` with `mod read;`, CONTRIBUTING's
/// `<game>/read`, `<game>/passes`, `<game>/components`).
fn role_root(src_path: &str, tokens: &[Tok]) -> bool {
    src_path == "lib.rs"
        || (src_path.ends_with("/mod.rs")
            && tokens.windows(2).any(|w| {
                ident(&w[0], "mod") && matches!(&w[1], Tok::Ident(n) if matches!(n.as_str(), "read" | "write" | "passes" | "components"))
            }))
}

/// A file's module path in its crate (`["a", "b"]` for `src/a/b.rs` or `src/a/b/mod.rs`; empty for `lib.rs`).
fn module_of(src_path: &str) -> Vec<String> {
    let p = src_path.trim_end_matches(".rs");
    let p = p.strip_suffix("/mod").unwrap_or(p);
    if p == "lib" {
        return Vec::new();
    }
    p.split('/').map(str::to_owned).collect()
}

/// The path starting at `t[i]` (`a::b::c`) and the index after it.
fn path_at(t: &[Tok], i: usize) -> (Vec<String>, usize) {
    let mut segs = Vec::new();
    if let Some(Tok::Ident(first)) = t.get(i) {
        segs.push(first.clone());
    }
    let mut j = i + 1;
    while let (Some(a), Some(b), Some(Tok::Ident(seg))) = (t.get(j), t.get(j + 1), t.get(j + 2)) {
        if !(punct(a, ':') && punct(b, ':')) {
            break;
        }
        segs.push(seg.clone());
        j += 3;
    }
    (segs, j)
}

/// The paths of a `use` tree (`crate::{read::x, y as z, w::*}`), each with the name it brings in (`*` for a glob).
fn use_tree(t: &[Tok], prefix: &[String], out: &mut Vec<(Vec<String>, String)>) {
    let mut segs = prefix.to_vec();
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            Tok::Ident(s) if s == "as" => {
                if let Some(Tok::Ident(name)) = t.get(i + 1) {
                    out.push((segs, name.clone()));
                }
                return;
            }
            Tok::Ident(s) => {
                segs.push(s.clone());
                i += 1;
            }
            Tok::Punct('{') => {
                let end = block_end(t, i).min(t.len());
                let inner = &t[i + 1..end];
                let (mut depth, mut start) = (0i32, 0);
                for (k, tok) in inner.iter().enumerate() {
                    match tok {
                        Tok::Punct('{') => depth += 1,
                        Tok::Punct('}') => depth -= 1,
                        Tok::Punct(',') if depth == 0 => {
                            use_tree(&inner[start..k], &segs, out);
                            start = k + 1;
                        }
                        _ => {}
                    }
                }
                use_tree(&inner[start..], &segs, out);
                return;
            }
            Tok::Punct('*') => {
                out.push((segs, "*".to_owned()));
                return;
            }
            _ => i += 1,
        }
    }
    if segs.len() > prefix.len() {
        if segs.last().is_some_and(|s| s == "self") {
            segs.pop();
        }
        let name = segs.last().cloned().unwrap_or_default();
        out.push((segs, name));
    }
}

/// A `use` item: whether it is a re-export (`pub`, `pub(crate)`, ...), and its paths with the names they bring in.
struct Use {
    public: bool,
    items: Vec<(Vec<String>, String)>,
}

/// The `use` items and the other paths (two segments or more) a token stream names.
fn named(t: &[Tok]) -> (Vec<Use>, Vec<Vec<String>>) {
    let (mut uses, mut paths) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < t.len() {
        if punct(&t[i], '#') {
            // an attribute names lints and paths of its own, not dependencies: skip `#[...]` and `#![...]`
            let open = if t.get(i + 1).is_some_and(|x| punct(x, '!')) {
                i + 2
            } else {
                i + 1
            };
            if t.get(open).is_some_and(|x| punct(x, '[')) {
                let mut depth = 0i32;
                let mut k = open;
                while k < t.len() {
                    if punct(&t[k], '[') {
                        depth += 1;
                    } else if punct(&t[k], ']') {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                i = k + 1;
                continue;
            }
            i += 1;
        } else if ident(&t[i], "use") {
            let public = i > 0
                && (ident(&t[i - 1], "pub") || (punct(&t[i - 1], ')') && i >= 4 && ident(&t[i - 4], "pub")));
            let end = (i..t.len()).find(|&k| punct(&t[k], ';')).unwrap_or(t.len());
            let mut items = Vec::new();
            use_tree(&t[i + 1..end], &[], &mut items);
            uses.push(Use { public, items });
            i = end;
        } else if matches!(t[i], Tok::Ident(_)) {
            let (segs, j) = path_at(t, i);
            if segs.len() > 1 {
                paths.push(segs);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    (uses, paths)
}

/// A path as `crate_ident::...`: `crate`, `self` and `super` resolved against the file's module; a first segment
/// naming another crate (`primase_*`, `std`, `core`, `alloc`) kept; anything else relative to the file's module.
fn absolute(segs: &[String], krate: &str, module: &[String]) -> String {
    let here = || -> Vec<String> {
        std::iter::once(krate.to_owned())
            .chain(module.iter().cloned())
            .collect()
    };
    let (mut base, mut rest) = match segs.first().map(String::as_str) {
        Some("crate") => (vec![krate.to_owned()], &segs[1..]),
        Some("self") => (here(), &segs[1..]),
        Some(s) if s.starts_with("primase_") || matches!(s, "std" | "core" | "alloc") => (Vec::new(), segs),
        _ => (here(), segs),
    };
    while rest.first().is_some_and(|s| s == "super") {
        base.pop();
        rest = &rest[1..];
    }
    base.extend(rest.iter().cloned());
    base.join("::")
}

/// Whether `path` is `prefix` or lies under it.
fn within(path: &str, prefix: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|r| r.starts_with("::"))
}

/// The workspace's modules and which do I/O, as `crate_ident::path`: a path is I/O when the deepest module it lies in
/// is (a pure child of an I/O module is not), or when it is under a re-export of an I/O path.
struct IoIndex {
    /// Every library module (each crate root too), with whether it does I/O.
    modules: BTreeMap<String, bool>,
    /// Re-exports of I/O paths, by the path they are re-exported as.
    aliases: BTreeSet<String>,
}

impl IoIndex {
    /// The deepest known module `path` lies in.
    fn module(&self, path: &str) -> Option<&String> {
        self.modules
            .keys()
            .filter(|m| within(path, m))
            .max_by_key(|m| m.len())
    }
    /// The I/O module or re-export `path` reaches, if any.
    fn io(&self, path: &str) -> Option<String> {
        if let Some(a) = self.aliases.iter().find(|a| within(path, a)) {
            return Some(a.clone());
        }
        let m = self.module(path)?;
        self.modules[m].then(|| m.clone())
    }
}

/// One source file of a worlds crate, as the layout rules see it.
struct LayoutFile<'a> {
    file: &'a str,
    src_path: &'a str,
    text: &'a str,
    tokens: &'a [Tok],
    krate: &'a str,
}

/// The layout rules for one file of a worlds crate (CONTRIBUTING "Worlds crates: one role vocabulary"):
/// - reader I/O only under `read/` or a stage folder, writer I/O only under `write/` or a stage folder;
/// - an I/O allowance covers no module beneath it: none on a `mod` declaration, and no inner one in a file that
///   declares modules;
/// - no `#[path]` attribute and no inline role module (`mod read { ... }`), so a file's place is its path;
/// - passes, components and the other modules (format modules, codecs; the crate root aside, which re-exports for
///   consumers) name no I/O path but their own module's: every module that opts in to I/O, every `read`/`write`/stage
///   module of a worlds crate, and every re-export of one of them (`IoIndex`).
///
/// Coverage is by name: a function value, a trait object or a macro that reaches I/O is not seen.
fn layout_violations(f: &LayoutFile, stages: &[String], io: &IoIndex) -> Vec<String> {
    let (file, src_path) = (f.file, f.src_path);
    let at = place(src_path, stages);
    let module = module_of(src_path);
    let mut out = Vec::new();
    for (role, inner, on_mod) in io_attrs(f.text) {
        let allowed = match role.as_str() {
            "reader I/O" => matches!(at, Place::Read | Place::Stage(_)),
            "writer I/O" => matches!(at, Place::Write | Place::Stage(_)),
            _ => true,
        };
        if !allowed {
            out.push(format!(
                "{file}: {role} in {at:?} (reader I/O belongs under read/, writer I/O under write/)"
            ));
        }
        if on_mod {
            out.push(format!(
                "{file}: an I/O allowance on a mod declaration covers the whole module"
            ));
        }
        if inner && declares_modules(f.tokens) {
            out.push(format!(
                "{file}: an inner I/O allowance covers the modules this file declares"
            ));
        }
    }
    if f.text.contains("#[path") {
        out.push(format!(
            "{file}: a #[path] attribute (a module's place is its file's path)"
        ));
    }
    let role_name =
        |n: &str| matches!(n, "read" | "write" | "passes" | "components") || stages.iter().any(|s| s == n);
    for w in f.tokens.windows(3) {
        if ident(&w[0], "mod") && matches!(&w[1], Tok::Ident(n) if role_name(n)) && punct(&w[2], '{') {
            out.push(format!("{file}: an inline role module (give it its own file)"));
        }
    }
    if matches!(at, Place::Passes | Place::Components | Place::Other) && !role_root(src_path, f.tokens) {
        let (uses, paths) = named(f.tokens);
        let all = uses
            .iter()
            .flat_map(|u| u.items.iter().map(|(p, _)| p.clone()))
            .chain(paths);
        let here = absolute(&[], f.krate, &module);
        let mut seen = BTreeSet::new();
        for p in all {
            let abs = absolute(&p, f.krate, &module);
            if io.module(&abs) == Some(&here) {
                continue; // its own module
            }
            if let Some(m) = io.io(&abs)
                && seen.insert(m.clone())
            {
                out.push(format!("{file}: {at:?} names the I/O module {m}"));
            }
        }
    }
    out
}

/// The `allow(clippy::disallowed_*)` attributes of a source text: (role, inner attribute, applied to a `mod` item).
fn io_attrs(text: &str) -> Vec<(String, bool, bool)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = text[from..].find("allow(") {
        let start = from + p;
        let end = text[start..].find(")]").map_or(text.len(), |e| start + e + 2);
        let attr = &text[start..end];
        if attr.contains("clippy::disallowed_") {
            let role = attr
                .split_once("reason = \"")
                .and_then(|(_, r)| r.split_once(':'))
                .map_or_else(|| "(no reason)".to_owned(), |(role, _)| role.to_owned());
            let inner = text[..start].trim_end().ends_with("#![");
            let after = text[end..].trim_start();
            let after = after
                .strip_prefix("pub(crate)")
                .or_else(|| after.strip_prefix("pub(super)"))
                .or_else(|| after.strip_prefix("pub"))
                .unwrap_or(after)
                .trim_start();
            out.push((role, inner, !inner && after.starts_with("mod ")));
        }
        from = end;
    }
    out
}

/// Whether a (test-free) token stream declares a module.
fn declares_modules(t: &[Tok]) -> bool {
    t.windows(3).any(|w| {
        ident(&w[0], "mod") && matches!(w[1], Tok::Ident(_)) && (punct(&w[2], ';') || punct(&w[2], '{'))
    })
}

/// The workspace's I/O index: every library module, I/O when it opts in to reader, writer or toolkit I/O or is a
/// `read`/`write`/stage module of a worlds crate, and every re-export of an I/O path (followed until nothing new
/// appears).
fn io_paths(members: &[Member]) -> IoIndex {
    let mut modules = BTreeMap::new();
    let mut files = Vec::new();
    for m in members.iter().filter(|m| m.library()) {
        let krate = m.name.replace('-', "_");
        let src = format!("{}/src/", m.path);
        for file in rust_files(&root().join(&m.path).join("src")) {
            let src_path = file[src.len()..].to_owned();
            let module = module_of(&src_path);
            let here: Vec<String> = std::iter::once(krate.clone())
                .chain(module.iter().cloned())
                .collect();
            let opted = io_attrs(&read(&root().join(&file)))
                .iter()
                .any(|(r, ..)| matches!(r.as_str(), "reader I/O" | "writer I/O" | "toolkit I/O"));
            let role = m.layer() == "worlds"
                && matches!(
                    place(&src_path, &m.stages),
                    Place::Read | Place::Write | Place::Stage(_)
                );
            modules.insert(here.join("::"), (opted || role) && !module.is_empty());
            files.push((krate.clone(), module, here, code(&file)));
        }
    }
    let mut io = IoIndex {
        modules,
        aliases: BTreeSet::new(),
    };
    loop {
        let before = io.aliases.len();
        for (krate, module, here, tokens) in &files {
            let (uses, _) = named(tokens);
            for u in uses.iter().filter(|u| u.public) {
                for (path, name) in &u.items {
                    if name != "*" && io.io(&absolute(path, krate, module)).is_some() {
                        io.aliases.insert(format!("{}::{name}", here.join("::")));
                    }
                }
            }
        }
        if io.aliases.len() == before {
            return io;
        }
    }
}

#[test]
fn worlds_layout() {
    let members = members();
    let io = io_paths(&members);
    let excused: BTreeSet<String> = entries("layout").iter().map(|e| e["file"].clone()).collect();
    let mut found = BTreeMap::new();
    for m in members.iter().filter(|m| m.layer() == "worlds") {
        let krate = m.name.replace('-', "_");
        let src = format!("{}/src/", m.path);
        for file in rust_files(&root().join(&m.path).join("src")) {
            let text = read(&root().join(&file));
            let tokens = code(&file);
            let f = LayoutFile {
                file: &file,
                src_path: &file[src.len()..],
                text: &text,
                tokens: &tokens,
                krate: &krate,
            };
            let v = layout_violations(&f, &m.stages, &io);
            if !v.is_empty() {
                found.insert(file.clone(), v);
            }
        }
    }
    let mut msg = String::new();
    for (file, v) in &found {
        if !excused.contains(file) {
            for line in v {
                msg += &format!("\n  violation: {line}");
            }
        }
    }
    for file in &excused {
        if !found.contains_key(file) {
            msg +=
                &format!("\n  stale migration entry (the module follows the layout now; delete it): {file}");
        }
    }
    assert!(msg.is_empty(), "worlds crate layout:{msg}");
}

#[test]
fn layout_rules() {
    let stages = vec!["postcompile".to_owned()];
    let modules = [
        ("primase_image", false),
        ("primase_image::texture", true),
        ("primase_x", false),
        ("primase_x::read", true),
        ("primase_x::write", true),
        ("primase_x::postcompile", true),
        ("primase_x::components", false),
        ("primase_x::interchange", true),
        ("primase_x::interchange::tables", false),
        ("primase_x::passes::floor", false),
        ("primase_x::archive", true),
    ];
    let io = IoIndex {
        modules: modules.iter().map(|(m, io)| ((*m).to_owned(), *io)).collect(),
        aliases: ["primase_x::read_scene".to_owned()].into(),
    };
    let check = |path: &str, src: &str| {
        let tokens = code_of(src);
        let f = LayoutFile {
            file: path,
            src_path: path,
            text: src,
            tokens: &tokens,
            krate: "primase_x",
        };
        layout_violations(&f, &stages, &io)
    };
    let reader = "#[allow(clippy::disallowed_methods, reason = \"reader I/O: x\")]\nfn r() {}";
    let writer = "#[allow(clippy::disallowed_methods, reason = \"writer I/O: x\")]\nfn w() {}";
    // I/O where its role is
    assert!(check("read/scene.rs", reader).is_empty());
    assert!(check("sekiro/read/msb.rs", reader).is_empty());
    assert!(check("postcompile/map_folder.rs", reader).is_empty());
    assert!(check("write/map.rs", writer).is_empty());
    assert!(check("postcompile/vblend.rs", writer).is_empty());
    // a writer reads nothing; reading and writing elsewhere is refused
    assert_eq!(check("write/map.rs", reader).len(), 1);
    assert_eq!(check("passes/floor.rs", writer).len(), 1);
    assert_eq!(check("components.rs", reader).len(), 1);
    assert_eq!(check("interchange/schema.rs", writer).len(), 1);
    // narrow allowances, and a module's place is its file
    let on_mod = "#[allow(clippy::disallowed_methods, reason = \"reader I/O: x\")]\npub mod scene;";
    assert_eq!(check("read/mod.rs", on_mod).len(), 1);
    let inner = "#![allow(clippy::disallowed_methods, reason = \"reader I/O: x\")]\nmod scene;\nfn r() {}";
    assert_eq!(check("read/mod.rs", inner).len(), 1);
    assert_eq!(check("read/mod.rs", "#[path = \"x.rs\"] mod scene;").len(), 1);
    assert_eq!(check("lib.rs", "mod read { pub fn f() {} }").len(), 1);
    // passes, components and shared modules name no I/O path, however it is written
    assert!(check("passes/floor.rs", "use crate::components::Floor; fn f() {}").is_empty());
    assert_eq!(
        check("passes/floor.rs", "use crate::read::scene; fn f() {}").len(),
        1
    );
    assert_eq!(
        check("passes/floor.rs", "use crate::{components::Floor, read::scene};").len(),
        1
    );
    assert_eq!(
        check(
            "passes/a/floor.rs",
            "fn f() { super::super::super::write::map(); }"
        )
        .len(),
        1
    );
    assert_eq!(
        check("passes/floor.rs", "fn f() { crate::postcompile::run(); }").len(),
        1
    );
    assert_eq!(
        check("passes/floor.rs", "fn f() { primase_image::texture::load(); }").len(),
        1
    );
    assert_eq!(check("passes/floor.rs", "use crate::read_scene as rs;").len(), 1);
    assert_eq!(check("components.rs", "use crate::read::Thing;").len(), 1);
    assert_eq!(
        check("collision_query.rs", "fn f() { crate::write::encode(); }").len(),
        1
    );
    assert!(check("passes/floor.rs", "fn f() { primase_image::resize(); }").is_empty());
    // a pure child of an I/O module is not I/O; a module naming itself is not a dependency
    assert!(check("passes/floor.rs", "use crate::interchange::tables::Row;").is_empty());
    assert_eq!(
        check("passes/floor.rs", "use crate::interchange::Thing;").len(),
        1
    );
    assert!(check("archive.rs", "fn f() { crate::archive::open(); }").is_empty());
    // an attribute's paths are not dependencies
    assert!(
        check(
            "interchange/schema.rs",
            "#[allow(clippy::too_many_lines)]
fn f() {}"
        )
        .is_empty()
    );
    // I/O modules may name each other; the crate root re-exports for consumers
    assert!(check("write/map.rs", "use crate::read::scene;").is_empty());
    assert!(check("lib.rs", "pub use read::scene::read_scene;").is_empty());
    // a game folder's root (it declares role modules) re-exports too; another folder's mod.rs does not
    assert!(
        check(
            "sekiro/mod.rs",
            "mod read;
pub use crate::read::scene::read_scene;"
        )
        .is_empty()
    );
    assert_eq!(
        check(
            "misc/mod.rs",
            "mod tables;
pub use crate::read::scene::read_scene;"
        )
        .len(),
        1
    );
}

#[test]
fn layout_paths() {
    let mut out = Vec::new();
    use_tree(
        &code_of("crate::{read::{a, b as c}, passes::*, write::self}"),
        &[],
        &mut out,
    );
    let got: Vec<(String, String)> = out.into_iter().map(|(p, n)| (p.join("::"), n)).collect();
    let want: Vec<(String, String)> = [
        ("crate::read::a", "a"),
        ("crate::read::b", "c"),
        ("crate::passes", "*"),
        ("crate::write", "write"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect();
    assert_eq!(got, want);
    let segs = |s: &str| s.split("::").map(str::to_owned).collect::<Vec<_>>();
    let module = segs("passes::a::floor");
    assert_eq!(
        absolute(&segs("super::write::x"), "k", &module),
        "k::passes::a::write::x"
    );
    assert_eq!(
        absolute(&segs("super::super::super::write"), "k", &module),
        "k::write"
    );
    assert_eq!(absolute(&segs("crate::read::x"), "k", &module), "k::read::x");
    assert_eq!(
        absolute(&segs("primase_image::texture"), "k", &module),
        "primase_image::texture"
    );
    assert_eq!(absolute(&segs("read::x"), "k", &[]), "k::read::x");
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
    "monogame",
    "xna",
    "fna",
    "stuntboost",
    "anvil",
    "scimitar",
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
        // a worlds crate's own names (its games, its engine, its destination); shared code has none. A game crate's
        // declared source names are its game's own (a whole literal, as stored), and each must be used.
        let mut used = BTreeSet::new();
        for file in rust_files(&root().join(&m.path).join("src")) {
            for t in code(&file) {
                let s = match t {
                    Tok::Ident(s) => s,
                    Tok::Str(s) if m.source_names.contains(&s) => {
                        used.insert(s);
                        continue;
                    }
                    Tok::Str(s) => s,
                    _ => continue,
                };
                for w in words(&s) {
                    if NAMES.contains(&w.as_str()) && !m.names.contains(&w) {
                        found.insert(format!("{}: {w}", m.path));
                    }
                }
            }
        }
        for n in m.source_names.iter().filter(|n| !used.contains(*n)) {
            found.insert(format!("{}: declared source name {n:?} is not used", m.path));
        }
    }
    let excused = entries("name")
        .iter()
        .map(|e| format!("{}: {}", e["crate"], e["word"]))
        .collect();
    assert_reconciled("game and engine names in shared code", found, excused);
}

// ---- entry points only in examples and bins ----

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

/// Core's list of its own names (`primase_core::names`) holds every component and resource core implements: the types
/// named in `impl ... Component for T` and `impl ... Resource for T` in core's sources (tests left out) are exactly the
/// types it lists.
#[test]
fn core_names_listed() {
    let mut implemented = BTreeSet::new();
    for f in rust_files(&root().join("core").join("src")) {
        let t = code(&f);
        for i in 1..t.len().saturating_sub(1) {
            if ident(&t[i], "for")
                && (ident(&t[i - 1], "Component") || ident(&t[i - 1], "Resource"))
                && let Tok::Ident(ty) = &t[i + 1]
            {
                implemented.insert(ty.clone());
            }
        }
    }
    let listed: BTreeSet<String> = primase_core::names::LISTED
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let missing: Vec<&String> = implemented.difference(&listed).collect();
    let extra: Vec<&String> = listed.difference(&implemented).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "core/src/names.rs lists other types than core implements as components and resources: not listed {missing:?}, \
         listed but not implemented {extra:?}"
    );
}

/// Every component's stated geometry ([`primase_core::Geometry`], `const GEOMETRY`) matches the hooks it implements,
/// in every crate, tests included: `PerPrimitive` implements `remap_primitives`, `PerVertex` that and
/// `remap_vertices`, `None` neither. The World refuses at run time a dependent component that does not answer an
/// edit; this sees a missing hook before an edit reaches one.
#[test]
fn component_geometry_hooks() {
    let mut wrong = Vec::new();
    let mut seen = 0;
    for m in members() {
        for f in rust_files(&root().join(&m.path)) {
            let t = lex(&read(&root().join(&f)));
            for i in 1..t.len().saturating_sub(1) {
                if !(ident(&t[i], "for") && ident(&t[i - 1], "Component")) {
                    continue;
                }
                let Some(open) = (i..t.len()).find(|&k| punct(&t[k], '{')) else {
                    continue;
                };
                let body = &t[open..block_end(&t, open)];
                let ty = match &t[i + 1] {
                    Tok::Ident(ty) => ty.clone(),
                    other => format!("{other:?}"),
                };
                let declared = body.iter().position(|x| ident(x, "GEOMETRY")).and_then(|g| {
                    body[g..]
                        .iter()
                        .take_while(|x| !punct(x, ';'))
                        .filter_map(|x| match x {
                            Tok::Ident(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .last()
                });
                let has = |hook: &str| body.windows(2).any(|w| ident(&w[0], "fn") && ident(&w[1], hook));
                let hooks = (has("remap_primitives"), has("remap_vertices"));
                let expected = match declared {
                    Some("None") => (false, false),
                    Some("PerPrimitive") => (true, false),
                    Some("PerVertex") => (true, true),
                    other => {
                        wrong.push(format!("{f}: {ty} states no geometry ({other:?})"));
                        continue;
                    }
                };
                seen += 1;
                if hooks != expected {
                    wrong.push(format!(
                        "{f}: {ty} states {} but implements remap_primitives {}, remap_vertices {}",
                        declared.unwrap_or("?"),
                        hooks.0,
                        hooks.1
                    ));
                }
            }
        }
    }
    assert!(seen > 100, "only {seen} components found: the scan is broken");
    assert!(
        wrong.is_empty(),
        "components whose stated geometry and hooks disagree:\n  {}",
        wrong.join("\n  ")
    );
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
const KNOWN_BREAKS: &[&str] = &[];

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
