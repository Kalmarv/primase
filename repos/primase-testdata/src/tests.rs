use super::*;
use std::collections::BTreeMap;

/// A consumer's declaration, as `tests/inputs/mod.rs` makes one.
mod inputs {
    crate::inputs! {
        games {
            Sekiro = "sekiro",
            Gta = "gta",
            Riders = "riders",
            Skate = "skate",
            General = "general",
        }
        installs {
            Gta {
                about: "a GTA V install folder",
                source: "the user's own install",
                gate: true,
            },
            Riders {
                about: "Riders Republic's folder of `.forge` archives",
                source: "the user's own install",
                gate: false,
            },
            Skate {
                about: "the skate. install",
                source: "the user's own install",
                gate: true,
            },
        }
        keys {
            Gta {
                about: "the user's GTA V archive key material",
                source: "the user's own key material",
                gate: true,
            },
        }
        tools {
            Warp = "warp" {
                about: "the WARP oracle program",
                source: "third-party: Microsoft's WARP rasterizer",
                gate: true,
            },
            UnityTpk = "unity_tpk" {
                about: "a type tree package",
                source: "input data",
                gate: true,
            },
            Oodle6 = "oodle6" {
                about: "an Oodle version-6 DLL",
                source: "the game's own DLL from the user's install",
                gate: true,
            },
        }
        oracle_sets {
            Corpus = "corpus" {
                about: "the games' own files cut from their installs",
                source: "the user's own installs: files extracted as stored",
            },
            CodeWalker = "codewalker" {
                about: "CodeWalker's readings of a GTA V install",
                source: "independent: CodeWalker.Core",
            },
        }
        declared {
            Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv") => {
                status: Live,
                gate: true,
                about: "a table of codec streams",
                source: "the user's own installs",
            },
            Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx") => {
                status: Live,
                gate: false,
                about: "Sekiro's DCX files",
                source: "the user's own install",
            },
            Query::Oracle(OracleSet::Corpus, Game::Gta, "legacy-effects") => {
                status: Unheld("needs a GTA V Legacy install"),
                gate: false,
                about: "GTA V Legacy's effect files",
                source: "the user's own install",
            },
            Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl") => {
                status: Blocked,
                gate: true,
                about: "CodeWalker's archive listing",
                source: "independent: CodeWalker.Core",
            },
            Query::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge")) => {
                status: Live,
                gate: false,
                about: "a large Anvil forge",
                source: "the user's own install",
            },
        }
        debug {
            "shader.diff_limit" => "how many programs of a corpus the WARP comparison runs",
            "gta.region" => "only this named gate region",
        }
    }
}

use inputs::{Game, OracleSet, REGISTRY, Tool};

/// The configuration a fake environment (`env`) and file (`text`, named `data.toml`) give.
fn config(env: &[(&str, &str)], text: Option<&str>) -> Result<Config, config::ConfigError> {
    let env: BTreeMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .chain(text.map(|_| ("GAME_DATA".to_owned(), "data.toml".to_owned())))
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
    let tpk = configured(&c, inputs::Query::Tool(Tool::UnityTpk).raw()).unwrap();
    assert_eq!(tpk.as_os_str(), "data/typetrees.tpk");
}

/// An input no machine here holds skips, naming why, even where every input is required; one that is there is
/// found.
#[test]
fn an_unheld_input_skips_naming_why_whatever_require_says() {
    let legacy = inputs::Query::Oracle(OracleSet::Corpus, Game::Gta, "legacy-effects").raw();
    let c = config(&[], Some("[oracles]\nroot = 'nowhere'\n[run]\nrequire = 'all'\n")).unwrap();
    assert!(
        matches!(decide(&REGISTRY, &c, legacy, false), Decision::Skip(why) if why.contains("GTA V Legacy"))
    );
    let root = std::env::temp_dir().join(format!("primase-testdata-unheld-{}", std::process::id()));
    std::fs::create_dir_all(root.join("corpus/gta/legacy-effects")).unwrap();
    let held = config(&[], Some(&format!("[oracles]\nroot = '{}'\n", root.display()))).unwrap();
    assert_eq!(
        decide(&REGISTRY, &held, legacy, false),
        Decision::Found(root.join("corpus/gta/legacy-effects"))
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// An oracle set this machine does not hold skips (or fails where required); a file missing inside a set that is
/// there fails; a file that is there is found.
#[test]
fn an_absent_oracle_set_skips_and_a_missing_file_in_a_present_one_fails() {
    let root = std::env::temp_dir().join(format!("primase-testdata-sets-{}", std::process::id()));
    std::fs::create_dir_all(root.join("corpus").join("general")).unwrap();
    std::fs::write(root.join("corpus").join("general").join("codecs.tsv"), b"").unwrap();
    let text = format!("[oracles]\nroot = '{}'\n", root.display());
    let c = config(&[], Some(&text)).unwrap();
    let codecs = inputs::Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv").raw();
    let typo = inputs::Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx").raw();
    let elsewhere = inputs::Query::Oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl").raw();
    let decide = |c: &Config, q, gate| decide(&REGISTRY, c, q, gate);
    assert_eq!(
        decide(&c, codecs, true),
        Decision::Found(root.join("corpus/general/codecs.tsv"))
    );
    assert!(matches!(decide(&c, typo, false), Decision::Fail(why) if why.contains("does not exist")));
    assert!(
        matches!(decide(&c, elsewhere, false), Decision::Skip(why) if why.contains("oracles.codewalker not on this machine"))
    );
    let required = config(&[], Some(&format!("{text}[run]\nrequire = 'all'\n"))).unwrap();
    assert!(matches!(decide(&required, elsewhere, false), Decision::Fail(why) if why.contains("require")));
    // a root that is gone is a configuration that is wrong, not sets this machine lacks
    let gone = config(
        &[],
        Some(&format!("[oracles]\nroot = '{}'\n", root.join("gone").display())),
    )
    .unwrap();
    assert!(matches!(decide(&gone, elsewhere, false), Decision::Fail(why) if why.contains("oracles.root")));
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
    let at = |q: inputs::Query| configured(&c, q.raw());
    use inputs::Query as Q;
    assert_eq!(
        at(Q::Install(Game::Riders, Some("DataPC_MAD_USNAT.forge"))),
        Some(PathBuf::from("games/rr").join("DataPC_MAD_USNAT.forge"))
    );
    assert_eq!(at(Q::Keys(Game::Gta)), Some(PathBuf::from("keys/gta")));
    assert_eq!(at(Q::Tool(Tool::Warp)), Some(PathBuf::from("bin/warp")));
    assert_eq!(
        at(Q::Oracle(OracleSet::CodeWalker, Game::Gta, "res/a.json")),
        Some(["oracles", "codewalker", "gta", "res", "a.json"].iter().collect())
    );
    assert_eq!(
        at(Q::Scratch(Some("warp"))),
        Some(["scratch", "warp"].iter().collect())
    );
    assert_eq!(at(Q::Install(Game::Sekiro, None)), None);
    // an empty file, or none, configures nothing
    assert_eq!(config(&[], Some("")).unwrap().roots.len(), 0);
    assert!(config(&[], None).unwrap().file.is_none());
}

#[test]
fn unknown_sections_keys_and_values_are_errors() {
    for text in [
        "[install]\nsekiro = 'x'",
        "[tools]\nwarp = 3",
        "[run]\nrequire = 'yes'",
        "[run]\ngate = 'quick'",
        "[run]\nseed = 'today'",
        "[run]\nthreads = '0'",
        "[run]\nrequired = 'gate'",
        "[oracles]\nsoulsformats = 'x'",
        "[scratch]\ndir = 'x'",
        "not valid = [",
    ] {
        let e = config(&[], Some(text))
            .err()
            .unwrap_or_else(|| panic!("{text}: accepted"));
        assert!(e.to_string().contains("data.toml"), "{e}");
    }
    let unreadable = Config::load(
        &|name| (name == "GAME_DATA").then(|| OsString::from("gone.toml")),
        |_| Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
    )
    .unwrap_err();
    assert!(unreadable.to_string().contains("gone.toml"));
}

/// One file serves every consumer: an install, keys or a tool this registry does not declare is another's, so it is
/// read, and only a query for it fails.
#[test]
fn keys_another_consumer_declares_are_accepted() {
    let c = config(
        &[],
        Some("[installs]\nsekiro2 = 'games/s'\n[keys]\nelden = 'keys/elden'\n[tools]\nfxc = 'bin/fxc'\n"),
    )
    .unwrap();
    assert_eq!(c.roots.len(), 3);
    let elsewhere = std::panic::catch_unwind(|| resolve(&REGISTRY, Query::Tool("fxc")));
    assert!(elsewhere.is_err(), "a tool this registry does not declare fails");
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

/// A flat file is refused, naming the sections to write, when loaded and when validated; a variable other than
/// `GAME_DATA` and `GAME_DATA_DEBUG` configures nothing.
#[test]
fn a_flat_file_is_refused_and_other_variables_configure_nothing() {
    let flat = "GTA_DIR = 'file/gta'\nSHADER_DIFF_LIMIT = '7'\n";
    let e = config(&[], Some(flat)).unwrap_err().to_string();
    assert!(
        e.contains("data.toml") && e.contains("structured sections"),
        "{e}"
    );
    let e = config::validate(flat).unwrap_err().to_string();
    assert!(e.contains("structured sections"), "{e}");
    // every variable set, GAME_DATA aside: the configuration is empty
    let everything = Config::load(
        &|name| (name != "GAME_DATA").then(|| OsString::from("set")),
        |_| unreachable!("no file is named"),
    )
    .unwrap();
    assert_eq!(everything, Config::default());
}

#[test]
fn debug_keys_are_read_from_game_data_debug() {
    let env =
        |n: &str| (n == "GAME_DATA_DEBUG").then(|| OsString::from("shader.diff_limit=9; gta.region=legion"));
    assert_eq!(config::debug(&env, "shader.diff_limit").as_deref(), Some("9"));
    assert_eq!(config::debug(&env, "gta.region").as_deref(), Some("legion"));
    assert_eq!(config::debug(&env, "scene.filter"), None);
    assert_eq!(config::debug(&|_| None, "gta.region"), None);
    let undeclared = std::panic::catch_unwind(|| debug(&REGISTRY, "scene.filter"));
    assert!(
        undeclared.is_err(),
        "a debug key the registry does not declare fails"
    );
}

/// The example configuration is one in the structured format.
#[test]
fn the_example_configuration_is_valid() {
    let example = include_str!("../game-data.example.toml");
    config::validate(example).unwrap_or_else(|e| panic!("game-data.example.toml: {e}"));
}

/// A whole registry reports nothing; one that is not names each fault.
#[test]
fn a_registry_reports_what_it_fails_to_say() {
    assert_eq!(REGISTRY.problems(), Vec::<String>::new());
    const BROKEN: Registry = Registry {
        names: Names {
            games: &[],
            tools: &[],
            oracle_sets: &[],
        },
        installs: &[Entry {
            name: "gta",
            about: "",
            source: "the user's own install",
            gate: true,
        }],
        keys: &[],
        tools: &[],
        oracle_sets: &[],
        declared: &[
            Declared {
                query: Query::Oracle("corpus", "gta", "../x"),
                status: Status::Blocked,
                gate: false,
                about: "x",
                source: "y",
            },
            Declared {
                query: Query::Install("sekiro", Some("Data0.bdt")),
                status: Status::Unheld { why: "" },
                gate: false,
                about: "x",
                source: "y",
            },
            Declared {
                query: Query::Install("sekiro", Some("Data0.bdt")),
                status: Status::Live,
                gate: false,
                about: "x",
                source: "y",
            },
            Declared {
                query: Query::Tool("warp"),
                status: Status::Live,
                gate: false,
                about: "x",
                source: "y",
            },
        ],
        debug: &[("gta.region", "")],
    };
    let problems = BROKEN.problems().join("\n");
    for fault in [
        "installs.gta: say what it is",
        "oracles.corpus is not declared",
        "blocked, so a landing-gate input",
        "a relative, /-separated path",
        "installs.sekiro is not declared",
        "unheld, so never a gate input",
        "installs.sekiro/Data0.bdt: declared twice",
        "only files in an install or an oracle set",
        "debug.gta.region: say what it does",
    ] {
        assert!(problems.contains(fault), "{fault} missing from:\n{problems}");
    }
}

#[test]
fn an_undeclared_file_fails_and_a_required_blocked_one_never_skips() {
    let typo = std::panic::catch_unwind(|| inputs::oracle(OracleSet::Corpus, Game::General, "codecs.tsvx"));
    assert!(typo.is_err(), "a mistyped oracle path fails");
    let typo = std::panic::catch_unwind(|| inputs::install_file(Game::Skate, "Date"));
    assert!(typo.is_err(), "a mistyped install path fails");
    let undeclared = std::panic::catch_unwind(|| inputs::install(Game::Sekiro));
    assert!(
        undeclared.is_err(),
        "an install the registry does not declare fails"
    );
    for d in REGISTRY.declared.iter().filter(|d| d.status == Status::Blocked) {
        assert!(gate_of(&REGISTRY, d.query), "{}", d.query.key());
    }
}

#[test]
fn an_empty_configured_input_fails() {
    let gta = inputs::Query::Install(Game::Gta, None);
    let empty = std::panic::catch_unwind(|| inputs::expect_found(gta, "archives", 0));
    assert!(empty.is_err());
    inputs::expect_found(gta, "archives", 1);
}

#[test]
fn query_calls_see_typed_queries() {
    // assembled, so this file's own text holds no such call
    let q = '"';
    let src = format!(
        "use inputs::{{oracle, Game}};\n// oracle(s) in a comment\nlet a = oracle(OracleSet::Corpus, Game::Sekiro, {q}dcx{q});\n\
         let b = crate::inputs::install_file(\n    inputs::Game::Skate,\n    {q}Data{q},\n);\nlet c = Query::Oracle(OracleSet::Corpus, game, {q}shaders{q});\n\
         let d = ImageQuery::Size(x); fn oracle(x: u8) {{}}\nlet e = map.keys(); let f = other::install(x);\n"
    );
    let calls = check::query_calls(&src);
    // an import rustfmt split over lines brings its names in all the same
    let split = format!(
        "use crate::inputs::{{\n    Game,\n    OracleSet,\n    oracle,\n}};\nlet a = oracle(OracleSet::Corpus, Game::Sekiro, {q}dcx{q});\n"
    );
    assert_eq!(check::query_calls(&split).len(), 1, "{split}");
    let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        calls,
        [
            ("oracle", owned(&["OracleSet::Corpus", "Game::Sekiro", "\"dcx\""])),
            ("install_file", owned(&["Game::Skate", "\"Data\""])),
            (
                "Query::Oracle",
                owned(&["OracleSet::Corpus", "game", "\"shaders\""])
            ),
        ]
    );
}

#[test]
fn variable_names_see_whole_and_built_names() {
    // assembled, so this file's own text holds no such literal
    let q = '"';
    let src = format!("var({q}GAME_DATA{q}); format!({q}GAME_DATA_{{x}}{q}); {q}GAME_DATA names{q}");
    assert_eq!(check::variable_names(&src), ["GAME_DATA", "GAME_DATA_"]);
}

/// The self-check over a crate's folder: what its sources ask that the registry lacks, and a README whose tables
/// differ.
#[test]
fn the_self_check_finds_undeclared_queries_and_stale_tables() {
    let root = std::env::temp_dir().join(format!("primase-testdata-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("tests/inputs")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let q = '"';
    // the declaration module itself is not read as asks
    std::fs::write(
        root.join("tests/inputs/mod.rs"),
        format!("Query::Oracle(OracleSet::Corpus, Game::Sekiro, {q}nowhere{q})"),
    )
    .unwrap();
    std::fs::write(
        root.join("tests/reads.rs"),
        format!(
            "mod inputs;\nuse inputs::{{Game as Data, OracleSet, debug, install, keys, oracle}};\n\
             fn a() {{ oracle(OracleSet::Corpus, Data::General, {q}codecs.tsv{q}); }}\n\
             fn b(game: Data) {{ oracle(OracleSet::Corpus, game, {q}dcx{q}); }}\n\
             fn c() {{ oracle(OracleSet::Corpus, Data::Riders, {q}dcx{q}); }}\n\
             fn d() {{ oracle(OracleSet::CodeWalker, Data::Gta, {q}listing.json{q}); }}\n\
             fn e() {{ install(Data::Gta); keys(Data::Riders); debug({q}gta.region{q}); debug({q}gta.regions{q}); }}\n\
             fn f(rel: &'static str) {{ inputs::install_file(Data::Riders, rel); }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        format!(
            "#[cfg(test)]\nmod tests {{\n    fn g() {{ crate::inputs::install_file(crate::inputs::Game::Riders, {q}DataPC_MAD_USNAT.forge{q}); }}\n    fn h() {{ std::env::var({q}GAME_DATA{q}); }}\n}}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("README.md"),
        format!("# x\n\n{}", doc::tables(&REGISTRY)),
    )
    .unwrap();
    std::fs::write(
        root.join("tests/inputs/README.md"),
        format!("{}\n\nstale\n{}\n", doc::BEGIN, doc::END),
    )
    .unwrap();
    let problems = problems(&REGISTRY, &root);
    let want = [
        "src/lib.rs: names GAME_DATA",
        "tests/reads.rs: oracle(OracleSet::Corpus, Data::Riders, \"dcx\") is not declared",
        "tests/reads.rs: oracle(OracleSet::CodeWalker, Data::Gta, \"listing.json\") is not declared",
        "tests/reads.rs: keys(Data::Riders): keys.riders is not declared",
        "tests/reads.rs: debug(\"gta.regions\"): debug.gta.regions is not declared",
        "tests/reads.rs: install_file(Data::Riders, rel): write the relative path as a string literal",
        "tests/inputs/README.md: its real-data tables differ",
    ];
    for w in want {
        assert!(
            problems.iter().any(|p| p.starts_with(w)),
            "{w} missing from:\n{}",
            problems.join("\n")
        );
    }
    assert_eq!(problems.len(), want.len(), "{}", problems.join("\n"));
    std::fs::remove_dir_all(&root).unwrap();
}
