# Bounded DXBC evaluation

`Program::parse` reads a checked DXBC container; `Program::from_words` accepts
a complete `ps_4_0`, `ps_4_1` or `ps_5_0` token stream. Compilation rejects
unsupported instructions by name before evaluation, including switch, discard,
explicit-LOD/bias/comparison samples, resources other than 2D and 3D textures,
non-float texture views, nonzero sample offsets, reduced precision and dynamic
addressing outside constant-buffer elements. A final `ret` is required, outside
any `if` or `loop`. Word count, register counts, index recursion and accesses
are bounded, and so are the instructions one evaluation executes (2^22: a loop
whose exit depends on values cannot run forever). Duplicate code chunks and
overlapping container chunks are errors. The container digest is not verified;
this parser does not authenticate shaders.

Supported base instructions: add, and, div, dp2/3/4, eq, exp, frc, ftoi, ftou,
ge, iadd, ieq, ige, ilt, imad, imax, imin, imul, ine, ineg, ishl, ishr, itof,
log, lt, mad, max, min, mov, movc, mul, ne, not, or, ret, round_ne, round_ni,
round_pi, round_z, rsq, sample, sample_d, sincos, sqrt, uge, ult, umax, umin,
ushr, utof, xor; the screen derivatives deriv_rtx and deriv_rty (and their SM5
coarse and fine forms); and structured flow control: if/else/endif, loop/endloop,
break, breakc, continue, continuec (a conditional takes its `_nz` or `_z` test
of all 32 bits). Flow is matched when the program is read: an else, endif or
endloop without its opener, a break or continue outside a loop, or an unclosed
block is an error. Float destinations support saturation. Registers preserve raw u32 lanes;
source modifiers and swizzles are applied before any destination write.
An unwritten temporary component, missing binding or out-of-bounds dynamic
buffer access is an error rather than a fabricated value. Unwritten output
lanes are identified by `Execution::output_masks` and are not shader results.

Sampling is a caller callback receiving texture/sampler registers and UVs, and
`sample_d`'s gradients as the program states them. There is no implicit
derivative calculation in a scalar execution: `execute_with_derivatives` takes
each input register's screen derivatives (d/dx, d/dy), which the derivative
instructions read; they are refused at compilation on anything but an unmodified
input register, and an evaluation without the derivatives they read is an error.
The caller must state its footprint/LOD, addressing, filtering and transfer
policy. A 3D texture may be declared (a branch the caller's constants never take
may sample one), but sampling one at run time is an error.

`reflection` reads RDEF with the signatures; a container whose RDEF was stripped
(as some engines ship them) is read with `signatures` and `Shader::parse_stripped`,
leaving the constant layout to the caller's own records.
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
| min, max | one NaN selects the non-NaN operand |
| frc | x - floor(x), in f32 |
| round_ni, round_pi, round_z | floor, ceiling, truncation; signed zero retained |
| ftoi | truncate toward zero; saturate to the i32 range; NaN becomes 0 |
| itof | i32 to f32 rounded nearest-even |
| sincos | host f64 sine and cosine rounded to f32 |
| eq, ne, ge, lt | IEEE comparisons (NaN unordered: only ne holds) as all-ones or zero masks |
| integer comparisons, iadd, ineg, shifts | wrapping two's complement; shift counts masked to 0..31 |
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

`reflection` reads SM5.0 pixel-program RDEF, ISGN and OSGN chunks into primase's interface vocabulary
(`primase_shader::Interface`, see `toolkit/shader/README.md`): resources by kind with their D3D bind points, constant
buffers with explicit byte offsets, strides and default bytes, and one varying per signature element with its
register and lanes in provenance. D3D's enum numbers stop there; one the mapping does not know is refused by name.
`Shader::parse` also gives each input the interpolation the program declares. It does not flatten arrays, infer
their use or assign material parameters.
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

## Lifting

`lift(container)` turns a Shader Model 4 or 5 pixel or vertex program into a `primase_shader::Program`. It reads the
program through `decode` (the token-level decoder every consumer shares: any stage's version token, typed
declarations, instructions with their controls and extended tokens; `Program::from_words` is that plus the evaluator's
subset checks) and the container's signatures and, where there is one, its RDEF (names and constant layouts).

A register is four raw lanes (`vec4<u32>` locals); each instruction bitcasts to the type it implies. Varyings are
unpacked from D3D registers (the register and lanes stay in provenance); only signature elements the program declares
are in the interface. Resources are `cb<n>` uniform `array<vec4<u32>, rows>` (group 0), `t<n>` textures and buffer
views (group 1), `s<n>` samplers (group 2), binding = slot. Reads out of range of a dynamically indexed buffer are
zero, as in D3D. D3D's results are written out (`D3dModes::ALL`). Refused by name, with the instruction word: compute,
geometry and tessellation programs, unordered-access views and atomics, `sample_info`, index ranges, a pixel input of
`SV_RenderTargetArrayIndex`, a vertex program with no `SV_Position`, doubles, SM5.1 declarations.

`tests/lift.rs` lifts our own HLSL fixture (`tests/fixtures/modes.hlsl`, fxc `/T ps_5_0`) and checks its interface,
layout and the D3D guards in the WGSL. `tests/lift_corpus.rs` (`PRIMASE_SHADER_CORPUS_SEKIRO`, `_RR`, `_UNITY`)
lifts a folder of real containers, validates and writes each, and prints the refusals by reason; a lifted program that
fails validation fails the test. Lifting is checked structurally here; values are checked against WARP and a GPU by the
batch that runs the executor over the lifted programs.
