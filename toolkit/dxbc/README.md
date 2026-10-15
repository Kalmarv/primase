# Bounded DXBC evaluation

`Program::parse` reads a checked DXBC container; `Program::from_words` accepts
a complete `ps_5_0` token stream. Compilation rejects unsupported instructions
by name before evaluation, including flow control, explicit-LOD/gradient samples,
non-2D resources, non-float texture views, nonzero sample offsets, reduced
precision and dynamic addressing outside constant-buffer elements. A final
`ret` is required. Word count, register counts, index recursion and accesses
are bounded. Duplicate code chunks and overlapping container chunks are errors.
The container digest is not verified; this parser does not authenticate shaders.

Supported base instructions: add, and, div, dp2/3/4, exp, ftou, imad, imul,
log, mad, min, mov, movc, mul, ne, ret, round_ne, rsq, sample, sqrt, utof.
Float destinations support saturation. Registers preserve raw u32 lanes;
source modifiers and swizzles are applied before any destination write.
An unwritten temporary component, missing binding or out-of-bounds dynamic
buffer access is an error rather than a fabricated value. Unwritten output
lanes are identified by `Execution::output_masks` and are not shader results.

Sampling is a caller callback receiving texture/sampler registers and UVs.
There is no implicit derivative calculation in a scalar execution. The caller
must state its footprint/LOD, addressing, filtering and transfer policy.
The interpreter has no texture cache, image decoder or material defaults.
No output is returned on error, but callbacks may have run before a runtime
binding error. Unsupported bytecode cannot invoke a callback.

## Numerical choices

These are explicit CPU evaluation choices within the bounded subset, not a
claim of bit identity with every GPU:

| Operation | Choice |
|---|---|
| add, mul, div | f32 arithmetic, nearest-even host default |
| mad | separately rounded f32 multiply then add; no contraction |
| dp2/3/4 | lane-order products and sequential additions, each rounded f32 |
| round_ne | nearest integer, ties to even; signed zero retained |
| ftou | truncate toward zero; clamp to 0..u32::MAX; NaN becomes 0 |
| utof | u32 to f32 rounded nearest-even |
| imad, imul | wrapping low 32 bits; imul also returns signed high 32 bits |
| rsq, sqrt | f64 square root and reciprocal, result rounded to f32 |
| log, exp | host f64 log2/exp2 rounded to f32; domain and infinity behavior retained |
| floating denormals | signed-zero-preserving flush at instruction inputs/results and intermediate unfused arithmetic boundaries |
| mov, movc, integer/bit operations | preserve bits, including denormals and NaN payloads |
| min | one NaN selects the non-NaN operand |
| saturation | clamp to [0,1]; NaN becomes zero |

Host transcendental libraries can differ in their final bits; cross-platform
bit identity is not established. Gates compare derived float values under an
explicit bound, while integer/mask behavior is exact. Repeated output is
deterministic for the same executable, inputs and deterministic sampler.

### Traced value-oracle differences

A controlled 22-program corpus (constant sampled RGBA, fixed raw inputs and
constant buffers) produced 364 written output components: 216 exact and 148
within the stated four-epsilon bound. Instruction traces and two isolated
controls explain the differences:

- Decimal-literal loss in the text oracle: the first divergent instruction in
  every original trace was `mul` using binary `0x3b808081`
  (0.003921568859368563), printed as `0.003922` by the disassembler. Another
  affected literal is the G-buffer bias printed as `0.500489`. Supplying exact
  hexadecimal immediate bits to the otherwise unchanged oracle gives 331 exact
  components and 33 within the bound. Binary literals remain authoritative.
- Reciprocal-square-root intermediate rounding: with exact literals, a first
  divergent `rsq` receives `0x3f91d6c7` (1.1393669843673706). f64 sqrt/reciprocal
  rounded once yields `0x3f6fd523`; the oracle's f32 sqrt followed by f32 division
  yields `0x3f6fd524`. A second control changes only that oracle operation to
  f64 intermediates: all 364 output components then match exactly, with no
  divergent instruction trace. This is a precision choice, not a format rule.

No residual `mad` contraction, `sqrt`, `log` or `exp` difference was observed
in these controls. That does not establish equality for other inputs. The
controls are diagnostic variants, not replacements for the unchanged oracle.
The four-epsilon allowance accounts for the oracle's rounded text literals and
its lower-precision reciprocal-square-root intermediate, not an error allowance
needed to reproduce them in this interpreter. We retain the original binary
constants and the more accurate f64 reciprocal-square-root calculation; the
unchanged oracle remains the gate.
`execute_traced` exposes read-only post-instruction registers and initialization
masks so future pixel differences can be attributed rather than hidden by a bound.

Format definitions: Microsoft's
[tokenized program format](https://github.com/microsoft/DirectX-Headers/blob/main/include/directx/D3D12TokenizedProgramFormat.hpp).
Arithmetic contracts: the
[D3D11 functional specification](https://microsoft.github.io/DirectX-Specs/d3d/archive/D3D11_3_FunctionalSpec.htm),
including its instruction reference. Opcode names/numeric values and bit fields
are format identifiers; no third-party interpreter implementation is included.

## Optional independent gates

`PRIMASE_DXBC_SHADERS` selects a directory of `.ppo` binaries with matching
`.ppo.asm` independent disassemblies. The test compares the complete executable
opcode sequence and count, including ret. Nothing is copied into the repository.
An optional `.ppo.state.json` supplies raw u32 arrays `inputs`, `outputs`, and
`buffers` (array indexed by buffer slot; unbound slots are null). Arrays contain
four consecutive words per register. The value oracle uses a constant sampler
returning `[0.25, 0.5, 0.75, 1]`; it tests execution, not a real sampling policy.
Written outputs compare exactly or within four f32 epsilon times
`max(1, abs(reference))`; NaN pairs are counted separately. An unset directory
skips the real-binary gate. The committed analytical tests require no data.

`Reflection::parse` reads SM5.0 pixel-program RDEF, ISGN and OSGN chunks.
It retains resource slots, buffer sizes/default bytes, variable and nested
member offsets, scalar/class/array metadata and input/output semantics.
It does not flatten arrays, infer their use or assign material parameters.
Record sizes, byte ranges, string lengths, recursion and total type count are
bounded; cyclic types and variables outside their buffer are refused. Stripped
reflection is an explicit error for this API, not an invented binding layout.
The binary record layout is also cross-checked against the independent
[RenderDoc DXBC container definitions](https://github.com/baldurk/renderdoc/blob/v1.x/renderdoc/driver/shaders/dxbc/dxbc_container.cpp).

The optional `.ppo.reflection.json` oracle contains `inputs` (semantic plus
index to register), `textures` and `buffers` (name to slot), and `layout`
(buffer name to member name to `{offset, count, type}`). The test checks each
named binding and member offset/array count exactly; nested member offsets
refer to the first enclosing array element. The production reflection retains
the full hierarchy. An optional `.ppo.trace.json` array contains post-instruction
`temps` and `outputs` as flat raw u32 arrays; the value gate reports the first
divergent initialized component and its instruction for diagnosis.
