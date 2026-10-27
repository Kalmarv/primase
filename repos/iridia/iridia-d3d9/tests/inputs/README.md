# Real-data inputs

What this crate's real-data tests read, as tests/inputs/mod.rs declares them: the keys of the configuration file
`GAME_DATA` names (its layout is primase-testdata's), the oracle sets and the files of each, and the debug keys
`GAME_DATA_DEBUG` sets. A test whose input is not configured skips with a line naming its key.

<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->

#### Configuration keys

| key | gate | about | source |
| --- | --- | --- | --- |
| `scratch.root` | - | where real-data tests write their outputs (the OS temporary folder when unset) | - |

#### Oracle sets (`<oracles.root>/<set>/<game>/`)

| set | about | source |
| --- | --- | --- |
| `oracles.d3d-disassembler` | Microsoft's D3D shader disassembly of a game's shader containers (`NNNN.dxbc` with its listing) | independent: Microsoft's D3D Shader Disassembler, run over the user's own install |

#### Declared files

| key | status | gate | about | source |
| --- | --- | --- | --- | --- |
| `oracles.d3d-disassembler.general/sm3` | live | yes | privately extracted raw ps_3_0 (`pixel.bin`) and two vs_3_0 programs (`vertex.bin`, `vertex-layered.bin`), each with Microsoft's `.asm` disassembly; repos/iridia/iridia-d3d9/tests/disassembly.rs checks all executable operands, declarations, definitions and CTAB allocations | independent: Microsoft d3dcompiler_47.dll D3DDisassemble via repos/iridia/iridia-d3d9/tests/oracles/disassemble.py, on a shipped Shaun White Skateboarding SDB2 v39 pixel shader (record 5) and vertex shaders (records 5 and 150, ekshadersPChigh.bin); raw bytes and disassembly stay outside the repository |

<!-- end of real-data inputs -->
