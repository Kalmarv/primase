# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `installs.stuntboost` | yes | STUNTBOOST's install folder (Steam buildid 25832744: `Content/Models/Export`) | the user's own install |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.game-code` | a game's own code run over its install (STUNTBOOST's parsers via reflection: one JSON per level part) | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `installs.stuntboost/Content/Effects` | live | - | STUNTBOOST's compiled effects (`D2/*.xnb`, `D3/*.xnb`: MonoGame `EffectReader`, MGFX version 12, OpenGL profile): every one is read to its shaders, parameters and passes; worlds/stuntboost/tests/effects.rs | the user's own install |
| `oracles.game-code.stuntboost/levels` | live | yes | the game's own parsers run over that install: per level part `<L>_<N>.json` (node names as its `ParsedName` reads them; its collision model as `SE.CollisionModel` loads it: meshes, grippable bits, ramps, ledges) and `<L>_<N>.tris.bin` (every collision triangle); worlds/stuntboost/tests/oracle.rs | independent: the game's own code via reflection, the oracle tool kept with the game's private tools |

<!-- end of real-data inputs -->
