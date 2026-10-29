# Animation and content component contracts

These are the version-1 JSON views of the generic components added during core lowering. They are
plain data: Phase 1 adds no game pass or renderer policy. The world container remains the separate
draft in [SPEC.md](SPEC.md); this document adds no binary codec or implemented world-file reader.
`core::names` lists each component/resource by its stable name and storage kind.

Small types serialize through `core::json::to_value`, which refuses non-finite floats instead of
replacing them with null. Typed deserialization is the inverse data construction, not world validation:
call the component/resource verifier against its World before using the result. Named structures
refuse unknown fields; maps retain their opaque parameter/field keys. Entity references
are integer ids meaningful within their originating World, not process-wide asset identities.

## Draw groups and primitive keys

| Component | Fields | Invariants |
| --- | --- | --- |
| `core.draw_groups` | `scope: string`, `groups: [u32 or null]` | Nonempty scope; exactly one entry per mesh primitive; groups 0 through 63. Null is unaffected by draw-group channels. |
| `core.draw_group_defaults` | `scope: string`, `shown: u64` | Nonempty scope; bit n is the default shown state of group n for this occurrence/rig. All 64 bits are retained. |
| `core.primitive_keys` | object: exact field name to `[u32]` | Nonempty field names; each list exactly the mesh's primitive count. All u32 values are legal. |

Draw groups and primitive keys declare `Geometry::PerPrimitive`. Remapping reorders, repeats or
drops entries by the same new-to-old map as the mesh, and refuses every out-of-range source index.
They contain no vertex or local-space data. Defaults belong to the occurrence, not to a shared mesh.
Static `core.primitive_condition` stays independent: hidden defaults do not authorize removing a mesh.
Scopes are exact strings; groups of different scopes never drive each other.

## Material parameters

`core.material_parameters` is an object from exact parameter name to one through four finite f32
values. Empty maps are allowed. Parameter names are nonempty but otherwise opaque; core neither
normalizes nor interprets them. These are rest defaults on a material. A driving
`ClipTarget::MaterialParameter { materials, name }` uses `Property::Values(width)` of the matching
width; absence of a driving channel retains the default. Material entities are resolved by game
lowering, so a core channel contains no source material id, hash rule or event payload layout.

Core assigns no boolean visibility rule to a kill amount, no colour-space conversion to a tint,
and no shader-buffer address to a name. Those require evidenced game lowering/binding semantics.
In particular this shape encodes no interpretation of event 198's unknown words or ramps.
It has `Geometry::None`; primitive remapping does not alter material defaults.

## Controllers

`core.controller` contains:

- `name`: nonempty string.
- `parameters`: ordered `{name, kind, default}`. Names are nonempty and unique. `kind` is `Float`,
  `Int`, `Bool` or `Trigger`; the f64 default is finite, integral for Int, exactly 0/1 for Bool/Trigger.
- `layers`: ordered `{name, states, default_state, default_weight, blending}`. Each layer has a
  nonempty name, a finite f32 default weight and a default state: an index of its states, or `null`
  exactly when the layer has no states (an empty layer plays nothing; a source's empty layers are kept
  so the layer indices line up with the source's). Weight is retained as
  stored, without clamping. `blending` is `Override`, `Additive`, or `{"Other": i64}`.
- Each state has `{name, motion, speed, speed_parameter, cycle_offset, cycle_offset_parameter}`.
  Name is nonempty; speed (f64) and offset (f32) are finite. Negative speed and offsets outside
  [0,1] are retained. Optional numeric parameter indices name Float/Int parameters only.

A motion is externally tagged JSON: `{"Clip": entity}`, `{"Blend1D": {"parameter": index,
"children": [[threshold, motion], ...]}}`, or `{"Unsupported": reason}`. Blend1D children are
nonempty, with finite, strictly increasing f32 thresholds and a Float/Int parameter index. Every
referenced clip has `core.clip`; Unsupported has a nonempty explanation and is never an implicit
choice of one of its source children. The reference visitor visits clips through every nested tree.

Transitions and body/skeleton layer masks remain in the Unity dialect for now; the projection does
not claim to evaluate or discard them. Other source dialects follow the same retention rule.
Evaluation and layering policy belong to the animation toolkit, not to these data types.

`core.controller_binding` has `{controller: entity, rig: entity}`. The first holds `core.controller`,
the second `core.rig_instance`. Both are declared entity references. Controller and binding have
`Geometry::None`, unaffected by primitive reordering.

## Content and display names

| Component | Fields | Invariants |
| --- | --- | --- |
| `core.content_note` | `key`, `reason`, `unsupported`, `at` | Nonempty stable machine key and reason; optional finite world-space `[x,y,z]`, metres. Unsupported says something is not drawn, with the source's reason. |
| `core.record` | `category`, `kind` | Each a nonempty lower-case word of a-z, 0-9, underscore, as `Marker::kind`. A retained event need not have a transform. |
| `core.display_name` | `name`, `origin` | Nonempty human name; source identity remains separate. |

`NameOrigin` is shared with clip-event naming: JSON `"Game"` or `{"Community":{"credit": text}}`.
Community credit is nonempty. These components declare `Geometry::None`. A note's optional position
is already in world space; editing an entity's local geometry does not move it.

## Listed clips and clip events

Listed clips are plan entities: an asset member with `Role::Clip`, a `core.name` label and the
source's namespaced descriptor. An unloaded clip has no `core.clip`. `Reader::load(world, roots)`
loads the requested entities in place as source data, preserving their ids; the program then runs
its selected lowering passes. There is no catalog resource containing a decoder recipe and no
decode pass.

`core.clip_key` is the source's nonempty stable key string. Core does not split or interpret it.
Optional `core.clip_length` is a finite, nonnegative f32 duration in seconds where the listing
knows it; unknown duration means no component. Neither component requires a decoded `core.clip`:
both may already belong to the plan entity and stay with it after loading/lowering.

`core.clip_events`, on a clip, has `{origin, events, refusals}`. Origin is null or a nonempty provenance
string. Refusals are nonempty explanations for records a lowering could not interpret; no partial
behavior is inferred for them. Events retain source order, with these fields:

- `kind`: source i64 type number.
- `name`, `naming`: both absent/null or a nonempty name paired with `NameOrigin`.
- `start`: finite, nonnegative seconds; `end`: null (to clip end) or finite and at least start.
  Explicit ends may outlast the clip and are not clamped.
- `values`: ordered `{name, value}` fields. Names are nonempty and may repeat: source index is the
  identity. Values are tagged `I64`, `F64` (finite), `Bool`, `Text` or `Bytes`.
- `bytes`: all stored parameter bytes, retained even when fields were interpreted.

Source-specific event semantics belong to a lowering, not to generic event storage. Listed-clip
identities, lengths and events contain no per-primitive geometry.

## Clip channel additions

`ClipTarget::DrawGroup {scope, group}` pairs only with `Property::Visibility`: one Step value per
key, exactly -1 (inherit this occurrence's default), 0 (hidden) or 1 (shown). Scope is nonempty,
group is 0 through 63. A visibility channel never interpolates states or removes stored geometry.

`ClipTarget::MaterialParameter {materials, name}` pairs only with `Property::Values(width)`, width
1 through 4, Step or Linear. Materials are a nonempty list of distinct entities, each with the
same-width named `MaterialParameters` default. All values are finite. The channel declares every
material as an entity reference. New targets follow the clip's ordinary ordered-key, duration and
extrapolation checks. They are not joint additive deltas; additive clips cannot contain them.


## Particle systems

`core.particles` is one particle effect in game-neutral terms: `{seed, emitters, inputs, states, warmup, time_scale,
max_view_distance, termination, end, lost}`. It is placed by the `core.transform` of its entity; a system with no transform is
not placed in the world and is only started by an emitter of another system (`spawns`, entity references). Emitters are a
tree in a flat list with parents before children. Each emitter has a placement, an attachment (what frame it follows), a
delay and duration, an optional motion of its own, a spawn schedule (once, periodic, by distance moved), a shape, a heading,
a spread, a law of the particles (lifetime, movement, size scale, tint) and the form each particle is drawn as (billboard,
point sprite, mesh, trail, streak, refraction or radial-blur sheet, light, lens flare, force field, water ripples).

A number that changes is a `Value`: an argument (`Constant`, particle `Age`, emitter `ActiveTime`, `EmissionTime`), a curve of one
to four components (steps, lines or cubic Beziers over ascending finite keys) and random variations that name a stream (and
an input of the system). Random draws are keyed by `(seed, emitter, scope, stream, component)`, never by processing order, so
a system stepped to a time gives the same particles anywhere. Units are metres, seconds, degrees and linear colour. `lost` is
accounting: the reasons a source's pass could not carry something, with how many, never state. The JSON view is the
serialization of these types and is not read back; `references` lists the textures, meshes and systems the draws and emitters
name. Core defines the description; stepping and drawing it belongs to a consumer.

## Clip effects

`core.clip_effects`, on a clip, has `{origin, spawns, lost}`: the particle systems the clip starts while it plays. Origin is
null or a nonempty provenance string; `lost` counts, by nonempty reason, what the lowering could not carry. Each spawn has
`{system, start, end, socket, follow, event}`: `system` is an entity that holds a `core.particles` and no `core.transform` (the
spawn places it); `start` is a finite, nonnegative clip time in seconds and `end` null (the system plays out) or a time at or
after it (its emitters are stopped); `socket` is null (the rig's origin) or the nonempty name of a socket of the clip's rig;
`follow` says whether the system moves with that socket as the clip moves it, else it stays where it started; `event` is the
index of the starting event in the clip's `core.clip_events`, where the source says. A player begins each spawn at its start
on the clip's clock.

## Lights as a program reads them

| Name | Kind | Fields | Invariants |
| --- | --- | --- | --- |
| `core.light_unit` | resource | `candela_per_unit: f32` | Finite and above 0. Written by the lowering that makes the world's point and spot `core.light`s from a source's own light values: such a light's intensity is its source value times this factor. |

A shader binding's buffer slot may hold `{"lights": table}` (`BufferSource::Lights`): the scene's
lights of one `kind` (`point` or `spot`), one record of `stride` bytes per `core.light` of that kind,
at most `capacity` records, each holding `fields` (`{offset, value}`) as 32-bit floats. A value is
`view_position` (3), `range` (1), `inverse_range_squared` (1), `colour` (3: colour times intensity over
`core.light_unit`'s factor), `view_to_cone` (16, spots only), `view_bounding_sphere` (4),
`view_depth_min` / `view_depth_max` (1), `{"constant": {value, why}}` (1) or
`{"unknown": {floats, name, why}}`. Fields lie inside the record on whole floats and never overlap; a
structured buffer's element size equals the stride; constants are finite; constants and unknowns say
why. A binding with a light table in a world that holds `core.light`s of the table's kind but no
`core.light_unit` is refused. `LightTable::pack` is the one packing of the world's lights into the records for a view; it
writes 0 for unknown fields and lists them. The constant role `{"frame": {"light_count": kind}}` is the
number of records the consumer packed, one unsigned 32-bit integer.

A buffer slot may hold `{"light_lists": part}` (`BufferSource::LightLists`): one of the frame's per-tile
light lists, which say which lights a program visits at a pixel. The screen is cut into squares of
`{"frame": "light_tile_size"}` pixels, tile `(x, y)` being record `x + y * light_tiles_per_row` (both
unsigned 32-bit constants) of the `{"lookup": {stride, capacity, first, counts, constants, unknown}}`,
whose record holds the tile's first entry in the `{"indices": {stride, capacity, index, slices, constants,
unknown}}` list and, per kind in `counts` order, how many of its lights follow. An entry holds a light's
record in its kind's light table and one bit per depth slice its volume reaches. Each value is
`{offset, shift, bits}` of a little-endian 32-bit word; fields lie inside the record and never overlap;
constants fit their bits and say why; unknowns are named and say why. A stage's lists are one lookup and
one index list, and the stage binds a light table of each kind the lookup counts. `pack_light_lists` is
the one packing: every light in every tile and every slice (each light's own range still bounds what it
lights).
