# What primase should take from MLIR (and USD, glTF)

Research note (2026-10-15) behind DESIGN.md §3-4 and the pass manager.

**Sources read** (primary; quoted behaviour below comes from these):
- llvm-project `main` at `61aea712` (2026-10-15): `mlir/docs/` `DialectConversion.md`, `PassManagement.md`,
  `PatternRewriter.md`, `Interfaces.md`, `LangRef.md` (Operations, Properties, Attributes, IR Versioning),
  `DefiningDialects/_index.md` (Discardable Attribute Verification), `SymbolsAndSymbolTables.md`, `Remarks.md`,
  `ActionTracing.md` (Debug Counters); source `mlir/lib/Transforms/Utils/DialectConversion.cpp`
  (`buildLegalizationGraph`, `computeOpLegalizationDepth`, `applyCostModelToPatterns`), `mlir/lib/Pass/Pass.cpp`
  (verifier after each pass), `mlir/include/mlir/Pass/PassManager.h`, `mlir/lib/IR/Verifier.cpp`,
  `mlir/include/mlir/IR/DialectRegistry.h`, `mlir/lib/Tools/mlir-opt/MlirOptMain.cpp` (`doVerifyRoundTrip`),
  `mlir/include/mlir/IR/BuiltinAttributes.td` (`dense_resource`); `llvm/mlir-www` `TestingGuide.md`.
- OpenUSD: glossary (API Schema, IsA Schema), UsdGeom "Encoding Stage UpAxis" and "Linear Units" pages, `UsdStage`
  (`WriteFallbackPrimTypes`).
- glTF 2.0: `Specification.adoc` §"Specifying Extensions", schema `glTFProperty`/`extras`/`glTF.schema.json`,
  `extensions/README.md`, `EXT_texture_webp` README (fallback rules).
- primase: `docs/DESIGN.md`, `CONTRIBUTING.md`, `core/src/{world,components,gate,frame}.rs`.

Status: DESIGN.md §3-4 adopts recommendations 1-9 and the text form (E1-E7); the rest are proposals. Everything
under "primase" and "verdict" is inference unless it quotes our code.

## 0. Summary

MLIR's core idea fits us: extensible namespaced data, verified by its owner, lowered by passes toward a declared
target. Its *machinery* mostly does not fit, because MLIR is built around SSA use-lists, nested regions and
op-local rewrites, and a world is a flat set of entities with cross-references. Take the contracts (verifier after
every pass, legality actions, a deterministic legalization search, instrumentation, a printable round-tripping text
form, a dialect registry), skip the drivers (greedy fixed point, rollback, region nesting).

| # | recommendation | verdict | urgent for the pass manager? |
|---|---|---|---|
| 1 | Pass effects: `requires / provides / removes / modifies` (+ `approximates`); stable pass names with text options | adopt | **adopted** |
| 2 | Pass manager checks a pass's actual component-name changes against its declared effects | adapt | **adopted** |
| 3 | Verifier on the input world, after every pass and before the writer; always on; errors name pass + entity + `Source` | adopt | **adopted** |
| 4 | Writer target = per-name action `Legal / Dynamic(pred) / Illegal / Carry` plus an unknown-name policy; full vs partial as MLIR | adapt | **adopted** |
| 5 | Path search = MLIR's legalization depth (min chain per illegal name, tie-break depth, priority, name), emitted as a textual pipeline that the report records; explicit pipelines are primary | adapt | **adopted** |
| 6 | No greedy fixed-point driver, no rollback; one `Pass` over `&mut World` plus a per-entity adaptor (MLIR's sibling rule, deterministic merge) | skip / adapt | **yes** (shape of trait) |
| 7 | Instrumentation hooks (before/after/failed pass); built-ins: statistics, approximation remarks, timing (outside deterministic output), print-after(-change), reproducer | adopt | hooks yes, built-ins later |
| 8 | An explicit `Registry` of dialects: per component name, version, required flag, verifier, entity-ref visitor, text codec, interfaces; opaque values in a registered namespace are an error | adopt | soon (core API) |
| 9 | Components enumerate their `Entity` references (dangling-ref check, printing, GC) | adopt | soon (core API) |
| 10 | Analyses: defer; when needed, cache keyed by the component names they read, invalidated by declared effects | adapt later | no (but #1 makes it possible) |
| 11 | Interfaces: narrow read-only queries via the registry; conversions stay passes ("give me a mesh" is a lowering pass) | adapt | no |
| 12 | World text form (print / parse / round trip) + golden and diagnostic tests | adopt | printer early, as debug hook |
| 13 | v2: each component kind marked **required** or **optional** (glTF `extensionsRequired`); a reader meeting an unknown required kind fails | adopt | v2 SPEC  |
| 14 | Keep one fixed world frame (not USD-style per-stage units); USD writer always authors `metersPerUnit = 1`, `upAxis = "Y"` | keep / note | no |
| 15 | "Lower and keep" (USD fallback prim types, glTF fallback images): a lowering may keep its source; writers `Carry` it | adopt | via #4 |
| 16 | No multiple-apply components; one component per name per entity (use child entities or a `Vec`) | keep | no |
| 17 | Merging passes merge provenance (MLIR fused locations) | adopt later | no |

Found in current core while reading (to fix regardless):
- `World::insert_opaque` refuses a name only if a *typed* column for it was already created **in this world**
  (`names` is filled lazily by `register`), so `insert_opaque(e, "core.mesh", ..)` on a fresh world succeeds and the
  name then exists both typed and opaque (verified with a throwaway test: opaque `core.mesh` on e0,
  typed `Mesh` on e1, both present, `component_names()` lists `core.mesh` once). A registry (#8) closes this.
- `despawn` leaves `Instance.prototype` / `Primitive.material` pointing at dead ids and nothing detects it; #3 and
  #9 close this.
- DESIGN §3 lists `Frame` as a world resource, while `frame.rs` and CONTRIBUTING say the world frame is fixed. #14.

## 1. Dialects and registration

| | |
|---|---|
| MLIR | A dialect is a namespace (`let name = "my_dialect"`) owning ops, attributes, types. A `DialectRegistry` holds constructors; the context loads dialects lazily as the parser meets them. Ops of an unregistered dialect are an error unless `allowUnregisteredDialects()`; a registered dialect may still `allowsUnknownOperations()` (Verifier.cpp 383-405). Passes declare `getDependentDialects()` they may create. `DialectExtension`s attach extra behaviour once all their required dialects are loaded. Dialect versioning: `BytecodeDialectInterface::upgradeFromVersion` (bytecode only). |
| primase | Namespaced components (`core.`, `fromsoft.`, `skate.`, `pbr.`), each owned by one crate. Registration is implicit: a type's name enters `World::names` on first `insert`. Opaque components hold anything unknown. `Component::VERSION` exists, no upgrade path. |
| verdict | **Adopt an explicit registry**, as a value (not a global static: tests and determinism). Each crate exposes `pub fn register(reg: &mut Registry)`; the CLI composes the registries of the crates it links. Per component name the registry holds: version, `required` flag (#13), verifier, entity-ref visitor (#9), text codec (#12), interface impls (#11). Rules: an opaque value whose namespace is registered is an error (strict readers), except for a *newer version* of a known name, which is also an error with "written by a newer primase" (we are strict, and carrying it blindly would let a writer drop it). A known older version goes through an explicit upgrade function or fails; never a silent reinterpretation. Passes declare the dialects they produce implicitly via `provides` (no separate `dependent dialects`). Skip `DialectExtension`: the crate rule (dependencies point down) makes it unnecessary. |

## 2. Ops and attributes vs ECS components

| | |
|---|---|
| MLIR | An op has a name (its type), operands/results (SSA), properties (inherent attributes, verified by the op), a dictionary of *discardable* attributes whose name carries a dialect prefix and which **the prefix's dialect verifies** (`verifyOperationAttribute`, Verifier.cpp 299-309), and regions. References to non-SSA things go through symbols (named ops in a symbol table). |
| USD | Same split: one IsA (typed) schema per prim via `typeName`; any number of applied API schemas ("has a"), recorded in the prim's `apiSchemas` metadata, single- or multiple-apply (with instance names). |
| primase | Entities have no type: every component is "has a", like discardable attributes or applied API schemas. Roles (prototype, placement, material, texture) are "by convention, not enforced by the store" (components.rs header). References are `Entity` fields (no SSA, no use-lists). |
| verdict | **Keep has-a; enforce roles in the verifier, not the store.** The MLIR lesson is the verification split: core verifies the role rules (`core.instance` needs `core.transform` on the same entity and its `prototype` must be alive and have `core.mesh` or `core.mesh_file`; `Primitive.material` alive and carrying some material component), and each dialect verifies its own components, exactly as a dialect verifies its prefixed attributes. **Skip multiple-apply** (#16): it needs instance names in every key; a `Vec` inside the component or child entities do the job. **Skip symbols**: our ids are already stable and never reused; we need what symbols give MLIR for free instead: an enumerable reference list (#9). |

## 3. The verifier

| | |
|---|---|
| MLIR | `PassManager` construction sets `verifyPasses(true)` (Pass.cpp 1023, 1028): the verifier runs after every pass unless disabled with `enableVerifier(false)`. It is skipped only when the pass marked all analyses preserved, i.e. promised it changed nothing (Pass.cpp 631-640; always run under `EXPENSIVE_CHECKS`). Verification = op invariants + region invariants + dialect checks on discardable attributes + unregistered-op policy. A failed verify fails the pipeline like a pass failure. |
| primase | Principle 4 and DESIGN §6 "world validators" exist as ideas; nothing runs them between passes. |
| verdict | **Adopt, stricter than MLIR.** `verify(&World, &Registry) -> Report` runs: on the reader's output (attributes the bug to the reader), after every pass, and before the writer. Always on: no flag (CONTRIBUTING: never a fast path that skips a check). Cost control as MLIR does it, but from declarations: after a pass, re-verify only the component kinds it provides/modifies plus the reference check; a full verify at the start and end. Core checks: references alive and of the right role; `Primitive` streams equal length, `Position` present and `F32x3`, indices `< vertex_count`, finite positions; `Texture.levels` sizes per `TexFormat::level_size`; `Transform` finite; `Bounds.min <= max`. Dialect checks via the registry. Error text: `after pass bake-fromsoft-layered: e12 (m11_01/o001000) core.instance.prototype -> e99: not alive`. Identity rules ("one name -> one payload") are verifier rules of the dialect that owns the names (`skate.`). |

## 4. `ConversionTarget` legality; partial vs full

| | |
|---|---|
| MLIR | Per op or dialect: **Legal**, **Dynamic** (a callback decides per instance, e.g. `arith.addi` legal only on i32), **Illegal**; anything unmarked is "unknown". Partial conversion legalises what it can and lets unknown ops remain; full conversion fails unless every op ends legal; **analysis conversion** reports what *would* be legalised without changing the IR. `markOpRecursivelyLegal` exempts nested ops. |
| glTF | `extensionsUsed` (may be ignored) vs `extensionsRequired` (subset; must be understood to load). `extras`: "application-specific data", no schema. `EXT_texture_webp`: with a PNG fallback the extension must *not* be required. |
| primase | Writers declare legal component names; partial keeps unknown, full requires all legal (DESIGN §4). DESIGN §7: the v1 glTF writer keeps dialect data in extras next to baked PBR. |
| verdict | **Adapt: four actions plus an unknown policy.** `Legal` (the writer consumes it), `Dynamic(fn(&World, Entity) -> Result<(), Reason>)` (format and shape limits are per value: skate. takes `core.texture` only in BC1/3/4/5/7; glTF needs a `Transform` "decomposable to TRS" (glTF spec "MUST be decomposable"), so `Sheared` is illegal there; triangle-only targets), `Illegal` (must be lowered), and `Carry` (written through untouched: glTF extras, skate-scene extras, our v2). Unknown names (no action): full conversion = error; partial = the target's `on_unknown` (`Carry` if the format can hold opaque data, else an error; a writer that drops data says so by count, never silently). The search plans on names; `Dynamic` is checked per entity in the final legality check, and a failure there names the entity and the reason. Expose analysis conversion as a dry run: `primase plan --to gltf` prints the pipeline and anything still illegal. |

## 5. Rewrite patterns and the greedy driver vs explicit passes

| | |
|---|---|
| MLIR | A pattern has a static **benefit** and an optional **root op**. Drivers: the walk driver (one post-order pass, no revisits), the **greedy** driver (worklist to a fixed point or an iteration cap; used by canonicalize) and the **conversion** driver (legality-directed, optionally with rollback, which the doc discourages: "significant toll on compilation time, is error-prone and makes debugging ... more complicated"). Patterns must route every change through the rewriter so the driver can track it. |
| primase | Passes over the whole world; most lowerings are per entity (bake one material, encode one texture, weld one mesh), some are global (tiling, decal order across entities, roof smoothing over neighbours, identity validation). |
| verdict | **Skip greedy and rollback.** Fixed-point rewriting makes the result depend on pattern order and termination on iteration caps; we need byte-identical output and a report of each approximation, which explicit passes give directly. Rollback needs every mutation routed through a tracking rewriter; our passes mutate columns directly. **Adapt the walk driver as a helper**: `EntityPass` (a query plus a per-entity function) run by an adaptor that may parallelise and then applies results in entity order. Its rule is MLIR's operation-pass rule: read your own entity and (read-only) what it references; write only your own entity's components. Parallel must give the same bytes as serial (CONTRIBUTING), and a test proves it per pass. |

## 6. The pass manager

| | |
|---|---|
| MLIR nesting | `OpPassManager`s nest on `IsolatedFromAbove` ops (module, func); a nested pipeline runs per op, in parallel. |
| MLIR analyses | Lazy, cached, computed from an op; **every pass invalidates all analyses** unless it marks some preserved; an analysis may refine via `isInvalidated`. |
| MLIR instrumentation | `PassInstrumentation`: `runBefore/AfterPipeline`, `runBeforePass`, `runAfterPass`, `runAfterPassFailed`, `runBefore/AfterAnalysis`, stacked. Built-ins: timing (list/tree), IR printing before/after/after-change (change detected by hashing the op), statistics (per pass instance, named counters), crash/failure **reproducer** (pipeline text + input IR, or "local": IR right before the failing pass), remarks (Passed / Missed / Failure / Analysis), debug counters (skip=N,count=M on an action, to bisect which application breaks things). Passes have a registered argument name and options; pipelines have a textual form (`builtin.module(func.func(cse,canonicalize))`) that round-trips via `printAsTextualPipeline`. |
| primase | Pass manager in progress (W1 §2): requires/provides/removes, approximations, explicit pipeline or path search, a run report. |
| verdict | **Nesting: skip.** Entities reference each other, so nothing is isolated from above; per-entity parallelism lives in the adaptor (#6). Revisit per-tile sub-worlds when GTA-scale runs need streaming. **Analyses: defer**, but design for them now through effects: an analysis declares the component names it reads; it stays valid until a pass provides, removes or **modifies** one of them. That is MLIR's invalidation computed from declarations instead of trusted per pass. Candidates when they're needed: reference graph (who points at e), material->texture use, the identity index. **Instrumentation: adopt** the hook trait now; built-ins in this order: statistics (`PassContext::count("welded_vertices", n)`) and approximation remarks (entity, `Source`, kind, text) into the run report; timing kept *outside* the deterministic report (wall time is not reproducible); `--print-world-after <pass>` and `--print-world-after-change` using the text form (#12) and a content hash (`sha2`); reproducer = pipeline text + the world before the failing pass in v2 or text form; a per-entity counter (`--only-entities skip=N,count=M`) to bisect which entity a pass breaks. **Textual pipelines: adopt**: `load-texture-files,bake-fromsoft-layered{size=1024},tex-encode{format=bc7}`. The path search prints its result in this form, the report records it, and golden tests pin it. |

The path search, made concrete (MLIR `buildLegalizationGraph` / `computeOpLegalizationDepth`, adapted to name sets):
1. `available` = the world's names, closed forward under passes whose `requires` are all available.
2. Backward from the target: `depth(n) = 0` if `n` is Legal/Carry/(allowed unknown); otherwise
   `min` over passes `p` with `n` in `removes(p)` and `requires(p)` within `available` of
   `1 + max(depth(v) for v in provides(p))`. Unreachable = infinite. (MLIR: depth of a pattern = 1 + max depth of
   the ops it generates; patterns sorted by depth, then benefit, stable.)
3. For each illegal name present, pick its min-depth pass; ties by explicit priority, then pass name (MLIR's stable
   sort uses insertion order; ours must not depend on link or registration order). Add the producers of each
   chosen pass's `requires` the same way.
4. Order: `p` before `q` if `p` provides what `q` requires, and `q` before `p` if `p` removes what `q` requires.
   A cycle is an error naming the passes. Remaining ties by name.
5. Simulate the pipeline on name sets (MLIR's analysis conversion): each step's `requires` present, final names all
   legal; else fail naming the first missing link ("no pass removes `fromsoft.layered_material` with outputs legal
   for `stl`; candidates: bake-fromsoft-layered -> `pbr.material` (illegal for `stl`)").
6. Run it as an explicit pipeline. The plan is static; `Dynamic` legality and pass failures surface at run time,
   with entity names. No backtracking.

## 7. Interfaces

| | |
|---|---|
| MLIR | Op/attribute/type interfaces (concept/model, virtual dispatch) and dialect interfaces let passes work on ops they don't know. **External models** attach an interface to another dialect's op; `declarePromisedInterface` makes a forgotten registration a hard error instead of a silent miss. |
| primase | Want: "give me a mesh" across dialects (collision soups, Havok shapes, core meshes), "which textures does this material use" (skate., pbr., fromsoft. materials), entity references. |
| verdict | **Adapt narrowly.** Interfaces are read-only queries, registered as function pointers per component name (this also avoids Rust's orphan rule, the reason MLIR has external models). First three: `refs` (#9), `material_textures` (dependency closure for writers, identity rules), `describe` (the text form's comment line). **"Give me a mesh" is a lowering pass**, not an interface: anything that becomes output goes through a pass that records approximations (Principle 3) and is visible in the pipeline. Promised interfaces: the registry check at start-up that every component with `Entity` fields registered `refs` can't be automatic in Rust; a unit test per crate (round trip through the text form remaps refs) covers it. |

## 8. Text form, round trip, FileCheck-style tests

| | |
|---|---|
| MLIR | Every op prints in a **generic form** (`"name"(operands) <{properties}> {attrs} : type`), dialects may add a custom form. Large constants live in a trailer (`dense_resource<blob1>` plus `{-# dialect_resources: { builtin: { blob1: "0x..." } } #-}`). `mlir-opt --verify-roundtrip` prints generic, parses into a fresh context, prints again and compares, for text and bytecode (MlirOptMain.cpp 420-480). Tests are `lit` files: `RUN: mlir-opt %s -pass | FileCheck %s` with minimal `CHECK` lines (the guide warns full diffs become brittle), `-split-input-file` for many cases per file, `-verify-diagnostics` with `expected-error {{...}}` annotations. |
| primase | Nothing yet; the world prints only counts (`Debug`). |
| verdict | **Adopt.** A deterministic text form of the world, the same data as v2 (names, versions, ids, opaque values), human-readable. Uses: golden tests for passes, readers on tiny fixtures, verifier tests, `print-world-after`, reproducers, review of a run. |

Sketch (`.pwt`, "primase world text"). Entities ascending, components sorted by name, one component per line;
bodies are JSON from each component's codec (serde, which CONTRIBUTING already approves); entity references print as
`"e3"`; `#` starts a comment the parser ignores. The role comment comes from `describe`/the verifier.

```
primase-world 1
resource core.source_frame@1 {"axes":["x","y","-z"],"metres_per_unit":1.0}   # Sekiro: LH, Y up

e0 {                                                   # prototype
  core.mesh@1 {"primitives":[
    {"topology":"triangles","material":"e1","indices":[0,1,2,0,2,3],
     "streams":{"position":{"f32x3":[[-0.5,0.0,0.0],[0.5,0.0,0.0],[0.5,1.0,0.0],[-0.5,1.0,0.0]]},
                "texcoord0":{"f32x2":[[0.0,1.0],[1.0,1.0],[1.0,0.0],[0.0,0.0]]}}},
    {"topology":"triangles","material":"e1","indices":null,
     "streams":{"position":{"f32x3":{"len":3,"sha256":"9f2c41d0..."}}}}]}   # elided payload
  core.name@1 "lantern"
  core.source@1 "m11_01_00_00/o001000"
}
e1 {                                                   # material
  core.name@1 "lantern_mat"
  fromsoft.layered_material@1 {"shader":"M[ARSN]_l","layers":[{"albedo":"e2","blend_channel":0}]}
}
e2 {                                                   # texture (still a file)
  core.texture_file@1 {"path":"tex/lantern_a.dds"}
}
e3 {                                                   # placement
  core.instance@1 {"prototype":"e0","variant":0}
  core.transform@1 [[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[12.5,0.0,-3.0]]
}
e4 {                                                   # placement, mirrored
  core.instance@1 {"prototype":"e0","variant":0}
  core.transform@1 [[-1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[14.0,0.0,-3.0]]
  sunset.grind_hint@2 opaque bytes 0x0a0b0c0d          # unknown to this build, carried untouched
}
```

Rules: two print modes. **Full** prints every value and must round-trip (`parse(print(w)) == w` and
`print(parse(print(w))) == print(w)`, MLIR's `--verify-roundtrip`). **Digest** (the default for goldens) replaces
any array longer than N (e.g. 16) with `{"len":..,"sha256":..}` over its canonical JSON, so a 6.5 M-triangle world
prints in a few hundred KB and its golden still catches any changed vertex. A blob trailer like MLIR's
`dialect_resources` is a later option if full-mode files get unwieldy. Floats: shortest round-trip decimal. Two
traps to gate, not assume: serde_json writes NaN/inf as `null` (the printer must error, and the verifier forbids
non-finite core geometry), and serde_json parses f32 through f64, so the round-trip test must check f32 bit
patterns (signed zeros included) over a large sample.

Golden tests (inferred design; a small helper in core, no new dependency):
```
core/tests/golden/<case>/input.pwt      tiny hand-written world (or a reader's output on a synthetic fixture)
core/tests/golden/<case>/pipeline.txt   load-texture-files,bake-fromsoft-layered{size=4}
core/tests/golden/<case>/expected.pwt   digest-mode print after the pipeline
```
The test parses, verifies, runs, prints and compares line by line (diff on failure); `PRIMASE_BLESS=1` rewrites
`expected.pwt` and the change shows in review. Keep inputs minimal, the guide's rule, rather than a FileCheck clone:
our worlds are deterministic and small, so a full compare of a tiny world is not brittle in the way IR text is.
Diagnostics: `verify/*.pwt` files with `# expect-error: e3 core.instance.prototype -> e9: not alive` lines and
`# -----` separators (`-split-input-file`), one case each. Round trips: text -> world -> v2 -> world -> text
equal (MLIR checks text and bytecode the same way).

## 9. USD and glTF as "core plus extensions"

| idea | USD / glTF | what it suggests |
|---|---|---|
| typed vs applied | IsA (one per prim, `typeName`) vs applied API ("has a", many, listed in `apiSchemas`); API schemas exist so you don't have to subclass every typed schema | we are all "has a" already; keep it (§2) |
| units, axes | stage metadata `metersPerUnit` (fallback **0.01**, centimetres) and `upAxis` (fallback Y, configurable per site); consumers correct when assembling; "exporters strongly encouraged to always set upAxis" | keep our fixed frame (passes never think about units). Drop `Frame` from the core components table; keep the source frame as a provenance resource. The USD writer **must** author `metersPerUnit = 1` and `upAxis = "Y"` or USD readers take our metres as centimetres |
| fallbacks | USD `fallbackPrimTypes` stage metadata (older types for older readers); glTF WebP with a PNG fallback is *not* required | "lower and keep": the bake keeps `fromsoft.layered_material` next to `pbr.material`; the writer marks the source `Carry`. That is DESIGN §7's v1 glTF plan, made a legality rule |
| required vs used | `extensionsRequired` subset of `extensionsUsed`; a loader must fail on a required extension it doesn't know | our opaque carry-through treats every unknown kind as optional. In v2 mark each component table `required` when ignoring it changes the meaning of other data (a quantisation or transform-convention component); unknown + required = read error |
| extras | free-form app data, no schema, SHOULD be an object | v1 glTF writer: our dialect components as `extras.primase` (keyed by component name and version), never required, so any glTF loader opens the file. A vendor extension only if we ever need loaders to fail on it |

## 10. Proposed edits to DESIGN.md §3-4

Marked **[pass manager]** where they affect the pass manager being built now.

**E1 (§3, after "Storage" paragraph): registry and verification. [core]**
> **Registry.** Each crate registers its components in a `Registry` (`pub fn register(reg: &mut Registry)`; the
> CLI composes them): name, schema version, required/optional, verifier, entity-reference visitor, text codec and
> interface queries. A value whose namespace is registered but whose name or version is not known is an error,
> never an opaque component; opaque components are for namespaces this build doesn't know.
> **Roles are checked, not stored.** Entities stay untyped (components are "has a"); the core verifier enforces the
> role conventions (an `Instance` has a `Transform` and points at a live prototype with a mesh; a primitive's
> material is alive and has a material component), and each dialect verifies its own components.

**E2 (§3, core components table):** replace the `Frame (world resource)` row with
> | `SourceFrame` (resource, provenance) | the frame the reader converted from; the world itself is always metres, right-handed, +Y up, -Z north |

**E3 (§3, "Unknown components" sentence):** replace with
> Unknown components are carried through serialisation untouched unless their kind is marked required, in which case
> a reader that doesn't know it fails (glTF's `extensionsRequired`).

**E4 (§4, first paragraph): replace. [adopted]**
> A pass has a stable name, options with a text form, and declares the component names it **requires**,
> **provides**, **removes** and **modifies** (changes in place), plus what it approximates. The pass manager checks
> after each pass that the names that appeared are within `provides` and the names that vanished within
> `removes`, and runs the verifier (on the reader's output, after every pass, before the writer; always on, only
> the kinds the pass touched plus references between full checks). A writer declares a **target**: per component
> name `Legal`, `Dynamic` (a per-value check: texture format, TRS-decomposable transform), `Illegal` or `Carry`
> (written through untouched), and what it does with unknown names (`Carry` or error). Full conversion requires
> every name Legal or Carry; partial conversion keeps unknown names per the target.
> Pipelines are explicit lists in a text form (`load-texture-files,bake-fromsoft-layered{size=1024}`). The planner
> builds one by MLIR's legalization depth: for each illegal name, the pass chain of least depth that removes it with
> outputs legal (ties by priority, then name), plus producers of what those passes require, ordered by data
> dependencies, checked by simulating the declarations; it fails naming the missing link ("no pass lowers
> `fromsoft.layered_material` to anything `stl` accepts"). The chosen pipeline is printed and recorded in the
> report, so every run is reproducible as an explicit pipeline. No fixed-point rewriting, no backtracking.

**E5 (§4, new paragraph): reports and instrumentation. [pass manager: hooks and report shape]**
> Passes record named counters and approximation remarks (entity, `Source`, what was approximated) in the run
> report; the report is deterministic (no timings in it). Instrumentation hooks run before and after each pass and
> on failure; built in: timing (separate from the report), print-world-after(-change), and a reproducer (the
> pipeline plus the world before the failing pass). Per-entity passes go through an adaptor that may run in parallel
> and applies results in entity order; a pass reads its entity and what it references, and writes only its entity.

**E6 (§4 or §6, new): text form. [pass manager: printer as a debug hook; parser with v2]**
> The world has a deterministic text form (`.pwt`: entities ascending, one component per line, JSON bodies,
> payloads elided to length + sha256 in digest mode) that round-trips in full mode. Golden tests are
> `input.pwt` + `pipeline.txt` -> `expected.pwt`; verifier tests annotate expected errors in the input.

**E7 (§3): references.** Add to the components paragraph:
> A component with `Entity` fields registers a reference visitor; the verifier rejects references to dead
> entities, the printer and v2 writer use it, and passes that despawn use it to find dangling users.

## 11. What I did not do / open questions

- No prototype code, by design. The path-search algorithm in §6 and the text-form syntax are my designs from
  MLIR's; MLIR searches per op instance with rollback, ours plans statically on names.
- Not checked: whether serde_json's f32 parse via f64 is exact for every shortest-repr f32 (the gate in §8 decides);
  whether Blender's glTF importer keeps `extras` as custom properties (likely; check when the gltf writer lands).
- Open decisions: E1 and E7 change the core API (`Component` gets a registry entry / reference visitor;
  `insert_opaque` checks namespaces); serde derives on core components for the text form; whether `Carry` in glTF
  is `extras` (proposed) or a vendor extension.
