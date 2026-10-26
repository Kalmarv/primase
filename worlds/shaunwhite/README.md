# Shaun White Skateboarding

The PC texture reader preserves stored BCn blocks, mip chains, colour space, map role, sampler fields and cube
faces. It uses `primase-anvil` for forge tables, compression and object envelopes. It does not infer ownership from
names or neighbouring ids: a TextureMap's compiled object is inline in its own body.

```rust,no_run
use primase_shaunwhite::read::texture::TextureMap;
# fn read(body: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let stored = TextureMap::parse(body)?;
let faces = stored.textures()?; // one core Texture for 2D; six in source order for a cube
# Ok(())
# }
```

The body starts with a managed byte and its id/class pair. Its fields are width, height, depth, pixel format,
dimension, gamma, mip count, three address modes, three filters, map type and category (all u32), three flag bytes,
user category (u32), and an inline pointer. The compiled object has an id/class pair, a u32 pixel byte length, those
pixels, then platform and SDK versions, dimensions, mip count, format, dimension, gamma and the six sampler fields.
The outer and compiled metadata must agree; the mip chains must consume the complete pixel buffer.
A stored mip count of zero denotes one level, as the reference reader and this game's buffer sizes agree.

| Stored format | Core storage | Evidence |
| --- | --- | --- |
| 2, 3 | BC1 (DXT1, DXT1A) | AnvilToolkit's AC2 schema and format table; complete mip lengths and real decodes |
| 4 | BC2 (DXT3) | same |
| 5 | BC3 (DXT5) | same |
| 12 | BC3 normal storage | this game's mip lengths and decoded content; **not** the AC2 enum, which says AutoSelect |

AnvilToolkit 1.3.7's AC2/Brotherhood readers are the third-party layout reference (the local decompilation is private;
[tool release](https://www.nexusmods.com/site/mods/455)). Shaun White adds the user category and omits the compiled
MCT flag. The three outer flag bytes are retained without assigning bool semantics: the first can hold values up
to 5. The format-12 neutral normal blocks have alpha/green near 128 and red/blue 255/0, consistent with packed
normal components. The normal role is stored, but its orientation is not established, so `Texture::normal` remains
`None`. Cube face ordinals are retained; direction and rotation are not inferred. Generic map roles remain `Other`.

Unknown codec/dimension/gamma/sampler codes, noninline compiled references, inconsistent sizes, metadata mismatches,
truncation and trailing bytes are named errors. Unsupported formats are not silently converted.

The tests contain a hand-worked red BC1 fixture, cube ordering and refusal checks. The private texture gate reads
`installs.shaunwhite/DataPC_01.forge` and `DataPC_extra.forge` through `primase-testdata`. It checks the complete
stored mip buffers and compares decoded levels against third-party bcdec_rs 0.2.0. Repeated ids must have identical
bodies. Ten raw-channel PNG previews and a manifest are written under `scratch.root/textures-sample`; BCn decoding
is confined to the tests.

## Collision

`read::collision::Shape::parse` reads complete managed shape bodies obtained through Anvil's `read_objects`.
`MeshShape::parse` reads the mesh case directly: local float4 vertices, triangle indices in source winding, and
the source surface **object id per triangle**. It also retains the material table/index bytes, filter layer/part,
local bounds and MOPP bytes, offset/scale and version. Coordinates remain in the source frame (Z up); the reader
does not place, tessellate or change the geometry.

Also read: capsule and sphere endpoints/centre/radius, transformed boxes, BarrelShape convex planes/vertices/matrix,
lists (inline children, references or null), and MultiMeshShape's contained entity handles and shape ids. A list's
final flag is retained. `InertComponent::parse` exposes the rigid-body shape id, mass, damping, flags, inertia and
walkability; `MultiInertComponent::parse` exposes its inline shape container. Inline inert objects can have managed
byte zero. The inertia tensor is not a placement transform.

An ordinary inert's collision uses its entity's transform. MultiMeshShape's parallel entity/shape arrays refer to
the contained entities' own transforms. Pairing is supported by an independently inspected ten-child sample:
each locally resolved collision shape has exactly the corresponding entity's visual mesh name. References are
kept as ids for a consumer to resolve across cells/forges in its chosen order.

The explicit mesh layout follows AnvilToolkit 1.3.7's AC2 schema, except the game's vertices are float4, not float3.
The MOPP is an acceleration structure; no Havok tagfile/packfile decoder is needed for these triangles. Code version
5 is stored; the Havok SDK version is unknown. Modern 2016/2018 tagfiles are a different container.

Source surface names are the directory names of `SWCollisionMaterial` (class `0xef0c7e58`), referenced from the
shape's material table: Concrete, ConcreteVert, ConcreteNonRidable, Asphalt, Metal, WoodDeck, WoodPipeTop, Bail, etc.
Keep these ids/names in a consumer's surface table. Surface id zero remains zero; its default meaning is unknown.
The game-specific material parameters and grind flags are not decoded. This reader supplies source collision;
destination surface assignments and skateability in a running target are separate work.

Collision tests use a hand-worked two-surface quad, capsule/list fixtures, malformed inputs, three real cells with
36 meshes and three list/hull shapes, a real inert shape reference and a fourth cell's multi-inert container.

`read::level::read_level` resolves inert shape references across the supplied forges and places collision-only
prototypes beside the visual placements. MultiInert's child shapes use each contained entity's own matrix; list
leaves retain their own local matrices. Shared cell references to the same entity/shape pair are placed once.
The namespaced `CollisionSource` holds geometry in core's frame and source material ids; `CollisionPlacedBy` keeps
the entity/shape provenance, and `CollisionSurfaces` holds source names (unresolved ids use `source:0x........`).

The registered `lower-shaunwhite-collision` pass turns these into `core.collision` or `core.collider` and indexes the
authored names in `core.surfaces`, preserving any existing table entries. Mesh triangles and winding stay exact;
capsules, spheres, boxes and convex hulls remain implicit. Programs needing triangles then select the shared
`tessellate-colliders` pass. No grind flags or destination surfaces are guessed. Level 01's test exercises both
inert forms through the reader and pass manager, alongside the material lowering.

## Levels

`read::level::read_level` reads a level forge's cells into a World: every Visual of every Entity becomes a placement of
a Mesh prototype (`shaunwhite.placed_by` names its cell and Entity), with the vertex stride and flags of the prototype
kept as `shaunwhite.mesh_format`. An Entity that marks a place is read as a `core.marker` at its matrix, named as the
Entity is: a **spawn node** (a component of the SpawnPoint class, kind `spawn_point`) and a **start position** (a name
in the `read::markers::START_NAMES` table, kind `start`: `CH_GAME_StartPosition`, `D01_C04_StartPosition`,
`Q01_Intro_FinalPlayerPos`, ...). The marker carries the whole matrix and no yaw: which local axis the game calls
forward is not read, and the program picks the spawn it wants. What no row maps is counted in the report.

## Selecting what a map draws

`select-shaunwhite-content{helpers, effects, characters, missions}` (every option `keep` or `drop`, none defaulted) drops
the placements a map should not draw, by the rows of `passes::ROWS`: each a name test on the Entity, the prototype Mesh
or one of its materials, with the level 01 evidence that gives it its class, plus the vertex flag that skins a mesh
(`MeshFormat::SKINNED`) and the cell name that tells a mission cell from a grid cell. The source states no hide flag this
reader can decode, so the rows are the game's names, not a field: the pass says so in its approximation. It runs before
`lower-shaunwhite-material` (which removes the material names the material rows match). Every drop is declared by its
row, and the prototypes, materials and textures only the dropped placements reached go with them.
Collision-only placements remain: these options select what Visuals draw, without removing authored physics.
