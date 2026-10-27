# primase-testdata

Real-data inputs for tests. Tests that read a game's own files, run a third-party tool or compare against an oracle's
output ask for them by a typed query; one TOML file per machine, named by `GAME_DATA`, says where each is. What a
machine does not have skips the test with one line naming its key, unless the run says the gate must have it.

Use it only as a dev-dependency.

## Declaring a crate's inputs

Each crate declares what its tests read in `tests/inputs/mod.rs`, with `primase_testdata::inputs!`: its games, installs,
keys, tools, oracle sets, every file of an install or an oracle set it asks for, and its debug keys, each with what it
is, where it comes from and whether the landing gate needs it.

```rust
primase_testdata::inputs! {
    games {
        Sekiro = "sekiro",
    }
    installs {
        Sekiro {
            about: "Sekiro's install folder",
            source: "the user's own install",
            gate: true,
        },
    }
    keys {}
    tools {
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
    }
    declared {
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "dcx") => {
            status: Live,
            gate: true,
            about: "Sekiro's DCX files as stored",
            source: "the user's own install",
        },
    }
    debug {
        "oodle.dump" => "a folder under the scratch root the Oodle gate writes to",
    }
}
```

It gives the module the enums `Game`, `Tool` and `OracleSet`, a typed `Query`, the `REGISTRY`, and the queries over
it: `install`, `install_file`, `keys`, `tool`, `oodle`, `oracle`, `resolve`, `debug` and `expect_found`, beside
`scratch`, `own_scratch`, `warp_exclusive`, `depth`, `seed`, `threads` and `read_input`.

Integration tests reach it with `mod inputs;`; unit tests with
`#[cfg(test)] #[path = "../tests/inputs/mod.rs"] mod inputs;` in the crate root:

```rust
mod inputs;
use inputs::{Game, OracleSet, oracle};

#[test]
fn every_dcx_file_reads() {
    let Some(folder) = oracle(OracleSet::Corpus, Game::Sekiro, "dcx") else {
        return; // not configured here: a SKIP line names `oracles.corpus.sekiro/dcx`
    };
    // ...
}
```

Each crate runs the self-check as one test (`tests/real_data_inputs.rs`):

```rust
mod inputs;

#[test]
fn real_data_inputs_are_declared() {
    primase_testdata::check_declared(&inputs::REGISTRY, env!("CARGO_MANIFEST_DIR"));
}
```

It fails on a query in the crate's sources for an input the registry does not declare, a path that is not a literal,
a source that reads `GAME_DATA` itself, and a README whose list of inputs (`primase_testdata::doc::tables`, between its
markers) differs from the registry.

## The configuration file

One file serves every crate on the machine ([game-data.example.toml](game-data.example.toml)):

```toml
[installs]
sekiro = 'D:/games/Sekiro'
[keys]
gta = 'D:/keys/gta'
[tools]
oodle6 = 'D:/games/Sekiro/oo2core_6_win64.dll'
[oracles]
root = 'D:/oracles'               # <root>/<set>/<game>/<file>
[scratch]
root = 'D:/scratch'
[run]
require = 'gate'                  # or 'all': an unset input fails instead of skipping
gate = 'smoke'                    # or 'full': how much the heavy tests cover
seed = '20384'                    # the smoke sample's seed (the day's by default)
threads = '1'                     # the threads a heavy test's work may use
```

An unknown section is an error, and so is an unknown key of `[oracles]`, `[scratch]` and `[run]`; the keys of
`[installs]`, `[keys]` and `[tools]` belong to every crate, so one a crate does not declare is accepted. A configured
path that does not exist fails; an oracle set the machine does not hold (no `<root>/<set>` folder) skips. Debug
switches go in `GAME_DATA_DEBUG='key=value;key=value'`.

## Gates

`primase_testdata::gate` counts a comparison against an oracle: how many items agree exactly, how many within a stated
tolerance (`GateReport`, `Tolerance`), and compares JSON documents number by number (`compare_json`).

## Licence

GPL-3.0-or-later.
