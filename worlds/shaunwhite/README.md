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

## Water

The Material's **stored bool at body offset 91** selects the water shader. Independently checking 1,971 fixed-size
materials found six set: WaterPort, WaterPool, WaterToxic01, Water_Ocean, Water_PirateShip and ReservoireWater01.
All six have normal, specular and reflection-cube slots, with no diffuse. Foam decals, fountains, tanks and fabric
whose names contain water leave it clear. The lowering uses the field, never a name or missing-diffuse heuristic.
`read::material::Material::water` and `MaterialState::water` retain it (`shaunwhite.material` version 2).

Run `lower-shaunwhite-collision`, then `lower-shaunwhite-water{roughness=0.05}`, then `lower-shaunwhite-material`.
The water pass lowers only the flagged materials to `core.material_usage = Water` and a static PBR preview, keeping
the authored colour at 168, mesh and placement. Roughness is an explicit preview choice; wave normals, reflection
cubes, specular shading and animation are omitted and reported. The normal slot's stored UV transform remains
available before lowering; its packed BC3 normal is not treated as an ordinary RGB normal. No shader template id
or independent scalar water-height field has been established: the surface uses its existing geometry and entity
matrix (level 01's harbour entity has source Z -10.999994).

The exact source collision-surface name `Water` maps to shared `SurfaceClass::Water`. `Bail` (and `RobotBail` in
other forges), `NonRidable`, `Unsafe` variants and `InvisibleWall` stay separate source names, without guessed
death/respawn behaviour. The named `01A_DeathZoneSafety` is a Scene volume without a visual mesh, not a water shader
material. A shared water surface class does not encode that volume's scripted respawn behaviour.


## Lights and sun

`read::light::read_light` reads complete OmniLight, SpotLight, DirectionalLight and SunLight bodies. It retains
RGBA, intensity, near/far range, spot cone angles, cookies, shadow flags/resolution and the inline flare. The
AnvilToolkit 1.3.7 AC2 schema supplies the field names; this game's category, five light flags, resolution and
range order were checked on the stored objects. Level 01 resolves 110 omni and 79 spot placements through their
Visual resource ids, at their owning Entity matrices.

`read::light::read_sky` reads the level root's 200-byte SkyLight, including its inline SunLight. Native property
hashes and the serializer establish the sun colour/intensity and AmbientColor fields. The other scalars, vector
and second colour remain unlabelled in `shaunwhite.sky_settings`; no ambient alpha or fog interpretation is made.
This component stays after lighting is lowered. The reader rejects unknown classes, malformed bools, invalid
ranges/cones, non-finite values, truncation and trailing bytes by field name.

A program explicitly chooses the interpretation, for example:
`lower-shaunwhite-lighting{point-scale=100,sun-scale=1,spot-axis=y,sun-vector=to-sun,colour=linear}`.
The two scales convert stored relative intensity to candela (local) and lux (directional); they are **uncalibrated**.
The spot's local +Y and the sky vector pointing toward the sun are hypotheses, exposed as required choices,
not established game conventions. Spot angles are retained as radians. `colour=linear|srgb` states the assumed
source encoding. The pass produces `core.light` and names the sky's directional light in `core.environment`.
It reports replacing source near/far falloff with inverse-square falloff and a far cutoff, omitting character-only
lights, flares and cookies, and retaining resolved lights without selecting day/night or scripted activation.
Global ambient, sky/fog settings and physical calibration remain separate work.


## Restored material layers

Select `lower-shaunwhite-layers{influence=restored}` after content selection/water and before
`lower-shaunwhite-material`. The explicit influence choice selects the colour-restored world. Its RGB equation is
from the game's shipped `ekgeneric.fx` (`FctDiffuseColor`) and `ekcommonps.hlsl`, checked against compiled SM3
pixel shader instructions with Microsoft's disassembler. Vertex `COLOR0` times the mask's RGB supplies the factors:
red interpolates between the first diffuse and that diffuse times its tint; green then lerps to the second layer;
blue then lerps to the third. Each later layer's factor also takes its sampled alpha times tint alpha when its
stored UseTextureAlpha bool is set. The three factors stay in `Channel::Product`; none is discarded or normalized.

The source tints are body 168/332/434: **332 is Layer2_ModulationColor**, corrected from the earlier specular-colour
interpretation. UseTextureAlpha lives at 224/392/494. Native property CRCs identify these fields; independent
source-byte inspection checks their offsets. The material component is version 3. Layer UV sets and authored map
wrap/clamp modes are retained; the map's rotation is retained as source data.

The pass creates up to four ordinary layer materials with `core.layered` in `over` order. A material shared by
meshes with and without colour streams gets a separate colourless drawing (including material variants); the shader
supplies one for absent vertex colour. The original weight stream stays on the mesh. The following generic material
pass lowers the ordinary child materials and supplies the host's roughness/specular approximation.

Current coverage is opaque host materials. Host blend/alpha-test, authored rotated-UV matrices, border addressing,
missing base textures, unread fields and out-of-range tints are named and counted omissions; they retain the generic
material fallback. This does not omit alpha factors of supported opaque materials' later layers. Layer-dependent
specular response, the un-restored greyscale path and the source lighting model remain outside this RGB lowering.
The real-data gate checks the metro's `01_Wall_Metro01Grafitti_Mat`, CliffDry01 and level 06's masked walls. The
mural's second and third weights each retain vertex, mask and sampled-alpha sources; its third map clamps V.

Runtime Direct3D 9 declarations identify COLOR0 and morph COLOR1 as D3DCOLOR (stored BGRA). The mesh reader
converts both to core RGBA. This is also checked against the restored game's metro mural: raw byte order hides the
third layer, whereas BGRA-to-RGBA restores it at the same places. Saturated colour corners remain saturated under
this permutation; they are shader weights, and the earlier red/blue interpretation was reversed.
