# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `tools.fxc` | - | Microsoft's `fxc` shader compiler executable | third-party: Microsoft's fxc |
| `tools.warp` | yes | the WARP oracle program that runs a shader on Microsoft's WARP rasterizer | third-party: Microsoft's WARP rasterizer, through a thin host program kept with the private oracle tools |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.corpus` | the games' own files cut from their installs (shader containers, EBX documents, Havok tagfiles, DCX files, CAS frames) | the user's own installs: files extracted as stored |
| `oracles.d3d-disassembler` | Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing) | independent: Microsoft's D3D Shader Disassembler, run over the user's own install |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.corpus.elden/shaders` | live | - | a folder (searched recursively) of the DXIL containers of Elden Ring's Shader Model 6 shader bundles (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; repos/iridia/iridia-dxil/tests/lift_corpus.rs | the user's own install: the containers of its `/shader/*.shaderbnd.dcx` binders and `/shader/*.shaderbdlebnd.dcx` bundles (ray-tracing `[rt]` bundles included), worlds/fromsoft/examples/corpus_elden_shaders.rs |
| `oracles.corpus.general/dxbc` | live | yes | Sekiro's 390 pixel programs (`.ppo`), Microsoft's disassembly of each (`.ppo.asm`) and the bindings, inputs and constant layout read from that text (`.ppo.reflection.json`); toolkit/dxbc/tests/disassembly.rs, reflection.rs; schema in toolkit/dxbc/README.md | independent: the game's own programs (`corpus.sekiro/shaders`) disassembled by Microsoft's fxc 10.0.26100 (`fxc /nologo /dumpbin`), the reflection read from the disassembly's text by toolkit/dxbc/tests/oracles/dxbc_corpus.py |
| `oracles.corpus.riders/shaders` | live | - | a folder (searched recursively) of the DXBC containers of Riders Republic's shader pack (ShaderDataDX11.bin, split at its containers), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs | the user's own install: the programs split from its shader pack |
| `oracles.corpus.sekiro/shaders` | live | - | a folder (searched recursively) of the DXBC containers of Sekiro's extracted pixel and vertex programs (`.ppo`, `.vpo`), lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs; the detail-blend programs' equations derived and proven, worlds/fromsoft/tests/detail_derive.rs | the user's own install: the programs extracted from its shader archives |
| `oracles.corpus.skate/shaders` | live | - | a folder (searched recursively) of the DXIL containers of skate.'s CompiledBytecode resources (one file per container), lifted, validated and written as WGSL, the refusals counted by reason with an example program; repos/iridia/iridia-dxil/tests/lift_corpus.rs | the user's own install: the containers of its superbundles' CompiledBytecode resources, worlds/reskate/examples/corpus_skate_shaders.rs |
| `oracles.corpus.sledding/shaders` | live | - | a folder (searched recursively) of the DXBC containers of a Unity player's compiled pixel programs, as primase-unity extracts them, lifted, validated and written as WGSL, the refusals counted by reason; toolkit/dxbc/tests/lift_corpus.rs | the user's own install: the programs read from its shader assets |
| `oracles.d3d-disassembler.riders/shader-pack` | live | - | a folder of DXBC containers `NNNN.dxbc` (Riders Republic's ShaderDataDX11.bin, Steam build 25394987, split at its containers: 2,443) each with `NNNN.txt`, Microsoft's D3D Shader Disassembler text of it; toolkit/dxbc/tests/disassembly.rs | independent: Microsoft's D3D Shader Disassembler, run over the user's own install's pack (kept in a private cache) |

#### Debug keys (`GAME_DATA_DEBUG`)

| key | about |
| --- | --- |
| `shader.diff_limit` | how many programs of a corpus the WARP comparison runs |
| `shader.warp_all` | `1` compares every program the WARP oracle can bind |

<!-- end of real-data inputs -->
