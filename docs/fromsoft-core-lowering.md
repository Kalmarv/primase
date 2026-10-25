# FromSoft read/load and lowering boundaries

`CharacterReader::with_animations` lists all authored clip members. `read()` publishes
one plan entity per member: `core.clip_key`, `core.name`, optional `core.clip_length`
and `fromsoft.clip_source`. The descriptor states character, binder, exact member and
its stored table index. Repeated stems, including repeated member paths, keep distinct
opaque keys. Selection by an ambiguous stem refuses and lists the keys.
Each plan has `core.provenance`: the binder path in `Origin.archive` and the HKX
member in `Origin.file`, unchanged by load. Viewers can group by binder before decoding
without interpreting a key.

`load(world, roots)` loads the clip entities the roots reach onto those same entities.
The reader privately retains its archive and binders. It parses only the requested HKX
and its skeleton/compendium/TAE dependencies, returning `havok.animation` and
`fromsoft.*` facts. It creates no `core.clip`, chooses no skeleton correspondence and
runs no pass. Newly loaded dependencies are reached through the selected clip roots.
Unsupported encodings refuse by clip name. The input plan World stays unchanged.

The program then selects and runs `lower-havok-clips`, `correspond-skeletons`,
`map-clip`, `lower-fromsoft-clip-events`, `lower-fromsoft-display-masks` and
`lower-fromsoft-material-events`, with their explicit options. The last three are pure
World transformations. There is no decode pass or decoder text in World data.

## The model-space choice

The deferred-clip consumer's old rule switched Sekiro from
`parents=same, rests=target, reparented=refuse` to
`parents=any, rests=source, reparented=model-space` when `hierarchy_counts.paired == 0`.
That condition is an observation about correspondence, not an authored HKX or FLVER
selector. `primase_animation::correspond` documents why differing skeleton hierarchies
can need model-space mapping; it does not prove that zero accepted pairs uniquely
select that policy. No source flag selecting this fallback was identified.

Therefore this is a program-owned heuristic. The reader does not make the switch.
A consumer retaining the heuristic must state the chosen correspondence and mapping
options in its pipeline and retain the pass reports. The exact shared-name/rest/parent
counts are available through the toolkit's `hierarchy_counts`; they do not choose a policy.

## Events and text

The TAE reader keeps the clip's own mini-header, import reference and authored events
beside the resolved event sequence. Its typed descriptors parallel the resolved events
in exact source order. Smithbox's credited MIT templates decode the parameter fields;
duplicate field names remain duplicates. Unknown/truncated fields retain raw bytes and
a named refusal. MaterialEx event 198 adds independently established structural fields
in the reader, including the row id only for its observed layout. No transition envelope
is inferred. The lowering pass copies typed facts and raw bytes into `core.clip_events`.

NPC text reading requires the caller's language and name dictionary. Available language
folders are the intersection of dictionary paths with entries present in the archive;
an empty or unknown selection refuses with those options. Missing optional DLC binders
and repeated FMG ids (including whether their text differs) are reported with paths.
`fromsoft.npc_names` retains the requested language and that report with the joined names.
The pure naming pass prefers authored NPC-row/FMG text, then credited community aliases,
and never changes `core.name` identity.

## Laws audit of this change

The quotations below are from CONTRIBUTING.md; it takes precedence over design notes.

| Piece | Law and result |
|---|---|
| Clip discovery and load | Reader may “load the heavy data ... of the parts it is asked for”; implemented under `read/`, descriptors under `components/`. |
| Clip decoder pass and catalog recipe | Pass may not “touch files, the environment or processes”; removed the decoder pass and its registration, and removed `ClipCatalog`. |
| Template and MaterialEx byte parsing | “A game's meaning ... goes to its reader”; template decoding is under `read/`, and pure passes consume typed fields. |
| Mapping fallback | “A default that is a choice”; no automatic fallback in the library. Program explicitly chooses mapping options. |
| NPC language | “A default that is a choice”; removed the fixed language, requiring the caller's selection. |
| Duplicate clips, missing DLC and repeated text ids | “A lost report”; distinct clip identities survive, and text omissions/conflicts carry named reports. |
| Map `core.record` additions | Passes “transform the world ... declare what [they] ... provide”; the existing map-record pass declares `core.record` and retains the typed source records on emitted entities. |
| Human names | A name list is allowed as a “named, documented table from source names to shared vocabulary”; bundled aliases are credited, are display metadata only, and do not replace source identity. |
| `NpcNames` | Reader brings in source text, component/resource owns data, pure pass names entities; no file access or language choice in the naming pass. |
| `densest_square` | Toolkit holds “shared mechanisms that games supply data to”; caller supplies points, side and cell size. Occupied-corner search and deterministic tie rule are documented. No automatic region or size choice moved into the library. |
| MSB drawn-part points | “A game's meaning ... goes to its reader”; query identifies authored map-piece/object/asset positions without mutating or selecting the World. It applies no map-offset guess. |

Remaining existing boundaries to review, rather than silently broadening this change:

- The map-record lowering leaves spawn regions to `lower-fromsoft-extras` and reports
  that count even when the consumer omitted that pass. Under “Reports return what the
  program needs to see”, a consumer must surface this omission; the new `core.record`
  fields do not independently recover it.
- The direct eager character/parts APIs still read tracks when explicitly requested.
  The new Reader adapter uses plan/load; legacy listing helpers and their source table
  remain temporarily until consumers migrate, with the replacement named in their docs.
- Geometry/material payloads in the pre-existing CharacterReader read remain eager;
  the clip redesign changes clip loading only, not the existing model-reader staging.

Verification pending: source review only in this round. Unit fixtures cover plan keys,
optional lengths, duplicate-stem refusal, unsupported-HKX refusal, typed event mapping,
language validation and duplicate message reports. An ignored installed-data test
checks original entity ids, load-one dependency closure and absence of core lowering.
