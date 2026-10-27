# DXIL: lifting

Shader Model 6 containers, read by `dxil-reader` (the container, the LLVM bitcode of the `DXIL` part, the reflection and
runtime-data records). `reflection` maps those records into iridia's interface vocabulary, and `lift` turns a program
of any stage into a `iridia::Program`; running one is `iridia_exec`'s (`ProgramShader` for a pixel or vertex program).
Sampling belongs to the caller of an execution.

## Mapping

- The lifter reads an operation's arguments by position after one check of their count (`src/lift/dxop.rs`, the count
  of each `dx.op` from DXC's `hctdb.py`, equal to how 69,826 modules of the corpora call it), relying on the reader's
  check that a function body names only what the function has. `tests/malformed.rs` makes the cases from committed
  fixtures.
- Reflection (`interface`): input and output signatures from `ISG1`/`OSG1`, their packing (element id, first register
  and column, size) and interpolation from `dx.entryPoints`, resources from `dx.resources` (kind, element type and
  stride tags, space), and constant-buffer layouts (field names, offsets, component types and matrix annotations) from
  `dx.typeAnnotations`, named from the `STAT` module when present (the program's own metadata has no names), mapped
  into iridia's interface vocabulary (`iridia::Interface`). A library's subobjects (`subobjects`) are mapped from its
  runtime data.

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
