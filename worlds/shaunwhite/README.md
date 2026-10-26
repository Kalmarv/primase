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

`select-shaunwhite-content{helpers, effects, characters, missions, influence}` (the first four `keep` or `drop`, `influence`
`restored` or `unrestored`, none defaulted) drops
the placements a map should not draw, by the rows of `passes::ROWS`: each a name test on the Entity, the prototype Mesh
or one of its materials, with the level 01 evidence that gives it its class, plus the vertex flag that skins a mesh
(`MeshFormat::SKINNED`) and the cell name that tells a mission cell from a grid cell. The source states no hide flag this
reader can decode, so the rows are the game's names, not a field: the pass says so in its approximation. It runs before
`lower-shaunwhite-material` (which removes the material names the material rows match). Every drop is declared by its
row, and the prototypes, materials and textures only the dropped placements reached go with them.
Collision-only placements remain: these options select what Visuals draw, without removing authored physics.

`influence` is the world's state for the two trees of each tree Entity. Every tree of level 01 (36 Entities) holds a
RealTreeMesh Visual (`shaunwhite.real_tree` marks its prototype) and the stored Mesh `00_Veg_RealTree_UrbanTree`, a metal
tree: a pole, branches and white light strips. The game draws the metal tree in both states and nothing hides it: its five
Materials (`00_Min_MetalTree01_Mat`, `Gen_Light_Tree1600`, `Gen_Light_White_No_Emissive_Mat`, `00_Veg_TrunkA_Mat`,
`00_Veg_TreeLeavesA`) store `InfluenceHidePixelMode` 0 (`HideNone`, the u32 at body 525, native property `0x7dacf81c`),
and the game's shader-key generation (`SKGenericMaterial` at `0xce2e44` and `0xce2e60`) sets `KILL_UNINFLUENCED_PIXEL`
only for a mode other than 0 and `INVERSE_KILL_UNINFLUENCED_PIXEL` only for mode 2, so neither is set. The RealTree is the
part that changes with the world: its leaf shader (`leaves.fx`) discards a leaf pixel where the influence is 0, and
`ActivateRealTreeGrowthEvent` grows it. The game's un-restored screens (level 01 views `g_c01`, `g_c06`) show the metal
tree and no leaves; a restored screen with the trees grown (`r_tree1`, the event fired in the live game) shows the foliage
with the metal tree's white strips among it. So `restored` keeps both Visuals of a pair and `unrestored` keeps the stored
Mesh and drops the RealTree, which has not grown there. The growth event is not decoded further; the pass says so in its
approximation. An Entity with only one of the two is untouched, and the pass counts both kinds.

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
The two scales convert stored relative intensity to candela (local) and lux (directional). Numbers are
**uncalibrated**; `point-scale=sun:LUX` calibrates from the level's own sun instead. The game's local lights and its sun
share one unit (its lighting multiplies colour x intensity by N.L, a local light also by a linear `(far - d) /
(far - near)` window; `ComputeOmniLightAttenuation`, `eklighting.hlsl`), so taking the sun's luminance x intensity as LUX
(100,000 at noon, as the GTA lights do) gives one lux per stored unit for both: `LUX / (intensity x luminance)`, 42,000
to 53,000 across the six levels (suns of 2.0 to 2.5). A level-05 light of intensity 0.95 is then 40,000 cd, 330,000 lm
as skate. writes it (8.1 lm per cd): San Vansterdam's own lights are 75,000 to 225,000 lm at 13 to 27 m, the
community's floodlit parks 0.6 to 2.1 million lm at 40 to 50 m, while `point-scale=100` is 770 lm. The game's static
world is lit by baked lightmaps, per-vertex lighting and composite spherical harmonics (the `STATIC_LIGHTMAP`,
`STATIC_VERTEXLIGHTING` paths of `eklightingps.hlsl` and the `COMPOSITELIGHTING` spherical harmonics of `ekcompositelightingcommon.hlsl`), which this does not carry.
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

The pass creates up to four ordinary layer materials with `core.layered` in `over` order. Each core layer selects its
map's coordinate set and tiling (`Layer.texcoord`, `Layer.transform`); its material samples `TexCoord(0)` untiled, as
core's layer contract states, so target lowerings move the selected set into each drawn part's first set. A material shared by
meshes with and without colour streams gets a separate colourless drawing (including material variants); the shader
supplies one for absent vertex colour. The original weight stream stays on the mesh. The following generic material
pass lowers the ordinary child materials and supplies the host's roughness/specular approximation.

A Material with only a diffuse layer is the equation's first step alone: `lerp(texture, texture * tint, COLOR_0.r)`. With
a white tint that is the texture, so `lower-shaunwhite-material` marks every Material `VertexColor::Ignored` (a vertex
colour is never a colour multiplier in the shader; level 01's plaza planters carry `COLOR_0` of about (0.93, 0, 0) on a
white tint and are grey concrete in the restored game, not red). With a tint, a texture and a drawing with `COLOR_0`
the layers pass builds the two-layer form (untinted base, tinted layer weighted by the vertex red), 101 materials of
level 01; with no `COLOR_0` the texture times the tint is exact. The vertex alpha does reach the output, as the alpha
of a blend or test, and core can multiply `COLOR_0` or ignore it but not take its alpha alone: a blended or
alpha-tested Material with a vertex alpha below one keeps the multiplier (54 of level 01's 551 materials drawn with
`COLOR_0`, their vertex red, green and blue then an approximation), counted by the material pass.

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

## Normal maps

A Material's `normal` slot is a BC3 texture of format code 12. The shipped shader (`SampleNormalMap`, `ekcommonps.hlsl`)
reads X from the **alpha** channel and Y from **green**, each `(v - 0.5) * 2`, rebuilds Z as `sqrt(1 - x^2 - y^2)`, scales
X and Y by the Material's `HeightScale` and takes `x * T + y * B + z * N` with the vertex tangent `T`, the normal `N` and
`B = cross(N, T) * w` (`DecompressTangentBasis`, `ekcommonvs.hlsl`). That is core's tangent frame with green along the
bitangent, so the reader gives every normal texture `NormalConvention { x: Alpha, green: Up, z: Rebuild,
basis: TangentBitangent }` (`read::texture::NORMAL_CONVENTION`), and the Mesh reader gives each primitive whose vertex
has room for it a `Tangent(0)` stream (xyz as the normal, the fourth byte the sign: 255 is +1, 0 is -1; 4,581 of level
01's 4,646 primitives, every normal-mapped one). `HeightScale` is the float at body 265 (property hash `0xd963a8e7`,
the CRC-32 of `HeightScale`, last field of the slot's `SKGenericMaterialNormalMap` object; 1 on 788 of the 798
Materials with a normal map, 0.75 to 1.5 on the characters' clothes) and becomes `pbr.normal_scale`.
`lower-shaunwhite-material` lowers the slot to `pbr.normal_texture` at the slot's tiling; a blend's (`core.layered`)
normal map is omitted and counted.

Checked against the pixels and vertices (`tests/normals.rs`, level 01's 15 character maps with a diffuse map, the
characters being the only static-height normal maps in the game's levels; the others are water waves): red and blue
are constant, the alpha channel correlates with the diffuse luminance's horizontal gradient (mean -0.102) and not its
vertical one (-0.004), green with the vertical gradient up the image (-0.071) and not the horizontal (-0.008): a map
baked from height tilts away from the slope, and a crease is dark where it is low. The vertices say the tangent
follows +u and `cross(N, T) * w` follows -v (up the image) on 99.4% of 30,784 triangles. Level 01 has no brick or wall
with a normal map: the maps belong to the characters and the water effects.

## Shader databases

`read::shaders::read` reads an uncompressed SDB2 v39 (`ekshadersPChigh.bin`, `ekshadersPCmed.bin`,
`ekshadersPClow.bin`). The caller chooses the quality file. `read_into` attaches the chosen database as
`shaunwhite.shader_library`; a failed read leaves the world unchanged. Material permutation selection and
binding resolution remain separate work, so merely loading this resource does not select a material shader.

The database retains every source's raw bytes, CRC-32 and dependency identity; every table key (kind,
64-bit permutation, 128-bit source dependency mask); raw vertex/pixel bytecodes; both native stage cache
keys; literal render-state assignments; save/restore state placeholders; both sampler-binding groups
(sampler name, texture parameter name, packed address, sampler-state pairs); and runtime state-variable
names. It checks source CRCs, unique keys, spans, zero table reserves, payload tiling and the exact end of
every metadata tail. Unknown versions/kinds and compressed siblings are named refusals. D3D9 state values
are retained in their original vocabulary, without destination policy.

Kinds 4 and 8 carry the same 48-byte BEEF header and counted metadata. Kind 8 is a **state-only pass**,
not a vertex declaration: the native loader saves, sets and restores its render states without shader
bytecode. Zero-length kind-4 records retain their keys as `Payload::Missing`, distinct from a shader with
an empty body. Kinds 5..7 hold utility programs; kind 6 is retained without guessing a purpose (the native
loader marks its presence without loading its stage). The generic `primase_d3d9::frame` walks SM3 token
lengths through END, including comments and immediates, without treating an END-shaped operand as a terminator.
Framing preserves operations a future lifter may refuse; it is not semantic shader validation.

The hand-worked fixture exercises every record kind, both sampler groups and truncated/corrupt inputs.
The real-data test reads all three quality databases to the exact end. High and medium each contain 62
sources and 6,456 records (54 uncompiled keys); low has 62 sources and 3,919 records. High record 5's pixel
program is the same one checked against Microsoft D3DDisassemble by `primase-d3d9`. Game sources,
bytecode, disassembly and native decompilation remain outside the repository.
