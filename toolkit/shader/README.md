# Shader programs and the pixel-shader evaluation interface

This crate holds what the lifters share: the program representation (`Program`: a naga module with the interface in
primase's vocabulary, `validate`, `text`, `emit`), the typed `build::Builder` they lift with, the D3D mapping tables
(`d3d`), the executor (`exec`), the refusal type and the version gate (`Refusal`, `gate`), and the older pixel-shader
evaluation interface described first. The program representation, the versions and the support ledger follow it.

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
- What is not mapped is refused by name; the support ledger below counts each lifter's refusals by reason.

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

## The program representation

`Program` is what every bytecode lifts into (`primase-dxbc`'s `lift` today): a `naga::Module` (typed body, structured
control flow, resources as globals; naga 30.0.1 is the IR) with what the module cannot say:

- `interface` (the vocabulary above) and `bindings` (which global each `ResourceId` is). `entry_io(&Interface)` is the
  one statement of how the entry point carries the interface: one argument per input varying, one result member per
  output varying (a vertex stage's clip distances together, one `array<f32, N>`), named `<NAME><index>`, built-in
  bindings for system values, locations in interface order, a pixel `Target(n)` always four lanes. The validator
  checks the module against it.
- `conventions` (clip depth, fragment and texture origin; D3D's are `Conventions::D3D`), applied once where a program
  meets a pipeline, never inside the body.
- `modes` (`D3dModes`): the D3D behaviours the body spells out as plain operations because the IR's and other APIs'
  answers differ (division by zero, float to integer of NaN and overflow, `min`/`max`/`saturate` with a NaN); the
  denormal flush is the source's expectation, which WGSL leaves to the implementation. `build::behaviour` writes them.
- `provenance` (bytecode format, target, lifter and its version) and `notes` (what a lifter approximated).

`validate()` runs naga's validator (with `CAPABILITIES`: WGSL's baseline plus clip distances, per-sample shading, the
primitive index, cube array textures, half-float packing and forced early depth, which a consumer that cannot provide
one refuses) and then ours: interface against entry point, resources against globals (kind, space, binding, layout
size). `text()` is the stable text form (a header of everything above, then the module as WGSL) and `emit::wgsl`
writes WGSL by naga's backend; a writer that cannot express a program refuses it, never approximates. `build::Builder`
is the typed builder lifters share. An operation asked of values that cannot do it (a vector of 2 lanes added to one of 3, a
lane that is not there) does not panic: the builder records the first such fault and hands back a value of the shape it
was asked for, and the lifter asks (`Builder::check`) where it is reading, to refuse the program naming the line or
instruction; a builder with a fault nobody asked for cannot be finished. The builder's assertions are for what a lifter
got wrong itself, such as control flow left open.

## Versions

`PROGRAM_VERSION` and each lifter's `LIFTER_VERSION` are what a cache keyed by a bytecode's hash trusts to know that a
program it holds is stale. They are gated: a lifter's tests (`toolkit/dxbc/tests/versions.rs`,
`toolkit/dxil/tests/versions.rs`, the last test of `toolkit/mojoshader/tests/lift.rs`) lift its committed fixtures and
compare a table of their outcomes (the program's text form hashed, or the refusal) under both versions with the
committed `tests/lifted.hashes`. Output that changed under unchanged versions fails and says which to bump; a bump
fails until the table is replaced by the current one, which the failing test writes to the scratch root
(`<crate>-lifted.hashes`; the failure names the path). The text form includes naga's WGSL, so a naga upgrade that changes it is a bump too. A
program a lifter makes carries `source_hash`, the SHA-256 of the bytes it was given, which the lifter computes.

## Support ledger

What the lifters make of every program of every corpus the project holds, counted by the census tests (`toolkit/dxbc/tests/lift_corpus.rs`, `toolkit/dxil/tests/lift_corpus.rs`, `worlds/gta/tests/fxc_census.rs`, `worlds/stuntboost/tests/effects.rs`) in one run on main `08b8fbe` plus the one-line executor fix in the commit that adds this section (without it the GTA census panics in `Executable::new`: a provenance index past the expressions). Release build, the corpora from the oracle tree (`oracles.corpus.<game>/shaders`) and the STUNTBOOST install's `Content/Effects`; the numbers are the tests' own output, recounted from that run. Lifted is `lift` returning a program; validated is `Program::validate`; written is `emit::wgsl` (or `wgsl_functions` for a stage with no entry point) succeeding; executable is `Executable::new` succeeding. It does not take the function-form stages (hull, domain, geometry and tessellator-feeding vertex programs), which run as functions through `Executable::function`, nor ray tracing pipeline programs, mesh programs and procedural-primitive ray queries, which are refused by name; the column counts the programs `Executable::new` takes.

| corpus (test) | programs | lifted | validated | written as WGSL | refused by the lift | executable |
|---|---:|---:|---:|---:|---:|---:|
| GTA V Enhanced shader database (SGD2; `fxc_census`) | 12,405 | 12,405 | 12,405 | 12,375 | 0 | 11,130 |
| Riders Republic (DXBC; `lift_corpus`) | 2,443 | 2,442 | 2,442 | 2,442 | 1 | 2,416 |
| Unity player (DXBC, `sledding`) | 4,854 | 4,854 | 4,854 | 4,854 | 0 | 4,854 |
| Sekiro (DXBC) | 432 | 432 | 432 | 432 | 0 | 432 |
| Elden Ring (DXIL; 44,450 containers) | 44,450 | 44,450 | 44,450 | 44,441 | 0 | 43,985 |
| skate. (DXIL; 19,445 containers) | 19,445 | 19,443 | 19,443 | 19,138 | 2 | 19,199 |
| STUNTBOOST (MojoShader GLSL in 18 effects) | 47 | 47 | 47 | 47 | 0 | not measured |

Not measured: GTA V Legacy's `.fxc` effects (`oracles.corpus.gta/legacy-effects`; the folder is not on the machine that ran this) and the STUNTBOOST shaders on the executor.

### Refused by the lift

| count | corpus | reason | example (corpus-relative) |
|---:|---|---|---|
| 2 | skate. | the old value of an atomic operation on a texture view (naga's image atomic returns none) | `235911bb-c5c8-bef5-9998-f3e2ea6597fb.dxil` |
| 1 | Riders Republic | the same (`imm_atomic_umin`) | `1378.dxbc` |

### Lifted and validated but not written as WGSL

| count | corpus | reason | example |
|---:|---|---|---|
| 100 | skate. | naga's WGSL writer does not write ray queries (`unreachable!()` on `Statement::RayQuery`; naga 30.0.1 is the newest release, so only a writer of our own or a change to naga lifts it) | `01f9f54b-846f-2a4b-c576-9e1e6f1d8077.dxil` |
| 30 | GTA Enhanced | the same | (the census prints none) |
| 205 | skate. | a ray tracing pipeline program (172 ray generation, 16 closest-hit, 8 miss, 6 intersection, 3 any-hit) has no WGSL form: WebGPU has no ray tracing pipeline | `000e14b5-25b8-967b-532c-1b7f6c7e62cb.dxil` |
| 9 | Elden Ring | the same (3 ray generation, 2 any-hit, 2 closest-hit, 2 miss) | `gxraytracing/GXRayGen.cpo` |

### Not taken by `Executable::new`

| count | corpus | reason |
|---:|---|---|
| 1,275 | GTA Enhanced | function-form stages, run as functions through `Executable::function` (458 domain, 406 hull, 411 vertex programs that feed a tessellator) |
| 26 | Riders Republic | function-form stages, run as functions (9 domain, 9 hull, 7 geometry, 1 vertex) |
| 465 | Elden Ring | function-form stages, run as functions (150 domain, 136 hull, 33 geometry, 137 vertex), and 9 ray tracing pipeline programs refused by name |
| 244 | skate. | 205 ray tracing pipeline programs (no pipeline to run them in), 38 ray queries that generate an intersection with a procedural primitive (scenes carry triangles only), 1 mesh program (a whole thread group writes one meshlet) |

The executable column is the lifted program count less these. The skate. row was measured again after the mesh programs were lifted (main d67b1261's `dxil-mesh`); the other rows are as first measured.
