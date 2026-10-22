# One pixel-shader evaluation interface

`PixelShader` is what a bake evaluates, whatever the bytecode: `interface()` (the program's varyings and resources in
primase's own vocabulary, below), `input_count()`, `constant_buffers()` (bind slot and rows of four lanes),
`textures()` and `samplers()` (bind slots), and `execute(inputs, buffers, sampler)`. `primase-dxbc` (`Shader`, Shader
Model 5) and `primase-dxil` (`Shader`, Shader Model 6) implement it.

Sampling is the caller's `Sampler` callback (texture and sampler slots, UV): a scalar evaluation has no
derivatives, so the caller states its footprint, addressing, filtering and transfer. An `Execution` holds the
output registers, the mask of lanes the shader wrote, and instruction and sample counts. Errors name their
bytecode and location.

## The interface vocabulary

What a shader program exposes is described in primase's own terms, independent of any graphics API. The types live in
core (`primase_core::shader`, because `core.shader` will carry them) and are re-exported here, with `Interface` as the
entry point. They are plain serializable data: owned strings, integers and enums.

- **`Interface { stage, inputs, outputs, resources }`.**
- **`Varying { label, system, value, interp, used, provenance }`**, one per label. The `label` is the source's own
  semantic name and index (`TEXCOORD` and 5); labels are not interpreted. D3D packs several varyings into one
  register; here they are separate, and the register and lanes are in `provenance.d3d`. `system` is the
  `SystemValue` when the pipeline supplies or takes it (`ClipPosition`, `FragCoord`, `FrontFacing`, `VertexIndex`,
  `InstanceIndex`, `PrimitiveIndex`, `SampleIndex`, `Target(n)`, `Depth`, `Coverage`, ...); the label keeps the source's
  name for it. `value` is a `ScalarType` and a width of 1 to 4. `interp` is an `Interpolation` of mode (perspective,
  linear, flat) and sampling (centre, centroid, sample), where the source states it for that input; `used` says the
  program reads (an input) or writes (an output) some lane.
- **`Resource { id, kind, count, provenance }`**. `kind` is `ConstantBuffer { layout }`, `Texture { dim, arrayed,
  multisampled, sample }`, `Sampler { comparison }`, `Buffer(Typed(value) | Structured { stride } | Raw)` or
  `Storage { target: Image | Buffer, access }`. `count` is the length of a resource array. `provenance` holds the
  source's name and its D3D bind point (class `b`/`t`/`s`/`u`, register, space).
- **Constant buffers** carry a `BufferLayout`: every constant with an explicit byte offset and a `ConstantType` whose
  matrix lines and array elements have stated strides and matrices their majorness, so D3D packing, std140 and DX9
  registers can all be written in it. `ConstantType::size()` is the extent of the data (no padding after the last
  line or element).

The `d3d` module is where D3D's enum numbers and packing rules become that vocabulary; the DXBC and DXIL readers share
it. A value it does not map (a D3D_NAME, a component type, a resource type with a counter, a tbuffer) is an error naming
it, never a guess.

What the readers keep and drop, so nobody looks for it:

- DXBC's interpolation is declared by the program, not the signature: `primase_dxbc::Shader::parse` (and
  `parse_stripped`) fill `interp` from the `dcl_input_ps` declarations covering an input's lanes; the standalone
  `reflection` and `signatures` leave it `None`. An input the program never declares has none. DXIL states it in the
  entry point's signature metadata and `primase_dxil::interface` fills it.
- A UAV's `access` is `ReadWrite`: neither reflection says more.
- A structured buffer is its stride; the element layout of D3D's "resource bind info" is read past, not kept.
- A DXIL `half` and a DXBC-side min-precision type are read as 32-bit; DXIL min-precision signature annotations are not
  read.
- Not mapped, refused by name: tbuffers, append/consume/counter UAVs, acceleration structures, feedback textures,
  tessellation and compute stages, scalar types other than bool, int, uint, float and double.

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
- `Grid`: pixel (x, y) of size x size is evaluated at U = (x + 0.5) / size * period[0], V likewise, in f32.
- Targets: (output register, component mask). Each comes back as size x size values, or None when the program does
  not write those components; a program that writes them at some pixels only is an error.

Tests: `tests/interface.rs` reads two HLSL fixtures (`tests/fixtures/resources.hlsl`, `varyings.hlsl`) compiled with
fxc (SM5) and dxc (SM6) and holds the vocabulary read from either to the same values worked out from the HLSL and fxc's
own listing of the buffer layouts. `tests/evaluate.rs` evaluates `tests/fixtures/evaluate.hlsl` through both
implementations against outputs worked out from the HLSL, the refusals, and constant packing worked out from D3D's
rules.
