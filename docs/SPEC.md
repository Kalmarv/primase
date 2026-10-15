# primase world format, version 2 (draft)

Status: **draft for review**, not implemented. Version 1 is the skate-scene interchange format that
`formats/skate-scene` reads and writes unchanged; version 2 is core's own serialisation of a [`World`]: "core plus
named component tables" (DESIGN.md §3 "On disk"). Open questions are collected in §10.

## 1. Goals

- **Exact.** Writing a world and reading it back gives the same world: the same entity ids, every typed component
  with its schema version, opaque components and resources untouched, f32 bit patterns (signed zeros included).
- **Deterministic.** The same world gives the same bytes (§8).
- **Open.** A reader that does not know a component kind carries it through untouched, unless the file marks the kind
  required (glTF's `extensionsRequired`). A kind whose namespace the reader registers but whose name or version it
  does not know is an error, never an opaque value (DESIGN.md §3 "Registry").
- **Columnar.** One table per component kind, so a pass or a tool that needs one kind reads one file, large kinds
  (meshes, soups) are binary, and small dialect kinds can stay JSON until someone needs them binary.
- **Strict.** Like every primase reader: the exact size, every index, every float checked; nothing after the end.

## 2. Folder

```
world.json                       the manifest (§3)
components/<name>.pwc            one table per component kind, e.g. components/core.transform.pwc (§4)
resources/<name>.pwc             one table per resource, record count 1, no entity column
files/<sha256>.<ext>             files the world refers to by path (core.texture_file, core.mesh_file), by content
```

Names are component names as they are (`[a-z0-9_]+(\.[a-z0-9_]+)+`), so the folder lists the world's kinds. Paths in
the manifest are relative, forward slashes, inside the folder (the same rules as v1).

A file-reference component (`core.texture_file`, `core.mesh_file`) is written with its path rewritten to the
copied file in `files/`, named by its content hash (identical files stored once, names a function of content).
Reading resolves them to absolute paths, as the v1 reader does.

## 3. Manifest

```json
{
  "format": "primase-world", "version": 2,
  "generator": "primase-core 0.1.0",
  "entities": {"table": "entities.pwc", "count": 5210, "next": 5210},
  "required": ["fromsoft.layered_material"],
  "components": [
    {"name": "core.instance", "version": 1, "encoding": "binary", "count": 3636,
     "file": "components/core.instance.pwc", "sha256": "…"},
    {"name": "skate.material", "version": 1, "encoding": "json", "count": 540,
     "file": "components/skate.material.pwc", "sha256": "…"},
    {"name": "sunset.grind_hint", "version": 2, "encoding": "bytes", "count": 12,
     "file": "components/sunset.grind_hint.pwc", "sha256": "…"}
  ],
  "resources": [
    {"name": "skate.scene", "version": 1, "encoding": "json", "file": "resources/skate.scene.pwc", "sha256": "…"}
  ]
}
```

| key | meaning |
|---|---|
| `format`, `version` | required: `"primase-world"`, `2` |
| `generator` | the writing crate and version (for reports; not part of equality) |
| `entities` | the alive entity ids (`table`: an entity-list table, §4), how many, and `next`: the id the next `spawn` takes (ids are never reused, so `next` can exceed the largest alive id) |
| `required` | component kinds a reader must understand to read this world (an unknown one is an error) |
| `components`, `resources` | one entry per kind present, sorted by `name`: schema `version`, `encoding` (`binary`, `json`, `bytes`), record `count`, the table `file` and its `sha256` |

A kind with no entities is not written. The manifest has no other content: everything about the world is in tables.

## 4. Tables

Little-endian. Every table starts with this header; sections follow in order, each padded with zero bytes to a
multiple of 8; nothing after the last section.

| bytes | type | |
|---|---|---|
| 0-3 | magic | `PWC2` |
| 4-7 | u32 | table layout version, 1 |
| 8-11 | u32 | kind: 0 component, 1 resource, 2 entity list |
| 12-15 | u32 | encoding: 0 binary, 1 JSON, 2 bytes |
| 16-19 | u32 | schema version of the component |
| 20-23 | u32 | record count `n` (resource: 1) |
| 24-27 | u32 | fixed record size `r` in bytes (binary encoding; else 0) |
| 28-31 | u32 | name length `L`, then the name's UTF-8 bytes (padded to 8) |

Then:

1. **Entity column** (components only): `n` x u32, the entities that have the component, strictly ascending.
2. **Records**, by encoding:
   - **binary**: `n` fixed records of `r` bytes, laid out by the component's codec (§5); variable-length parts are
     array references into the heap.
   - **JSON** and **bytes**: `n` x (u64 offset, u64 length) into the heap; record `i` is that slice: canonical JSON
     (§8) or the opaque bytes as they were.
3. **Heap**: u64 length, then the bytes. Binary records refer into it with **array references** `(u64 offset, u64
   count)`; the element type comes from the codec, `offset` is aligned to the element's size. The writer lays the
   heap out in record order, fields in codec order, no sharing, so the heap is a function of the records.

The reader checks the header against the manifest entry (name, version, encoding, count), the exact file size, the
entity column (ascending, alive, inside `entities`), every array reference inside the heap and aligned, every float
finite, every entity reference alive (§6), and the sha256.

### Entity list

Kind 2, encoding binary, `r` = 0, no records: the entity column alone lists the alive ids (`n` = `entities.count`).

## 5. Codecs

A component kind is serialised by the codec its crate registers with its name and version (DESIGN.md §3 "Registry").
A JSON codec is the component's `to_json` view (which never loses a value) plus its parser; kinds whose view is a
summary rather than the data (`core.mesh`, `core.texture`, `skate.soup`: counts, not vertices or pixels) are
binary only. A writer uses binary when a codec exists. Parsers are not written yet: today components serialise to
JSON but do not read back from it (they come with the registry, DESIGN.md §3). Binary layouts of the core components (all fields little-endian, records padded to 4):

| component | record | heap arrays |
|---|---|---|
| `core.transform` | 12 f32: the 3x4 column by column (48 B), as v1's `PLAC` | |
| `core.instance` | u32 prototype entity, u32 variant | |
| `core.tile` | i32 col, i32 row | |
| `core.bounds` | 3 f32 min, 3 f32 max | |
| `core.name`, `core.source` | string ref | u8: UTF-8 |
| `core.texture_file` | string ref (path) | u8 |
| `core.mesh_file` | string ref (path), u32 mesh, array ref (materials) | u8; u32 entity per primitive (`0xFFFFFFFF` none) |
| `core.material_variants` | array ref (primitives) | per primitive an array ref of (u32 variant, u32 material) pairs, sorted by variant |
| `core.mesh` | array ref (primitives) | primitive records: u32 topology, u32 material, u32 flags (bit 0: indexed), array ref indices (u32; empty when not indexed), array ref streams; stream records: u32 semantic code, u32 element type, array ref data |
| `core.texture` | u32 width, height, `TexFormat` code, colour space, role, normal (green, z, or none), array ref levels | per level an array ref of u8 |
| `skate.soup` | array refs: positions (3 f32), triangles (3 u32), surfaces (u8) | as v1's `SOUP` |

A string ref is an array ref of u8; enums are written as their codes, listed with each enum (`Semantic`, `Stream`
element types, `Topology`, `ColorSpace`, `TexRole`, `NormalGreen`, `NormalZ`, `TexFormat`); adding a code bumps the
component's version. Everything else (the `skate.*`, `pbr.*` and other dialect components) starts as JSON; a crate
adds a binary codec, with a version bump, when its tables get large.

## 6. Entity ids and references

Ids are u32, written as the world has them (gaps allowed); the reader spawns them ascending with
`World::spawn_at` and restores `next`, so ids, references and later spawns are as before. An entity reference
inside a component (binary: u32, none `0xFFFFFFFF`; JSON: the integer id, none `null`) must name an alive entity;
codecs report their references through the component's reference visitor, so the reader checks every one and a
despawning pass can find them.

## 7. Opaque components and versions

A component the reader does not know, in a namespace it does not register, is kept as `Opaque { version, data }`
(JSON or bytes, as encoded) and written back byte for byte, with its version. A required kind it does not know, or a
registered namespace with an unknown name or a version newer than its codec, is an error naming the kind. An older
version is read through the codec's migration when it has one, else it is an error.

## 8. Determinism

- Tables sorted by name in the manifest; records in entity order; the heap in record order.
- Canonical JSON: no whitespace, object keys in the codec's field order, integers as integers, floats as the
  shortest text that reads back as the same value of the field's type (f32 fields as f32: readers parse those fields
  straight from the text, not through f64), negative zero as `-0.0`, never NaN or infinity (the JSON view refuses
  them), strings with only `"`, `\` and control characters escaped.
- No timestamps, no host paths: file names in `files/` are content hashes; `generator` is the only free text, and
  world equality ignores it.
- Parallel writers produce the same bytes as serial ones.

Consequence: a world's v2 folder is a content address of the world; two runs of a pipeline can be compared by the
manifest's table hashes.

## 9. How v1 maps onto v2

The skate-scene reader (`formats/skate-scene`) is the bridge; its world goes to v2 like any other:

| v1 | v2 |
|---|---|
| `scene.json` top-level fields, key order, string lists, table references | resource `skate.scene` |
| `textures.<key>` + the file | `skate.texture`, `core.texture_file` (file in `files/`), `core.name`, `core.source` |
| `materials.<key>` | `skate.material`, `core.name`, `core.source` |
| `prototypes[i]` + its GLB | `skate.prototype` (parts' provenance), `core.mesh_file` (file in `files/`, per-part materials), `core.material_variants`, `core.name`, `core.source`; its collision `skate.soup` |
| `PLAC` record | `core.transform`, `core.instance`, `core.tile` (flag 1), `skate.placement`, `core.source` |
| `collision[i]` + `SOUP` | `skate.collision`, `skate.soup` |
| `LGHT` record | `skate.light`, `core.source` |
| `CURV` curve | `skate.grind_curve` |
| `water[i]`, `markers[i]` | `skate.water`; `skate.marker`, `core.name` |

v1 -> v2 -> v1 is exact for everything the v1 round-trip gate checks (tables byte-identical, manifest equal as
JSON, files byte-identical), since every v1 detail the writer needs lives in those components. The gate for v2:
read every v1 scene folder, write v2, read v2, write v1, and compare with v1's gate; plus world equality
(`print(read(write(w))) == print(w)` in the text form, full mode).

## 10. Open questions

1. **Files by content hash or by original path.** Hash names dedupe and keep the folder a function of content, but
   lose readable names (they stay in `core.name` / `core.source`). Alternative: `files/<original relative path>`
   with collisions refused.
2. **Large tables.** v1 allows a table in chunks; v2 could allow `file` to be a list of chunk files by entity range,
   for multi-GB meshes and streaming. Not in 2.0 unless needed.
3. **Compression.** None in 2.0 (no approved general-purpose compressor besides deflate). Binary heaps of meshes
   compress well; `flate2` per table is an option later.
4. **Which dialect kinds go binary first.** Proposed: `skate.soup` (millions of triangles) now; `skate.light`,
   `skate.grind_curve` when a world has many.
5. **Mesh payload sharing.** Prototypes that share a GLB mesh share nothing in the heap (no dedup, for determinism and
   simplicity). A content-hash dedup of heap arrays is possible later without changing readers if array references
   may point at earlier data.

[`World`]: ../core/src/world.rs
