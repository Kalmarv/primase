# Shader execution

Bounded CPU execution of a `iridia::Program`, whatever bytecode it was lifted from (`iridia` holds the
program, its validation and the lifters' builder; this crate depends on it, the lifters do not). `Executable::new` checks
and compiles an entry-point program, `Executable::functions` the functions of a hull, domain, geometry or ray tracing
program; a `Runner` executes one invocation, a 2 x 2 quad, or several invocations of their own in lockstep (masked
where they diverge, each with its own steps and budget) against a `Backend` that answers what is outside the program
(samples, loads, queries, unordered-access effects); `Pipeline` runs a ray tracing pipeline; `mesh`, `geometry`,
`tessellation` and `tessellator` run those stages. The crate documentation has the run rules.

`PixelShader` is what a bake evaluates, whatever the bytecode: `interface()` (the program's varyings and resources in
primase's own vocabulary, `repos/iridia/iridia/README.md`), `input_count()`, `constant_buffers()` (bind slot and rows of four lanes),
`textures()` and `samplers()` (bind slots), and `execute(inputs, buffers, sampler)`. `iridia_exec::ProgramShader` puts a lifted program (`iridia-dxbc`, Shader Model 5; `primase-dxil`, Shader Model 6) behind it.
`execute_lanes` evaluates invocations that share their inputs' derivatives, each as it evaluates alone (outputs,
instructions, samples; on an error, the first failing one's): `ProgramShader` runs them 16 at a time in lockstep, and
runs a group that stops again one invocation after another, so the error is the one alone gives.

Sampling is the caller's `Sampler` callback (texture and sampler slots, UV): a scalar evaluation has no
derivatives, so the caller states its footprint, addressing, filtering and transfer. An `Execution` holds the
output registers, the mask of lanes the shader wrote, and instruction and sample counts. Errors name their
bytecode and location.

## Evaluating over a grid

`evaluate` runs a `PixelShader` once per pixel of a UV grid: the generic part of a material bake. The caller states
everything the program reads and the evaluator adds no policy of its own:

- `Layout`: the buffers the program reads (bind slot, words) and where each named constant lives (`Field`: buffer,
  word offset, words). `Layout::of` builds it from the interface, struct members flattened to their own names (a name
  that occurs twice, or a field off a word boundary, is an error); a caller whose bytecode carries no reflection
  builds it from its own records. A field's words are its type's extent (`ConstantType::size()`).
- `Constants`: the words, zero until set. `set(field, values)` writes a field's leading words (`Ok(false)` for an
  unknown name, an error for more values than the field has); `set_words(buffer, offset, bits)` writes raw words at
  a stated offset; words past the rows the program reads are not stored. `unset_words` counts what stays zero.
- `Texture` per bind slot: an `Image` the caller samples by its own policy, or a constant. A slot the program
  samples without one is an error.
- `Input` per (semantic, index), compared case-insensitively: lanes of raw bits, the grid's U or V, a constant, or
  `GridUv`. Lane k is the element's k-th component wherever the signature places it (a float2 packed into `.zw`
  receives lanes 0 and 1 there). A used input without one is an error. The evaluation still runs on D3D's register
  model, so it reads each input's register and lanes from `provenance.d3d`.
- `Grid`: pixel (x, y) of size x size is evaluated at U = (x + 0.5) / size * period[0], V likewise, in f32. A scalar
  evaluation runs each row's pixels through `execute_lanes` (their derivatives are the grid's steps).
- Targets: (output register, component mask). Each comes back as size x size values, or None when the program does
  not write those components; a program that writes them at some pixels only is an error.

Tests: `tests/interface.rs` reads two HLSL fixtures (`tests/fixtures/resources.hlsl`, `varyings.hlsl`) compiled with
fxc (SM5) and dxc (SM6) and holds the vocabulary read from either to the same values worked out from the HLSL and fxc's
own listing of the buffer layouts. `tests/evaluate.rs` evaluates `tests/fixtures/evaluate.hlsl` through both
implementations against outputs worked out from the HLSL, the refusals, and constant packing worked out from D3D's
rules. `tests/lanes.rs` runs `tests/fixtures/lanes.hlsl`, whose pixels diverge (a branch with a sample, loops of their
own lengths, a discard, a pixel past the step limit), in lockstep against one pixel after another.

