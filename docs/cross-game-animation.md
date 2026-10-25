# The Sekiro / skate pair

This is a primase boundary, independent of a window or a scene's mesh/material payloads. The program owns source
identity, options, cancellation and a cache; readers state stored facts, and passes state their approximation policies.

## Light sources

`primase_reskate::read::animation` exposes:

- `read_skeletons(&Source, &mut World) -> Result<Skeletons, String>`: the authored animation rig and render skeleton;
  `Skeletons { animation, render }` contains their entity IDs. Neither costume geometry nor clips are read.
- `list_clips(&Source, toc) -> Result<Vec<ClipEntry>, String>`: sequential EBX inspection, without ECLP resources,
  channel maps, meshes or texture resources. The caller chooses TOCs from the data folder's listing. This reads EBX
  metadata documents, not just bundle filenames; no assertion about listing cost has been measured yet.
- `decode_clip(&Source, &ClipEntry, animation, &mut World) -> Result<Entity, String>`: one selected controller's
  ECLP and channel map, as `skate.clip`. It resolves the exact bundle recorded by the descriptor.

`ClipEntry { key, toc, bundle, name, duration, looping, refusal }` is strict serializable data. Its key is opaque:
canonical JSON `[toc,bundle,name]`, ASCII-lowercased. Equal names in different bundles remain distinct. Timing is
`EndFrame / FPS` seconds from EBX. Controllers whose timing/codec is unsupported remain listed with a refusal;
structural errors reading a document fail with its actual TOC/bundle/name rather than reporting an empty catalog.

Lower a selected skate clip with `lower-skate-clips{interpolation=linear}`, whose interpolation approximation is
reported. Build the animation-to-render map with `game::rig::render_map`, then run
`map-clip{unmapped=drop,rests=source,reparented=refuse}`. The existing render-map fixture compares model-space poses;
this correspondence is separate from the cross-game table.

FromSoft's light clip boundary is `sekiro::CharacterReader::animations`: `Reader::read` publishes
`core.asset` clip members with `ClipKey`, `Name`, optional `ClipLength` and binder provenance;
`Reader::load` decodes the reached plan entities on those same ids. The separate
`read_clip_skeleton` is for probes that choose no clip. That skeleton read parses
only the main binder's skeleton/compendium, not events, models, materials or animation tracks. Name listings have
unknown duration because BND member headers do not store it. A selected clip supplies its duration after decode.
The caller lowers that one raw Havok animation with explicit `lower-havok-clips` choices. A second decode reuses
the same named skeleton only when its actual stored definition agrees.

## Pair tables and one-clip transfer

The established human FLVER-to-render correspondence is kept as reviewable bone names in
`checks/tests/fixtures/sekiro-human__skate-render.json`. It names two games, so it lives with the checks (core, toolkit
and formats name no game) and only `checks/tests/skate_pair.rs` reads it; `toolkit/animation/tests/pair.rs` uses small
tables of invented bone names. This copies the previously authored pair table; it adds no game transforms or mesh payloads
and does not declare the table valid on all humans.
`reversed(&JointTable)` reverses clip correspondence and notes. Tables with skin placements refuse reversal,
because those offsets belong to the original target frame.

`inspect(from, to, table)` reports each stated pair's availability, uniqueness, immediate-parent agreement,
largest local-rest matrix entry difference, and target/source length ratio measured from the nearest mapped
ancestor. Unmapped joints on both sides remain named. A missing name never becomes a guessed match.
`available_table` is an explicit projection onto uniquely present pairs and returns the complete observation
report alongside it. The program must show what it projected out; this is not automatic confirmation of a mapping.

`retarget_one(&Clip, &Skeleton, &Skeleton, target: Entity, &JointTable, RetargetClip)` returns a `Transfer` (the `Clip`, and the pass's approximation and omission reasons by count).
It runs the existing rotation-transfer pass in a small isolated World holding two skeleton copies and one clip.
The input clip/World are untouched. Returned joint channels name the caller's target entity. A refused or unchanged
result is an error with the pass's omission reasons. The table is strict; projection, when wanted, is a separate
call with its own report. Morph channels refuse without a stated mesh correspondence. Control channels and separate
`RootMotion` are retained by the existing pass. Root joint translation uses the stated translation policy and its
reported reference-height ratio; separate rig root-motion curves are carried unchanged, not silently rescaled.
Both skeletons must already share metres, up and facing. There is no automatic axis/facing guess and no contact IK.

## Verification written for the coordinator

No builds or runs were performed during the write-only round. These are test definitions, not measured results:

- `primase-animation/tests/pair.rs`: isolated bidirectional transfer, root translation scaling, separate root-motion
  retention, source immutability, and named missing-pair reporting. All default fixtures are self-contained: a
  hand-worked short/tall pair also measures a 2:1 segment ratio, zero joint-fit residual, three weighted vertex
  displacements and the rebound skin's actual positions. `PAIR_FIXTURE_CENSUS` states this synthetic scope.
- Ignored `primase-reskate/tests/animation_source.rs`: existing off-board idle fixture, all listed / one decoded, duration,
  animation-to-render mapping, zero mesh/texture components.
  The same file's default synthetic rig/DOF/track fixture checks stored frame times, linear lowering, render-joint
  identity and unchanged root translations without opening an archive or native decoder.
- `primase-fromsoft/tests/deferred_clips.rs`: existing c1010 skeleton-only read then decode-one, skeleton reused.
- Ignored `primase-checks/tests/skate_pair.rs` (a test across two games lives in `checks/`) real-data proof: c1010 / the existing skate idle in both directions, then the character skin
  fitted and rebound to render. Choices are `reference=aligned,translation=root`; skin uses
  `fit=positions,unmapped=nearest,placement=automatic`, with all pass approximations reported.
- The ignored full `every_sekiro_human_candidate_reports_pairs_ratios_and_skin_fit` census considers every
  dictionary-named held character binder, reports every FLVER skeleton and errors, and fits candidates with Root
  plus both upper-arm and thigh names. This is a stated skeleton criterion, not a semantic classification of every
  character. `SEKIRO_SKATE_CENSUS` includes all pairs, omissions, ratios and fit refusal/error reasons. Fits measure
  maximum joint residual, vertex displacement and seam separation in metres. Authored initializer part outfits are
  logged as `SEKIRO_SKATE_OUTFIT`; each distinct readable outfit plus the established Wolf-9000 fixture is assembled
  and fitted once as `SEKIRO_SKATE_PARTS`. Unnamed hashed files and unsupported outfit sources remain unmeasured.
  `SEKIRO_SKATE_CENSUS_BEGIN` and `SEKIRO_SKATE_CENSUS_SUMMARY` state the actual denominators and outcomes; the
  explicitly requested installed proofs fail on missing configuration instead of passing without measurements.

The full census and installed one-pair proof are opt-in because they read game data and need native decoders;
the default pair/skin/census fixtures use synthetic data and no install, network or DLL. Numeric fit errors are not visual-quality claims. A
runtime/viewport proof, after compilation and the real-data tests, is still required for the lab's first visible pair.
