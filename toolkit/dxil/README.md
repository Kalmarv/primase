# Bounded DXIL evaluation

`Shader::parse` reads a Shader Model 6 container and checks its program against the evaluated subset;
`Shader` implements `primase_shader::PixelShader`, so a caller written against that trait (such as
`primase_shader::evaluate`) evaluates SM5 (`primase-dxbc`) and SM6 programs alike. Sampling is the caller's callback, as in
`primase-dxbc`: a scalar evaluation has no derivatives.

## Reading

- Container (`chunks`): the DXBC directory format; SM6 parts are not 4-byte aligned. Parts lie inside the
  container, do not overlap, and each kind appears once. The digest is not verified (not an authenticity check).
- Program (`program`): the `DXIL` part's header (program version, size, `DXIL` magic, DXIL version, bitcode
  offset and size); the `STAT` reflection part has the same header.
- Bitstream (`bitstream`): our own bounded reader of LLVM's bitstream container: abbreviations (literal, fixed,
  VBR, array, char6, blob), BLOCKINFO, nesting up to 32 blocks, at most 4096 abbreviations per block and 2^26
  record operands, every length checked against the bits that remain before allocating. (`llvm-bitcode` was
  considered; it sizes allocations from untrusted lengths.)
- Module (`module`): LLVM 3.7 IR as DXIL stores it, relative value ids (module version 1): types, globals and
  functions, constants (no constant expressions), metadata (strings, values, nodes, named nodes), value names and
  function bodies. Instructions: binary operators, casts, comparisons, select, extractvalue, insertvalue, call,
  phi, ret and forward-only branches; any other record is refused by name (`getelementptr`, `alloca`, `load`,
  `store`, `switch`, backward branches as loops, ...).
- A function body names only what the function has: every value an instruction reads is one of the function's (an
  earlier one, or a later one a phi or a loop reads), every branch target and phi predecessor a block of it, every
  instruction in a block, and a call has as many arguments as its callee has parameters. The lifter then reads an
  operation's arguments by position after one check of their count (`src/lift/dxop.rs`, the count of each `dx.op` as
  69,826 modules of the corpora call it). `tests/malformed.rs` makes the cases from committed fixtures.
- Reflection (`reflection`, `interface`): input and output signatures from `ISG1`/`OSG1`, their packing (element id,
  first register and column, size) and interpolation from `dx.entryPoints`, resources from `dx.resources` (kind,
  element type and stride tags, space), and constant-buffer layouts (field names, offsets, component types and
  matrix annotations) from `dx.typeAnnotations`, named from the `STAT` module when present (the program's own
  metadata has no names), mapped into primase's interface vocabulary (`primase_shader::Interface`).

## Evaluation

The entry function runs block by block along the taken branches; a phi takes the value of the block control came
from; at most 2^20 instructions run. `dx.op` operations evaluated: LoadInput, StoreOutput, FAbs, Saturate, IsNaN,
IsInf, IsFinite, Cos, Sin, Exp, Frc, Log, Sqrt, Rsqrt, Round_ne/ni/pi/z, FMax, FMin, IMax, IMin, UMax, UMin, FMad,
IMad, UMad, Dot2/3/4, CreateHandle, CBufferLoadLegacy and Sample (2D, implicit LOD, no offsets or clamp). Others
(SampleLevel, SampleBias, derivatives, Discard, ...) are refused by name when the shader is parsed;
`Shader::census` lists every instruction and operation a program uses and what is refused.

Numerical choices, as `primase-dxbc` states them: f32 arithmetic with the host's nearest-even rounding and
subnormal results flushed to zero (sign kept); FMad a separately rounded multiply then add; Dot2/3/4 lane-order
products and sequential additions, each rounded; float-to-unsigned truncates toward zero and clamps (NaN to 0);
Saturate maps NaN to 0. Half and double values are outside the subset.

Inputs are signature registers of four lanes, constant buffers are rows of four lanes by bind slot, textures are
identified by bind slot. A missing input, buffer of the wrong size, undeclared handle, out-of-range index or an
undefined value used in arithmetic is an error; no partial output is returned.

`Shader::execute_marked` also returns, per output register, the lanes whose value depends on an operation Direct3D
leaves to the implementation within a tolerance (Exp, Log, Sin, Cos, Rsqrt); every other lane is exact under the
choices above. `Shader::passthrough_vertex_shader` writes the HLSL of a vertex shader that feeds the program, for
running it on a rasterizer.

## Gates

- `tests/fixtures.rs`: our own HLSL (`tests/fixtures/*.hlsl`, compiled with the dxc command in each file):
  reflection (signature packing, resource names and slots, buffer field names and offsets), evaluation against
  outputs worked out from the HLSL, missing bindings refused, every truncation of a fixture refused, and a seeded
  mutation run (the debug key `dxil.fuzz_runs`, default 3000).
- `tests/warp.rs`: the fixtures rendered by Microsoft's WARP rasterizer (Direct3D 12) through a separate oracle
  program (`tools.warp`; its vertex shader compiled with `tools.dxc`) and evaluated by this crate on the same
  inputs: unmarked lanes must agree bit for bit, marked lanes within 1e-5 relative.
- Real data (Elden Ring's G-buffer pixel programs, read by the Elden Ring reader): of 2,110, 2,031 are inside
  the subset (37 refused for loops, 42 for `alloca`), and each one's instruction count and calls equal `dxc -dumpbin`'s;
  evaluated against WARP on two cases each (inputs and constants from the program's reflection, uniform textures),
  of 81,240 output lanes, the 30,968 marked ones agree within 5.7e-6 relative and the others bit for bit, except 12 that
  are NaN in both with different payloads (Direct3D leaves NaN payloads unspecified; the comparison now accepts them).
