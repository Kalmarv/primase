//! [`inputs!`](crate::inputs): a consumer's declaration of its real-data inputs, and the typed queries over it.

/// Declares a consumer's real-data inputs in its test-support module (`tests/inputs/mod.rs`) and gives its tests typed
/// queries over them.
///
/// It generates, in the module it is invoked in:
/// - `Game`, `Tool` and `OracleSet`: enums of the games, tools and oracle sets declared, each with `key()`, its name in
///   the configuration;
/// - `Query`: a typed [`Query`](crate::Query) (`Query::Oracle(OracleSet::Corpus, Game::Gta, "shaders")`), with `raw()`
///   and `key()`;
/// - `REGISTRY`: the [`Registry`](crate::Registry) of everything declared;
/// - the queries over it: `resolve`, `install`, `install_file`, `keys`, `tool`, `oodle`, `oracle`, `debug` and
///   `expect_found`, and this crate's own helpers re-exported (`scratch`, `own_scratch`, `warp_exclusive`, `depth`,
///   `seed`, `threads`, `read_input`, `Depth`, `OodleLibrary`, `OwnScratch`, `WarpLock`).
///
/// Every section is required, in this order, and may be empty. A declared file's `status` is `Live`, `Blocked` or
/// `Unheld("why")` ([`Status`](crate::Status)).
///
/// ```
/// mod inputs {
///     primase_testdata::inputs! {
///         games {
///             Sekiro = "sekiro",
///             General = "general",
///         }
///         installs {
///             Sekiro {
///                 about: "Sekiro's install folder",
///                 source: "the user's own install",
///                 gate: true,
///             },
///         }
///         keys {}
///         tools {
///             Oodle6 = "oodle6" {
///                 about: "an Oodle version-6 DLL",
///                 source: "the game's own DLL from the user's install",
///                 gate: true,
///             },
///         }
///         oracle_sets {
///             Corpus = "corpus" {
///                 about: "the games' own files cut from their installs",
///                 source: "the user's own installs",
///             },
///         }
///         declared {
///             Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv") => {
///                 status: Live,
///                 gate: true,
///                 about: "a table of codec streams",
///                 source: "the user's own installs",
///             },
///         }
///         debug {
///             "oodle.dump" => "a folder under the scratch root the Oodle gate writes to",
///         }
///     }
/// }
///
/// // GAME_DATA unset: a SKIP line naming `installs.sekiro`, and `None`
/// assert_eq!(inputs::install(inputs::Game::Sekiro), None);
/// assert!(inputs::REGISTRY.problems().is_empty());
/// ```
#[macro_export]
macro_rules! inputs {
    (
        games { $( $game:ident = $game_key:literal ),* $(,)? }
        installs {
            $( $install:ident {
                about: $install_about:literal,
                source: $install_source:literal,
                gate: $install_gate:literal $(,)?
            } ),* $(,)?
        }
        keys {
            $( $keys:ident {
                about: $keys_about:literal,
                source: $keys_source:literal,
                gate: $keys_gate:literal $(,)?
            } ),* $(,)?
        }
        tools {
            $( $tool:ident = $tool_key:literal {
                about: $tool_about:literal,
                source: $tool_source:literal,
                gate: $tool_gate:literal $(,)?
            } ),* $(,)?
        }
        oracle_sets {
            $( $set:ident = $set_key:literal {
                about: $set_about:literal,
                source: $set_source:literal $(,)?
            } ),* $(,)?
        }
        declared {
            $( $query:expr => {
                status: $status:ident $( ( $why:literal ) )?,
                gate: $gate:literal,
                about: $about:literal,
                source: $source:literal $(,)?
            } ),* $(,)?
        }
        debug { $( $debug_key:literal => $debug_about:literal ),* $(,)? }
    ) => {
        /// A game this crate's tests read, as the configuration names its install, keys and oracle folders.
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Game {
            $( $game ),*
        }

        #[allow(dead_code)]
        impl Game {
            /// Its key in the configuration and its folder name under an oracle set.
            pub const fn key(self) -> &'static str {
                match self {
                    $( Game::$game => $game_key ),*
                }
            }
        }

        /// A program a test runs or a file of one.
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Tool {
            $( $tool ),*
        }

        #[allow(dead_code)]
        impl Tool {
            /// Its key in the configuration's `[tools]`.
            pub const fn key(self) -> &'static str {
                match self {
                    $( Tool::$tool => $tool_key ),*
                }
            }
        }

        /// An oracle set: the outputs of one producer, under `<oracles.root>/<set>/<game>/`.
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum OracleSet {
            $( $set ),*
        }

        #[allow(dead_code)]
        impl OracleSet {
            /// Its folder under `oracles.root`.
            pub const fn key(self) -> &'static str {
                match self {
                    $( OracleSet::$set => $set_key ),*
                }
            }
        }

        /// What a test asks the configuration for.
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Query {
            /// A game's install, or a file or folder inside it (relative, `/`-separated).
            Install(Game, Option<&'static str>),
            /// A game's archive key material.
            Keys(Game),
            /// A tool's executable or file.
            Tool(Tool),
            /// A file or folder of an oracle set's folder for a game (relative, `/`-separated).
            Oracle(OracleSet, Game, &'static str),
            /// The scratch root, or a folder under it.
            Scratch(Option<&'static str>),
            /// A debug key.
            Debug(&'static str),
        }

        #[allow(dead_code)]
        impl Query {
            /// The same query by the configuration's names.
            pub const fn raw(self) -> $crate::Query {
                match self {
                    Query::Install(game, rel) => $crate::Query::Install(game.key(), rel),
                    Query::Keys(game) => $crate::Query::Keys(game.key()),
                    Query::Tool(tool) => $crate::Query::Tool(tool.key()),
                    Query::Oracle(set, game, rel) => $crate::Query::Oracle(set.key(), game.key(), rel),
                    Query::Scratch(rel) => $crate::Query::Scratch(rel),
                    Query::Debug(key) => $crate::Query::Debug(key),
                }
            }

            /// The query's key: the configuration key and, for a file or folder inside, its relative path.
            pub fn key(self) -> ::std::string::String {
                self.raw().key()
            }
        }

        /// Everything this crate's tests may ask the real-data configuration for.
        #[allow(dead_code)]
        pub static REGISTRY: $crate::Registry = $crate::Registry {
            names: $crate::Names {
                games: &[ $( (stringify!($game), $game_key) ),* ],
                tools: &[ $( (stringify!($tool), $tool_key) ),* ],
                oracle_sets: &[ $( (stringify!($set), $set_key) ),* ],
            },
            installs: &[ $( $crate::Entry {
                name: Game::$install.key(),
                about: $install_about,
                source: $install_source,
                gate: $install_gate,
            } ),* ],
            keys: &[ $( $crate::Entry {
                name: Game::$keys.key(),
                about: $keys_about,
                source: $keys_source,
                gate: $keys_gate,
            } ),* ],
            tools: &[ $( $crate::Entry {
                name: $tool_key,
                about: $tool_about,
                source: $tool_source,
                gate: $tool_gate,
            } ),* ],
            oracle_sets: &[ $( $crate::Set {
                name: $set_key,
                about: $set_about,
                source: $set_source,
            } ),* ],
            declared: &[ $( $crate::Declared {
                query: Query::raw($query),
                status: $crate::Status::$status $( { why: $why } )?,
                gate: $gate,
                about: $about,
                source: $source,
            } ),* ],
            debug: &[ $( ($debug_key, $debug_about) ),* ],
        };

        /// Resolve `query` to an existing path, or skip with a line naming its key (`primase_testdata::resolve`).
        #[allow(dead_code)]
        #[track_caller]
        pub fn resolve(query: Query) -> ::std::option::Option<::std::path::PathBuf> {
            $crate::resolve(&REGISTRY, query.raw())
        }

        /// A game's install folder.
        #[allow(dead_code)]
        #[track_caller]
        pub fn install(game: Game) -> ::std::option::Option<::std::path::PathBuf> {
            resolve(Query::Install(game, None))
        }

        /// A file or folder inside a game's install (`rel` relative, `/`-separated, declared).
        #[allow(dead_code)]
        #[track_caller]
        pub fn install_file(game: Game, rel: &'static str) -> ::std::option::Option<::std::path::PathBuf> {
            resolve(Query::Install(game, Some(rel)))
        }

        /// A game's archive key material.
        #[allow(dead_code)]
        #[track_caller]
        pub fn keys(game: Game) -> ::std::option::Option<::std::path::PathBuf> {
            resolve(Query::Keys(game))
        }

        /// A tool's executable or file.
        #[allow(dead_code)]
        #[track_caller]
        pub fn tool(tool: Tool) -> ::std::option::Option<::std::path::PathBuf> {
            resolve(Query::Tool(tool))
        }

        /// The Oodle codec for a game's data (`primase_testdata::oodle`).
        #[allow(dead_code)]
        #[track_caller]
        pub fn oodle(version: Tool) -> ::std::option::Option<$crate::OodleLibrary> {
            $crate::oodle(&REGISTRY, version.key())
        }

        /// A file or folder of an oracle set for a game: `<oracles.root>/<set>/<game>/<rel>` (`rel` relative,
        /// `/`-separated, declared, a literal so the self-check sees it).
        #[allow(dead_code)]
        #[track_caller]
        pub fn oracle(set: OracleSet, game: Game, rel: &'static str) -> ::std::option::Option<::std::path::PathBuf> {
            resolve(Query::Oracle(set, game, rel))
        }

        /// A declared debug key's value (`GAME_DATA_DEBUG=key=value;key=value`); `None` when unset.
        #[allow(dead_code)]
        #[track_caller]
        pub fn debug(key: &str) -> ::std::option::Option<::std::string::String> {
            $crate::debug(&REGISTRY, key)
        }

        /// A configured input that yields nothing is a failure: call it with how many `what` `query` gave.
        #[allow(dead_code)]
        #[track_caller]
        pub fn expect_found(query: Query, what: &str, n: usize) {
            $crate::expect_found(&REGISTRY, query.raw(), what, n)
        }

        #[allow(unused_imports)]
        pub use $crate::{
            Depth, OodleLibrary, OwnScratch, WarpLock, depth, own_scratch, read_input, scratch, seed, threads,
            warp_exclusive,
        };
    };
}
