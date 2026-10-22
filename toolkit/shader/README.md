# One pixel-shader evaluation interface

`PixelShader` is what a bake evaluates, whatever the bytecode: `reflection()` (resources, constant-buffer
layouts, input and output signatures, as D3D reflection states them), `input_count()`, `constant_buffers()`
(bind slot and rows of four lanes), `textures()` and `samplers()` (bind slots), and `execute(inputs, buffers,
sampler)`. `primase-dxbc` (`Shader`, Shader Model 5) and `primase-dxil` (`Shader`, Shader Model 6) implement it.

Sampling is the caller's `Sampler` callback (texture and sampler slots, UV): a scalar evaluation has no
derivatives, so the caller states its footprint, addressing, filtering and transfer. An `Execution` holds the
output registers, the mask of lanes the shader wrote, discard status, and instruction and sample counts. Errors name their
bytecode and location.

## Prepared programs

`PreparedProgram<S>` owns a concrete pixel shader, its `Constants`, and textures by bind slot. Its borrowed
`PreparedProgramRef` exposes the same contract without copying resources. Binding implementations use this common
view; source-specific parameter names, input recipes, fallback choices and output interpretation stay with their
producer. Preparing bindings does not execute the shader or bake images.

Inspect `shader()`, `constants()` and `textures()`, then use `as_ref().execute` with explicit input registers or
`evaluate_raw` with an explicit input grid and invocation mode. Raw results retain written masks and discarded
pixels. Quad and derivative-aware invocation are available explicitly; no surface frame is invented. Constant
buffers can be inspected without copying through `Constants::buffers`, `words` and `field`. Their source sizes
are not a promise about a translated GPU layout.

The concrete shader remains available for bytecode-specific analysis. `Texture::Image` is an opaque CPU sampling
callback, not a GPU pixel-storage contract; its implementation owns transfer, addressing and filtering. A caller
can override sampling explicitly for an invocation. No GPU backend, source-game name, or renderer binding policy
belongs in this interface.

## Evaluating over a grid

`evaluate` runs a `PixelShader` once per pixel of a UV grid: the generic part of a material bake. The caller states
everything the program reads and the evaluator adds no policy of its own:

- `Layout`: the buffers the program reads (bind slot, words) and where each named constant lives (`Field`: buffer,
  word offset, words). `Layout::of` builds it from reflection, struct members flattened to their own names (a name
  that occurs twice, or a field off a word boundary, is an error); a caller whose bytecode carries no reflection
  builds it from its own records. Sizes follow D3D packing (`Type::packed_words`).
- `Constants`: the words, zero until set. `set(field, values)` writes a field's leading words (`Ok(false)` for an
  unknown name, an error for more values than the field has); `set_words(buffer, offset, bits)` writes raw words at
  a stated offset; words past the rows the program reads are not stored. `unset_words` counts what stays zero.
- `Texture` per bind slot: an `Image` the caller samples by its own policy, or a constant. A slot the program
  samples without one is an error.
- `Input` per (semantic, index), compared case-insensitively: lanes of raw bits, the grid's U or V, a constant, or
  `GridUv`. Lane k is the element's k-th component wherever the signature places it (a float2 packed into `.zw`
  receives lanes 0 and 1 there). A used input without one is an error.
- `Grid`: pixel (x, y) of size x size is evaluated at U = (x + 0.5) / size * period[0], V likewise, in f32.
- Targets: (output register, component mask). Each comes back as size x size values, or None when the program does
  not write those components; a program that writes them at some pixels only is an error.

Colour-only `evaluate` and `evaluate_periodic` refuse discarded pixels because their results cannot represent
coverage. A caller handling cutouts uses `evaluate_raw` and its per-pixel `discarded` flags, choosing explicitly how
to retain coverage rather than treating a discarded invocation's register values as visible colour.

Tests: one HLSL fixture (`tests/fixtures/evaluate.hlsl`) compiled with fxc (SM5) and dxc (SM6), evaluated through
both implementations against outputs worked out from the HLSL, the refusals, and constant packing worked out from
D3D's rules.
