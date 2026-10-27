# DXBC: reading and lifting

Shader Model 4 and 5 programs. `decode` reads a program's token stream into typed declarations and instructions (any
stage's version token, instructions with their controls and extended tokens) and refuses, by name and word, what is
not a program; it says nothing about what a consumer supports. `reflection` reads RDEF with the signatures
(`signatures` alone for a container whose RDEF was stripped, as some engines ship them, leaving the constant layout to
the caller's own records). `lift` turns a program of any stage into a `iridia::Program`; running one is
`iridia_exec`'s (`ProgramShader` for a pixel or vertex program). The container itself is read by
`dxcontainer`: its digest is not verified, this reader does not authenticate shaders.

Sampling belongs to the caller of an execution: it states its footprint/LOD, addressing, filtering and transfer
policy, and a scalar execution cannot infer derivatives, so the caller states its inputs'.

`reflection` reads SM5.0 pixel-program RDEF, ISGN and OSGN chunks into primase's interface vocabulary
(`iridia::Interface`, see `repos/iridia/iridia/README.md`): resources by kind with their D3D bind points, constant
buffers with explicit byte offsets, strides and default bytes, and one varying per signature element with its
register and lanes in provenance. D3D's enum numbers stop there; one the mapping does not know is refused by name.
It does not flatten arrays, infer their use or assign material parameters; the interpolation a program declares is
read by `lift`. Record sizes, byte ranges, string lengths, recursion and total type count are bounded; cyclic types and
variables outside their buffer are refused. Stripped reflection is an explicit error for this API, not an invented
binding layout. The binary record layout is also cross-checked against the independent
[RenderDoc DXBC container definitions](https://github.com/baldurk/renderdoc/blob/v1.x/renderdoc/driver/shaders/dxbc/dxbc_container.cpp).

Format definitions: Microsoft's
[tokenized program format](https://github.com/microsoft/DirectX-Headers/blob/main/include/directx/D3D12TokenizedProgramFormat.hpp).
Arithmetic contracts: the
[D3D11 functional specification](https://microsoft.github.io/DirectX-Specs/d3d/archive/D3D11_3_FunctionalSpec.htm),
including its instruction reference. Opcode names/numeric values and bit fields are format identifiers; no
third-party interpreter implementation is included.

## Independent gates

- `tests/analytic.rs`: programs written as tokens, lifted and run on the executor against values worked out by hand from
  Direct3D's definition of each instruction (arithmetic and conversion edges, every numeric opcode, flow, derivatives,
  quads, sample callbacks, relative constant-buffer addressing); nothing is read from data.
- `tests/disassembly.rs`: `oracles.corpus.general/dxbc` selects a directory of `.ppo` binaries with matching `.ppo.asm`
  independent disassemblies (Microsoft's fxc `/dumpbin`) and the `.ppo.reflection.json` that
  `tests/oracles/dxbc_corpus.py` reads from that text: the decoder's complete opcode sequence and count, including
  ret, against the disassembly; the programs the decoder refuses are counted by reason and held to pinned counts (a new
  reason, or more, fails). `oracles.d3d-disassembler.riders/shader-pack` (Microsoft's D3D Shader Disassembler text of
  every container of Riders Republic's pack) gives the opcodes, sample offsets, resource declarations and immediates
  the decoder reads, one by one. Nothing is copied into the repository.
- `tests/exec_warp.rs`: the lifted programs rendered by Microsoft's WARP rasterizer (Direct3D 11) and run on the
  executor, every lane both write compared (needs `tools.warp` and `tools.fxc`).

## Retired evaluator: what it established

The bounded pixel-program interpreter (`Shader`, `Program::execute*`) was removed once the executor ran everything it
did. Its traced value-oracle record stands, for anyone who compares against a text disassembler again: in a controlled
22-program corpus (constant sampled RGBA, fixed raw inputs and constant buffers), 364 written output components were
216 exact and 148 within four f32 epsilon of the oracle's, and two things explained all of the difference. (1) Decimal
literals are lost in a disassembler's text: the first divergent instruction in every trace was a `mul` by binary
`0x3b808081` (0.003921568859368563) printed as `0.003922`, and the G-buffer bias is printed as `0.500489`; with the
exact hexadecimal bits supplied, 331 components were exact and 33 within the bound. Binary literals are authoritative.
(2) Reciprocal square root: for a first divergent `rsq` input `0x3f91d6c7` (1.1393669843673706), f64 sqrt and reciprocal
rounded once give `0x3f6fd523`, an oracle's f32 sqrt then f32 division gives `0x3f6fd524`; changing only that
operation of the oracle to f64 intermediates made all 364 components match exactly. A precision choice, not a format
rule. No residual `mad` contraction, `sqrt`, `log` or `exp` difference was observed in these controls, which does not
establish equality for other inputs.

## Lifting

`lift(container)` turns a Shader Model 4 or 5 pixel or vertex program into a `iridia::Program`. It reads the
program through `decode` (the token-level decoder every consumer shares: any stage's version token, typed
declarations, instructions with their controls and extended tokens) and the container's signatures and, where there is one, its RDEF (names and constant layouts).

A register is four raw lanes (`vec4<u32>` locals); each instruction bitcasts to the type it implies. Varyings are
unpacked from D3D registers (the register and lanes stay in provenance); every element of the container's signatures is in the interface
(input layouts and stage linking are matched against the signature), `used` false for one the program does not declare. Resources are `cb<n>` uniform `array<vec4<u32>, rows>` (group 0), `t<n>` textures and buffer
views (group 1), `s<n>` samplers (group 2), typed texture views `u<n>` as storage images of four 32-bit lanes (group 3; the
view's real format is the binding's, a note on the program says so), binding = slot. Reads out of range of a dynamically indexed buffer are
zero, as in D3D. D3D's results are written out (`D3dModes::ALL`). Refused by name, with the instruction word: compute,
geometry and tessellation programs, unordered-access buffer views (raw, structured, typed) and atomics, a store through a view with a partial write mask, `sample_info`, index ranges, a pixel input of
`SV_RenderTargetArrayIndex`, a vertex program with no `SV_Position`, doubles, SM5.1 declarations.

A malformed program is an `Err`, never a panic or an allocation its declarations ask for: an instruction's operand count is
its opcode's (`src/arity.rs`, checked once before an operand is read by position), a resource operand is the register class
the instruction takes, a declaration is read to its last word, and what a declaration states is within D3D11's limits
(4096 temporaries, 4096 elements in an indexable array, 4096 constant-buffer rows, 32 KiB of thread-group memory, 2048-byte
structured elements, 1024 geometry output vertices, 32 geometry instances). `tests/malformed.rs` makes each case from a
committed fixture.

`tests/lift.rs` lifts our own HLSL fixture (`tests/fixtures/modes.hlsl`, fxc `/T ps_5_0`) and checks its interface,
layout and the D3D guards in the WGSL. `tests/lift_corpus.rs` (`oracles.corpus.<game>/shaders` for sekiro, riders, sledding)
lifts a folder of real containers, validates and writes each, and prints the refusals by reason; a lifted program that
fails validation fails the test. Lifting is checked structurally here; values are checked against WARP and a GPU by the
batch that runs the executor over the lifted programs.
