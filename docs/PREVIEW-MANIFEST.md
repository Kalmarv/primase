# The preview manifest (version 1)

The preview export writes ordinary GLB files and, beside them, `manifest.json`. The GLBs hold every alternative a
source offers (every outfit, a hat as one primitive of the head, each material variant); the manifest says what each
thing is, what it depends on, and where it is in the files, so a website can show a character, let a person pick
outfits and accessories, and hand back a recipe (the chosen ids) that a program replays as passes. No glTF extension is
involved: a viewer that ignores the manifest sees a character wearing everything.

The writer is `primase_gltf::PreviewParts` (a conversion writer, one GLB per part as `GlbParts` writes them). The types
are `primase_gltf::preview::manifest`, which both write and read the file (`Manifest::to_json`, `Manifest::from_slice`),
and `primase_gltf::Recipe` for the answer. This page is the schema; the types are its one implementation.

## What a world has to look like

The manifest is made from the world the preview is written from, so the choices are state the world already states:

- A **choice** is a `core.state_axis` (a variable with a key and up to 64 named values).
- A **condition** says when something exists: `core.condition` on a placement, on a prototype (an entity with a mesh) or
  on a part or alternative of an asset (`core.asset`); `core.primitive_condition` on a mesh says it per primitive.
  A condition is an OR of clauses, each an AND of literals "axis is one of these values". Conditions control presence;
  they do not check that a combination of choices is legal.
- A **material variant** is `core.material_variants` on a prototype (per primitive, variant to material), selected by
  a placement's `core.instance` variant, which a `core.state_override` on the placement switches by axis.
- A **binding** is a placement's `core.skin_instance` (the rig and the binding of its mesh's skin) or its
  `core.attachment` (a rig socket).
- An **asset** is a `core.asset` root: its members, with their roles, are what the choices decide.

The preview writer resolves nothing and invents no default: an axis with no default stays without one in the manifest.
Parts that an outfit chooses between are members of role `Part` (the glTF writer draws placements, and `place-assets`
places `Part` members; it leaves `Alternative` members unplaced because choosing one is a selection).

## The file

```json
{
  "format": "primase.preview-manifest",
  "version": 1,
  "generator": "primase-gltf 0.1.0",
  "files": [{"name": "chr_hero.glb", "bytes": 12345}],
  "choices": [
    {"id": "outfit", "label": "outfit label", "class": "outfit", "source": "wardrobe outfit",
     "options": ["coat", "jacket"]},
    {"id": "hat", "label": "hat label", "class": "accessory", "source": "wardrobe hat",
     "options": ["off", "on"], "default": "off"}
  ],
  "assets": [{
    "id": "chr/hero", "label": "Hero", "file": "chr_hero.glb",
    "members": [
      {"id": "coat", "label": "coat", "role": "part",
       "requires": [[{"choice": "outfit", "options": ["coat"]}]],
       "draws": [{"id": "coat@0", "label": "coat", "node": 4, "mesh": 1,
                  "skin": {"rig": "skeleton@0", "binding": 0}}]},
      {"id": "head", "label": "head", "role": "part",
       "draws": [{"id": "head@0", "label": "head", "node": 6, "mesh": 3,
                  "primitives": [{"primitive": 1, "requires": [[{"choice": "hat", "options": ["on"]}]]}],
                  "variants": [{"variant": 0, "mesh": 3}, {"variant": 1, "mesh": 5}],
                  "overrides": [{"when": [{"choice": "skin", "options": ["dark"]}], "set": {"variant": 1}}]}]}
    ],
    "rigs": [{"id": "skeleton@0", "skeleton": "skeleton", "node": 0,
              "joints": [{"id": 0, "name": "root", "node": 1}],
              "sockets": [{"id": 0, "name": "grip", "joint": 1, "offset": [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0.1,0,1]}]}]
  }],
  "fidelity": [{"stage": "fit-outfit", "kind": "approximation", "reason": "...", "count": 3, "examples": ["coat"]}]
}
```

### Fields

- `format`, `version`: fixed `primase.preview-manifest` and `1`. A reader refuses another format or version and any
  field it does not know (`deny_unknown_fields`); a change of meaning or fields is a version bump.
- `files`: every GLB written, in order, with its size. A GLB holds one part of the conversion; a part named for its one
  asset is `<core.source, unsafe characters as _>.glb` (`chr/hero` is `chr_hero.glb`).
- `choices[]`: `id` is the axis key (stable, unique); `label` the axis entity's `core.name`, else the id; `class` and
  `source` as the axis states them; `options` the value names in domain order, each unique within the choice (the
  names serve as option labels; a bool is `off`, `on`); `default` the value the source states, absent where it states
  none. An axis scoped to an entity has no stable id for its owner, so writing one is refused, naming it.
- `assets[]`: `id` is the asset root's `core.source`, else its `core.name` (an asset with neither is refused); `label` its
  name else the id; `file` the GLB. Asset ids are unique across the manifest.
- `members[]`: `id` is the member's `core.name` when it has one that no other member of the asset has, else `member-<n>`,
  `n` its place in the asset's member list; ids are unique in the asset. `role` is `part`, `alternative`, `rig` or
  `clip`. `requires` is the member's condition (absent: always). Resolving a choice that fails it removes the member.
- `draws[]` (the nodes that draw a member's mesh): `id` is `<member id>@<n>`, `n` counting the member's draws in the
  order written, unique in the manifest; `node` and `mesh` are indices in the file's `nodes` and `meshes`. `requires`
  is the placement's own condition, besides the member's. `primitives` lists primitives (index in the mesh's
  `primitives`) drawn only when their condition holds; the others are always drawn. `variants` gives, for each material
  variant the node can take (its own, those its mesh's materials name and those its overrides switch to), the glTF mesh
  that draws it: same accessors as `mesh`, other materials. `overrides` are the node's changes in states, in order; the
  first whose `when` (an AND of literals) holds applies: `{"variant": n}` (draw that variant's mesh), `{"transform":
  [16 numbers]}` (a column-major matrix, glTF's frame) or `{"prototype": "<member id>"}`. `skin` names the rig and the
  skin binding the node is drawn with; `attachment` names the rig socket it follows and its offset in the socket's
  space.
- `rigs[]`: `id` is `<skeleton member id>@<n>`; `node` the rig's root node, `joints[]` each joint's id, name and node,
  `sockets[]` each socket's id, name, joint id and offset (a column-major matrix in the joint's space).
- `fidelity[]`: what the preview approximates, leaves out or chose, as the conversion's notices say it (`stage`, `kind`
  one of `unsupported`, `approximation`, `selection`, `failure`, `diagnostic`, `reason`, `count`, a few `examples`).
  The writer's own notices are in the file it writes; a program adds what its passes reported once the conversion's
  report is in (`Manifest::add_notices`, then `Manifest::write_file`), and the same notice twice is one entry.

A **condition** is a JSON array of clauses (an OR; an empty array is never) of arrays of literals (an AND), a literal
`{"choice": id, "options": [option ids]}` meaning the choice has one of those options.

### What `Manifest::check` guarantees a reader

Ids are unique and non-empty where stated; every file an asset names is listed; every literal names a choice and
options of it; a default is one of its options; an override's variant has a mesh and its prototype is a member; a skin or
attachment names a rig of its asset and an attachment a socket of it; a rig's skeleton is a member and its sockets sit
on its joints. It does not read the GLBs: the tests check the node, mesh and joint indices against the files' own JSON.

## How a viewer uses it

1. Draw `meshes[draw.mesh]` at `nodes[draw.node]` for every draw whose member and node conditions can hold.
2. A choice made by the person fixes literals: hide the members and draws whose `requires` fails, hide the primitives
   whose condition fails, and for each node take the first override whose `when` holds and draw its variant's mesh.
3. Show choices from `choices` (label, options, default). A choice with no default starts unchosen: nothing is invented.
4. Hand back a recipe.

## The recipe

```json
{"format": "primase.preview-recipe", "manifest_version": 1, "choices": {"outfit": "jacket", "hat": "on"}}
```

A recipe holds only ids from the manifest. `Recipe::check` refuses a choice or option the manifest lacks;
`Recipe::pipeline(&manifest)` is the passes that apply it: one `select-outfit{choices=hat:on;outfit:jacket}`, the same
pass a program runs by hand, in the world read and prepared as the manifest's was. `select-outfit` resolves the named
axes only; the choices the recipe does not make (`Recipe::undecided`) stay open until the program resolves them
(`resolve-state` with its own named values), so preview and export are one recipe and nothing is chosen for the person.
Applying it drops the members and placements whose conditions fail and the primitives whose conditions fail (every
per-primitive component following), and reports each by axis and value.
