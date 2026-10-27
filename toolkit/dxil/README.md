# DXIL: reading and lifting

Shader Model 6 containers. `module` reads the LLVM bitcode of the `DXIL` part (our own bounded bitstream reader),
`reflection` the signatures, resources and constant-buffer layouts, and `lift` turns a program of any stage into a
`iridia::Program`; running one is `iridia_exec`'s (`ProgramShader` for a pixel or vertex program).
Sampling belongs to the caller of an execution.

## Reading

- Container (`dxcontainer`): the DXBC directory format; SM6 parts are not 4-byte aligned. Parts lie inside the
  container, do not overlap, and each kind appears once. The digest is not verified (not an authenticity check).
- Program (`program`): the `DXIL` part's header (program version, size, `DXIL` magic, DXIL version, bitcode
  offset and size); the `STAT` reflection part has the same header.
- Bitstream (`bitstream`): our own bounded reader of LLVM's bitstream container: abbreviations (literal, fixed,
  VBR, array, char6, blob), BLOCKINFO, nesting up to 32 blocks, at most 4096 abbreviations per block and 2^26
  record operands, every length checked against the bits that remain before allocating. (`llvm-bitcode` was
  considered; it sizes allocations from untrusted lengths.)
- Module (`module`): LLVM 3.7 IR as DXIL stores it, relative value ids (module version 1): types, globals and
  functions, constants (no constant expressions), metadata (strings, values, nodes, named nodes), value names and
  function bodies. Instructions: binary operators, casts, comparisons, select, extractvalue and insertvalue, vector
  element and shuffle operations, call, phi, ret, branches, switch, unreachable, alloca, load and store; any other
  record is refused by name.
- A function body names only what the function has: every value an instruction reads is one of the function's (an
  earlier one, or a later one a phi or a loop reads), every branch target and phi predecessor a block of it, every
  instruction in a block, and a call has as many arguments as its callee has parameters. The lifter then reads an
  operation's arguments by position after one check of their count (`src/lift/dxop.rs`, the count of each `dx.op` from DXC's
  `hctdb.py`, equal to how 69,826 modules of the corpora call it). `tests/malformed.rs` makes the cases from committed fixtures.
- Reflection (`reflection`, `interface`): input and output signatures from `ISG1`/`OSG1`, their packing (element id,
  first register and column, size) and interpolation from `dx.entryPoints`, resources from `dx.resources` (kind,
  element type and stride tags, space), and constant-buffer layouts (field names, offsets, component types and
  matrix annotations) from `dx.typeAnnotations`, named from the `STAT` module when present (the program's own
  metadata has no names), mapped into primase's interface vocabulary (`iridia::Interface`).

## Lifting

`lift(container)` lifts a program of any stage a DXIL container holds. The interface lists every element of the
container's signatures, `used` where the program reads or writes a cell of it (the program's own reading: the compiler's
masks are not what is stored). Inputs are signature registers of four lanes, constant buffers rows of four lanes by
bind slot. An operation's arguments are read by position after one check of their count (`src/lift/dxop.rs`).

## Gates

- `tests/fixtures.rs`: our own HLSL (`tests/fixtures/*.hlsl`, compiled with the dxc command in each file):
  the lifted interface (signature packing of every signature element, `used` where the program reads or writes a cell of it, resource names and slots, buffer field names
  and offsets), execution (`iridia_exec::ProgramShader`) against outputs worked out from the HLSL, missing
  bindings refused, every truncation of a fixture refused, and a seeded mutation run (the debug key `dxil.fuzz_runs`,
  default 3000).
- `tests/exec_warp.rs`: the fixtures (blend, ops, flow, loops, switch, arrays, bits, resource arrays, pixel
  operations) rendered by Microsoft's WARP rasterizer (Direct3D 12) through a separate oracle program (`tools.warp`;
  its vertex shader compiled with `tools.dxc`) and executed on the same inputs: lanes must agree bit for bit, the
  float lanes of the fixture that uses `exp2`, `log2` and `rsqrt` within 1e-5 relative.
- Real data: `tests/lift_corpus.rs` lifts a folder of real containers (`oracles.corpus.skate/shaders`, `elden/shaders`),
  validates and writes each and prints the refusals by reason.

## Retired evaluator: what it established

The bounded pixel-program interpreter (`Shader`, with `execute_marked` and `census`) was removed once the executor ran
everything it did. On Elden Ring's G-buffer pixel programs (read by the Elden Ring reader) it covered 2,031 of 2,110 (37
refused for loops, 42 for `alloca`), and each one's instruction count and calls equalled `dxc -dumpbin`'s; evaluated
against WARP on two cases each (inputs and constants from the program's reflection, uniform textures), of 81,240 output
lanes the 30,968 whose value depends on `Exp`, `Log`, `Sin`, `Cos` or `Rsqrt` agreed within 5.7e-6 relative and the others
bit for bit, except 12 that are NaN in both with different payloads (Direct3D leaves NaN payloads unspecified).
