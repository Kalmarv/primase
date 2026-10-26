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
