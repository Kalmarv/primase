# Shader programs

This crate holds what the lifters share: the program representation (`Program`: a naga module with the interface in
primase's vocabulary, `validate`, `text`, `emit`), the typed `build::Builder` they lift with, the D3D mapping tables
(`d3d`), the refusal type and the version gate (`Refusal`, `gate`). Running a program is `primase-shader-exec`, which
depends on this crate; the lifters (`primase-dxbc`, `primase-dxil`, `primase-mojoshader`) do not. The interface
vocabulary, the program representation, the versions and the support ledger follow.

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

- DXBC's interpolation is declared by the program, not the signature: `primase_dxbc::lift` fills `interp` from the
  `dcl_input_ps` declarations covering an input's lanes; the standalone
  `reflection` and `signatures` leave it `None`. An input the program never declares has none. DXIL states it in the
  entry point's signature metadata and `primase_dxil::interface` fills it.
- A UAV's `access` is `ReadWrite`: neither reflection says more.
- A structured buffer is its stride; the element layout of D3D's "resource bind info" is read past, not kept.
- A DXIL `half` and a DXBC-side min-precision type are read as 32-bit; DXIL min-precision signature annotations are not
  read.
- What is not mapped is refused by name; the support ledger below counts each lifter's refusals by reason.

## The program representation

`Program` is what every bytecode lifts into (`primase-dxbc`'s `lift` today): a `naga::Module` (typed body, structured
control flow, resources as globals; naga 30.0.1 is the IR: the release plus three changes it lacks, from the `naga-30.0.1-primase` branch of `github.com/Kalmarv/wgpu`, pinned in the workspace `[patch.crates-io]`: the WGSL writer writes ray queries, and `Statement::ImageAtomic` has an optional result (the texel's previous value) and accepts `Exchange`) with what the module cannot say:

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
size). `text()` is the stable text form (a header of everything above, then the module as WGSL; for a program WGSL cannot express, a texture atomic's old value or an exchange, the module as naga's IR dump under a line that says it is not WGSL and why, so no lifted program is without a text form) and `emit::wgsl`
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

What the lifters make of every program of every corpus the project holds, counted by the census tests (`toolkit/dxbc/tests/lift_corpus.rs`, `toolkit/dxil/tests/lift_corpus.rs`, `worlds/gta/tests/fxc_census.rs`, `worlds/stuntboost/tests/effects.rs`) in one release run on main `11fe408a` plus the commit that switches naga to the fork (the STUNTBOOST row was not re-run: it is as first measured, and the fork changes nothing it uses). The corpora are from the oracle tree (`oracles.corpus.<game>/shaders`; the Elden Ring folder holds 50,283 containers today, the row it replaces counted 44,450) and the STUNTBOOST install's `Content/Effects`; the numbers are the tests' own output. Lifted is `lift` returning a program; validated is `Program::validate`; written is `emit::wgsl` (or `wgsl_functions` for a stage with no entry point) succeeding. A lifted program is executable in one of two forms: as an entry point (`primase_shader_exec::Executable::new`: pixel, vertex, compute and mesh programs) or as its functions (`primase_shader_exec::Executable::functions`, which compiles each with `primase_shader_exec::Executable::function`: hull, domain and geometry programs, vertex programs that feed a tessellator, and the ray tracing pipeline's programs, which `primase_shader_exec::Pipeline` runs). No lifted program of any corpus is refused by the executor.

| corpus (test) | programs | lifted | validated | written as WGSL | refused by the lift | executable as an entry point | executable as functions |
|---|---:|---:|---:|---:|---:|---:|---:|
| GTA V Enhanced shader database (SGD2; `fxc_census`) | 12,405 | 12,405 | 12,405 | 12,405 | 0 | 11,130 | 1,275 |
| Riders Republic (DXBC; `lift_corpus`) | 2,443 | 2,443 | 2,443 | 2,442 | 0 | 2,417 | 26 |
| Unity player (DXBC, `sledding`) | 4,854 | 4,854 | 4,854 | 4,854 | 0 | 4,854 | 0 |
| Sekiro (DXBC) | 432 | 432 | 432 | 432 | 0 | 432 | 0 |
| Elden Ring (DXIL) | 50,283 | 50,283 | 50,283 | 46,002 | 0 | 45,546 | 4,737 |
| skate. (DXIL) | 19,445 | 19,445 | 19,445 | 19,238 | 0 | 19,240 | 205 |
| STUNTBOOST (MojoShader GLSL in 18 effects) | 47 | 47 | 47 | 47 | 0 | 47 | 0 |

Not measured: GTA V Legacy's `.fxc` effects (`oracles.corpus.gta/legacy-effects`; the folder is not on the machine that ran this).

### Refused by the lift

None: no program of any corpus is refused by a lifter.

### Lifted and validated but not written as WGSL

| count | corpus | reason | example |
|---:|---|---|---|
| 2 | skate. | a texture atomic whose old value the program reads (one of them an exchange): WGSL's texture atomics return nothing and have no exchange (naga's writer: "Unsupported texture atomic result") | `235911bb-c5c8-bef5-9998-f3e2ea6597fb.dxil`, `6f244d9d-58a9-1d4a-0a22-6f2ea461df18.dxil` |
| 1 | Riders Republic | the same (`imm_atomic_umin` with its old value) | `1378.dxbc` |
| 205 | skate. | a ray tracing pipeline program (172 ray generation, 16 closest-hit, 8 miss, 6 intersection, 3 any-hit) has no WGSL form: WebGPU has no ray tracing pipeline | `000e14b5-25b8-967b-532c-1b7f6c7e62cb.dxil` |
| 4,281 | Elden Ring | the same (3 ray generation, 2,138 any-hit, 2,138 closest-hit, 2 miss) | `gxraytracing/GXRayGen.cpo` |

### Executable as functions

| count | corpus | stages |
|---:|---|---|
| 1,275 | GTA Enhanced | 458 domain, 406 hull, 411 vertex programs that feed a tessellator |
| 26 | Riders Republic | 9 domain, 9 hull, 7 geometry, 1 vertex |
| 4,737 | Elden Ring | 150 domain, 136 hull, 33 geometry, 137 vertex, and 4,281 ray tracing pipeline programs (2,138 any-hit, 2,138 closest-hit, 2 miss, 3 ray generation) |
| 205 | skate. | ray tracing pipeline programs (172 ray generation, 16 closest-hit, 8 miss, 6 intersection, 3 any-hit) |

A mesh program (skate. has one) is an entry point: `primase_shader_exec::mesh::MeshShader` runs each of its thread groups in lockstep and returns the meshlet it wrote. The census prints no refusal by the executor; the ledger it replaces counted 38 ray-query programs with procedural intersections and this mesh program as refused here.
