# Riders Republic content preview

Written during the 2026-10-24 write-only experiment. The new readers, passes and fixtures have not been compiled,
formatted, executed or compared with game data during the experiment. This document describes implemented code,
not a successful gate or a new measured coverage result.

The source evidence reused is the local Riders Republic conversion at commit 8134ee5, specifically
`docs/farview.md` sections 2, 3 and 6; `material-spec.md` sections 25–27; `lighting.md` section 2;
`sky.md` sections 2 and 6; `effects.md`; and the existing registry-driven sky scripts under the private
`scratch/riders-republic/rr-materials/sky` directory. Those are earlier findings requiring confirmation against
the installed build. No conversion source, game files, shaders or extracted image assets were copied here.

`AnvilImpostor` retains the 71-byte header, four owned TextureMap references, all 21 GPU-table words and every
27-float card. Unsupported counts, lengths and non-finite cards are refused. The table starts at 115 and cards
at 199; consequently word 20 is at 195. The earlier note's table labels overlap at 199, so the implementation
uses the table's stated word count and the complete-body length rather than repeating that inconsistent label.
Both direct resources and selector forms retain these records and their atlases.

`lower-anvil-impostor{form=side-cross}` draws all authored side cards. RGB comes from Diffuse; coverage and the
asset-space normal come from WorldNormalAlpha, with the normal expressed in each card's tangent frame; roughness
comes from Misc.g. The source transfer function, UV rectangle and coverage threshold are preserved. Derived
images and materials are shared for repeated occurrences of the same resource. This is an explicitly static
preview: the engine picks one azimuth card, whereas this pass draws a cross. Top-view switching, depth/self-shadow
atlas evaluation, translucency and instance LOD dithering are omitted and reported.

`select-anvil-preview-lod` changes only tiny stand-ins whose material is identified as MAD_FakeForest and whose
selector provides an actual canopy or impostor form. Ordinary small meshes are unaffected. The existing
MAD_FakeForest vertex-colour pow(saturate(c), 2.2) formula and MAD_Fake far-atlas lowering already existed; they
were not replaced with guessed texture recipes. AnvilRenderStatus now distinguishes authored untextured content,
successful formula lowering, successful own-program baking and a refused/unknown material recipe. Bake refusals
retain their specific reason when the later formula lowering also refuses.

`select-anvil-far-view{rect=...,radius=...}` is a caller's static composition policy. It treats source IsFake
records as merged object proxies, an actual Impostor form as a tree, the identified FakeForest material as canopy,
and other LongRange objects as real objects. Real objects are retained by their bounds. Merged proxy cells
overlapping the near rectangle are omitted whole because per-object span hiding is not decoded. Tree and canopy
positions are restricted to the documented 640–3500 m and 2500–15000 m bands, respectively, and the caller's radius.
The distances are from the near rectangle, not a claim to implement the engine's live camera selection. Every
class's keep/drop count is logged. FarFake is an object proxy, not far ground. The separate coarse-ground pass
described below supplies the terrain ring; this object policy does not reinterpret proxy meshes as terrain.

SpotLight and managed LightInstance records have dedicated strict readers. `lower-anvil-lights` requires caller
choices for stored-intensity-to-candela scale, RGB transfer and full/half cone interpretation. It preserves the
owning transform and range. The lab supplies scale 1, linear colour and half angles as preview calibrations;
these values are not claimed as measured engine units. Shadow flags remain unknown. Open-world emissive geometry
does not become a fabricated point light.

LayeredSky keeps the documented 24-frame and one-frame profiles and all five generated scattering buffers.
Unknown resource profiles fail explicitly. The static time/weather controller implementation, temporal LUT
interpolation and source environment are described below. WorldAmbiance's owned cube remains independently
inspectable; it is not substituted for dynamic GIDyn lighting.

TextureMap cube/array storage now becomes core.texture_layers without decoding its blocks. The supported volume
profile is one stored mip; multi-mip volume storage is still refused because its source slice packing is not
established. The shader resolver accepts layered bindings and preserves their shape and transfer flag.

Runtime effect components and unsupported Visual instance classes retain named source explanations at their real
owner transforms. This exposes the source class/identity instead of silently emitting an empty row. Particle
simulation and effect property-program evaluation remain unsupported. Existing water, collision and terrain-decal
lowerings remain the data producers for those features; no arbitrary object-hiding or decal-order workaround was
added. A decal entry without receiving terrain must say why it cannot project.

Files written here:

- `worlds/anvil/Cargo.toml` (runtime JSON summaries).
- `worlds/anvil/src/components.rs`, `components/impostor.rs`.
- `worlds/anvil/src/read/mod.rs`, `read/impostor.rs`, `read/light.rs`, `read/sky.rs`, `read/controller.rs`, `read/listing.rs`,
  `read/placements.rs`, `read/textures.rs`, `read/convert.rs`.
- `worlds/anvil/src/passes/mod.rs`, `passes/impostor.rs`, `passes/preview.rs`, `passes/far.rs`, `passes/far_terrain.rs`, `passes/surface.rs`,
  `passes/lights.rs`, `passes/sky.rs`, `passes/listed.rs`, `passes/material.rs`, `passes/bake.rs`,
  `passes/world_uv.rs`.
- This document.

Written fixtures cover impostor length/reference rules, separate alpha ownership and sharing, preview-form
selection without switching an ordinary sign, far-object/proxy selection, layered block order and truncation,
light calibration/cone interpretation and sky buffer/profile accounting. They have not been run. Named refusals
do not count as decoded coverage. The lab's per-area census ranks the remaining actual map materials.

## Round 2: map materials, coarse terrain, static atmosphere

The reused map denominator is the placed-USNAT audit in local conversion `docs/material-spec.md` §26–27,
not the whole-forge inventory. Among the missing named recipes addressed here, UniqueNormal has **20 distinct
map materials**, Cliffs_TriPlanar **6**, UniqueNormalGrad_Triplanar **3** (44 draws). These are historical source
counts from that audit, not freshly recounted results on this branch. Classic's 1224/843 IDs are not a map-priority
denominator. The local `scratch/primase-reset/anvil/composition-diff.md` also identifies CITYSCHOOL's one
UniqueNormal placeholder and the absence of a conversion-local Classic path. Reused execution evidence:
`scratch/riders-republic/rr-materials/verify_m17.py` and the accompanying Cliffs/UniqueNormal DXBC disassemblies.

`bake-anvil-material` now includes UniqueNormal with the established Rocks property/texture bindings and its own
pixel program, which writes layout A and detail-normal-alpha roughness. No new colour formula is substituted.
`bake-anvil-surface` is the surface-domain executor, first used by Cliffs_TriPlanar. It resolves the template's
feature-gated G-buffer key, material rows and texture roles, checks all constant reads, and evaluates 2×2 quads
over actual placement positions/normals and the template's original vertex inputs. The program performs projection and blending. Normals are decoded
from its packed output class and rebased into each triangle's atlas tangent frame. Class 1 and every kept target
must match the established contract; unknown varyings, engine constants and resources retain named failures.
There is no catch-all zero binding for unknown engine inputs. The only stand-ins are the stated uniform terrain
AO/no-sediment views, finite virtual-page coordinates, static motion values and sampler-address choice.

Each triangle has a private chart and propagated gutters. The caller chooses texels/metre and atlas/chart limits;
capacity failures refuse rather than silently lowering density. Each affected placement gets a separate prototype
and derived material. `AnvilSurfaceBake` links back to the original material so census counts retain template
and distinct source identity. Stored source programs/textures remain available. Alpha-tested/blended surfaces,
mirrored/sheared/nonuniform placements, variants and unsupported extra streams are named refusals. Capacity is
checked before input image decode or program execution. Original UV0/COLOR0 are preserved on remapped vertices;
the derived PBR material samples its private atlas through UV1, so the retained Game program still gets its masks.

The three UniqueNormalGrad_Triplanar materials now have their own surface contract, read from the existing
`rr-materials/audit/mat17/MAD_Tex1_UniqueNormalGrad_Triplanar.txt` and
`rr-materials/recipes/MAD_Tex1_UniqueNormalGrad_Triplanar/ps_400000.dxbc.asm`, alongside material-spec §27.14.10.
Its TEXCOORD0 is original mask/dirt UV, TEXCOORD1 is source normal, TEXCOORD2 source world position, TEXCOORD6
static clip. It does not use Cliffs' COLOR0. t75–t80 resolve through the template's six stored roles; cb3 values
remain the template/material values. cb1[0].xyz is the caller-stated static camera above the primitive centre
(`camera-distance=1000000` in the lab). The own program computes DetailBlendRange; no cb3 value is overridden
to suppress the distance term. This is static detail blending, not the viewing camera changing every frame.
The registry shading class is 1. Unknown engine constants/resources still refuse by name. The next round's
OutBorder contract and Raw_TerrainBlend's missing engine-resource requirements are detailed below.
These are written contracts, not new successful draws or a new material census.

`AnvilFarTerrain` holds coarse nodes separately from near `AnvilTerrain`. The lab reads levels 5/4/3 for its
explicit far_view option; no extra coarse rectangle is accidentally lowered as near splat. The ring uses the
documented `raw * range / 2^20 * 2^(8-level)` height scale and 4/8/16/32/64 m static tessellation tiers. Every near
boundary sample participates in the inner edge. Zipper strips join unequal subdivisions without T junctions;
a caller-stated 16 m smooth height band reaches the coarse surface. Shared ring boundaries use the same samples
even at LOD changes. Colour triangles are clipped to node boundaries and sample each node's stored BC1 block-1
tile with its border. Near detail ends at that boundary: the colour LOD transition is reported, not a claim to
match the engine's live blend. No dynamic camera LOD, runtime fake-span hiding or near-object ring is invented.

`read/controller.rs` consumes TimeOfDay/WeatherPropertyControllerData in registry order: nested entry/path/node
headers, scalar path profile, packed first-time/key-count header, float/vector/byte tracks and exact trailers.
Unknown layouts refuse. Values are interpolated at the stated hour (30-second ticks); weather input is an
explicit static scalar. Full property-path suffixes disambiguate cloud/fog names. Byte tracks use held values.
LayeredSky now retains its authored sun mode/angles/intensity and Mie defaults, and interpolates stored radiance
tables across the midnight boundary. The lab states Clear_Day at noon, weather input zero, for USNAT maps.

The source now emits only generic resources: sunlight and hemispherical mean ambient in `core.environment`,
fog in `core.atmosphere`. The duplicate AnvilEnvironment/AnvilFog components and lab-only source_environment
route are removed. A private temporary panorama is used for integration and the optional asset sphere only.
Consequently the environment preview is a flat mean-ambient approximation, not the original directional
scattering panorama or probe. This reduction is stated in the source notice and renderer environment report.
The explicit Studio/image choices still belong to the viewer. Exposure stays None for Anvil: no stored exposure
track's units have been established, and no camera multiplier is invented from sun intensity.
Clouds, moon, stars, forecast, GI relighting and effects are omitted with notices. Fog is an explicitly approximate
exponential slab, not the game's volumetric simulation. `FogFalloff::ExponentialHeightSlab` integrates the camera
segment's length inside absolute Y-up bottom/top metre bounds, density per metre, zero outside. Fog colour is
source albedo times the evaluated hemispherical ambient. Density-to-engine-unit equivalence and constant
in-scattering are stated approximations; source phase, noise, shadows and multiple scattering are omitted.
Photometric calibration/post-process grading are omitted.

**Unresolved, explicitly:** lighting.md §7 proves the controller's 0.317 is not noon sun elevation, and the world
dump's 51.29 belongs to 08:42:30. The SunPositionMode elevation(hour) formula remains unknown. This code uses the
source's authored manual elevation with a named notice and controller azimuth, never labels the dump value noon.
Consequently this is a source-driven static preview, not a verified reproduction of the default game's sun pose.

Additional written fixtures cover controller first-key times/interpolation, source colour/fog, midnight LUT
interpolation, surface chart derivatives/class decoding, zipper boundary coverage, coarse height scaling and
colour clipping. The lab adds preparation-order, generic-environment Before-generation and three-falloff/exposure fixtures. All remain
unrun under write-only mode; no new successful material count, image comparison or memory reduction is claimed.

## Next round: border ground, runtime exposure and the sun-vector producer

This round only reads files and writes source/docs. All new fixtures remain unrun. The map denominator reused
from material-spec §27.11/27.14 is OutBorder **one distinct material / four draws**, Raw_TerrainBlend **one / twenty**;
it is historical evidence, not a new census or successful-render count.

### OutBorder

Evidence: `scratch/riders-republic/rr-materials/audit/mat17/MAD_OutBorder.txt`,
`rr-materials/recipes/MAD_OutBorder/ps_400000.dxbc.asm`, and material-spec §27.0's default-texture correction.
The own G-buffer program is now a surface-executor contract (class 1), not a substitute colour formula.

| Input | Program meaning / binding |
|---|---|
| TEXCOORD0.xy | original mesh UV; the program samples its base at UV/16 |
| TEXCOORD1.xyz | source world position, used by the sediment lookup |
| TEXCOORD2.xyz | authored tangent in source world axes |
| TEXCOORD3.xyzw | authored normal and tangent handedness |
| TEXCOORD4.xyw | static clip stand-in, velocity is not baked |
| cb3[1..3].x | Sand, Sedimentary, Rock tilings; material/template values retained (75, 3, 3 in the audited material) |
| t75..t80 | template-bound maps: rock detail normal, base diffuse, blend mask, sand detail normal, object normal, sedimentary detail normal |

The old audit listed only the material's explicit DiffuseMap record. That does not mean the other five maps are
absent. The template supplies OutBorder_Details_02A/03A/01A normals at t75/t78/t80 (channel 2), a set-slot mask
at t77 (channel 1, slot 7), and t79 is the material/set normal. The reader's existing selector/default resolution
remains authoritative; unresolved maps still refuse by t slot/property, and are never filled with guessed normals.

Like the existing Cliffs preview, the lab explicitly selects `sediments=omit`: t16's sediment-ID view
is zero, so the own program's optional t20/t22 branch is not taken. This approximation is in both pass and material
notices; it is not a claim to supply the running terrain cache. The base, tilings, detail blending and packed
world-normal output still come from the source program. The derived atlas uses UV1; UV0 and the original tangent
stream survive, and the baked normal is rebased into that same authored tangent frame. This matters to the retained
Game binding as well as PBR. Missing tangents, mixed triangle handedness and unknown varyings refuse by name.
No OutBorder image or new success count has been collected in write-only mode.

`bake-anvil-surface` now requires the enumerable `sediments=omit|refuse` caller choice, exported as
`SurfaceSediments`. With `refuse`, a program reading the unresolved sediment views is refused by name. The lab's
preparation explicitly states `omit`; the library does not silently choose that approximation for another program.
Callers replaying older pass strings must state the choice. RawTerrainBlend still refuses with either choice,
because its base terrain inputs are missing too.

### Raw_TerrainBlend: established contract, unresolved producer

Evidence: the matching `audit/mat17/MAD_Raw_TerrainBlend.txt` and
`recipes/MAD_Raw_TerrainBlend/ps_400000.dxbc.asm` (7,648-byte G-buffer permutation). The exact template spelling
is `MAD_Raw_TerrainBlend`. Inputs are TEXCOORD0.xyz normal, TEXCOORD1.xyz source world position,
TEXCOORD5.xyw static clip; class 1 outputs. cb3[1..3].x are **ContourBlendOffset, ContourBlendHardness,
TerrainNormalInfluence**, audited values 0, 1, 0. There are no material texture slots.

The surface pass recognizes this template but deliberately produces a detailed named refusal, not a coloured
material: t13/t14 address/level lookup, t15 height, t16 sediment IDs, t17 colour, t19 AO, t20 sediment layers,
t22 sediment records; t45/t46 virtual detail caches; uint3D t50/t51 page tables and u5 feedback. cb0[47..50]
states terrain mapping/height/detail transition, cb0[146,148,149] the virtual layout; cb1[3..5].w supplies camera
position. The branch before t50 runs when `sat((distance - cb0[50].z) / cb0[50].w) < 1`. The far branch still
needs t15/t17 height/colour data. Turning TerrainNormalInfluence to zero does **not** remove the colour/height
lookups. AnvilTerrain's decoded TNL samples and the lab splat bake are not these runtime GPU views. A UV tile,
zero cache, or the nearest unrelated terrain texture would fabricate the program's inputs.

To execute it, the next producer must rebuild those cache/page views from the terrain reader at the mesh's
actual world positions, or provide a fully identified offline frame's views/constants. Cached engine programs
`cache/riders-republic/rr-terrain-sonnet/shaders/1987..2004.txt` (virtual atlas/render-tile/used-tile families)
are the next offline comparison, alongside `docs/terrain.md`'s TNL-to-page mapping. Required observation: the
same frame's t13–t22, t45/t46, t50/t51 and cb0 rows, including their formats/dimensions; a physical RAM dump
alone cannot supply GPU-only image contents. RawTerrainBlend is still **not decoded/rendered** by this round.

### Exposure: separate the three quantities

Reused identities are in `scratch/riders-republic/rr-rras-oracle/regtest/a/shaders.tsv`, whose header identifies
the unpacked menu image SHA256 `73cd846f39b26c314248c392bca1b761ae82a192245ef188ea5465b1e66e72d8`.
Cached disassemblies are `cache/riders-republic/rr-terrain-sonnet/shaders/0026.txt`, `0027.txt`, `0028.txt`,
`0070.txt`, and `scratch/riders-republic/rr-materials/warp-run/sky1815.dxbc.asm`; ambient consumer is
`rr-materials/sky/448.txt`. Unlike the old note's unresolved shader search, the registry names 26 AnalyseHistogram,
27 ComputeHistogram, 28 UpdateExposure, 70 ACESLUTACESTonemapperHDROutput, and 1815 SunSkyModel.

* **Lighting cb1[39].x is a linear multiplier**, proven by sky 1815's `mul radiance, cb1[39].x` and the ambient
  consumer. A resolved positive value `s` converts to `Exposure { stops: log2(s) }`, implemented by
  `read::sky::exposure_from_lighting_scale`. Zero/negative/nonfinite values refuse. This is not an EV conversion.
* **t23.x is a linear divisor** in G-buffer emissive storage, `E/t23`; ambient adds albedo-tinted emissive and
  multiplies by cb1[39].x. Equality of those two resources is not established just by their names.
* **Controller ColorGradingControllables/Parameters/Exposure** (0.8 at the stated Clear_Day noon look) has no
  proven mapping to either multiplier. Program 70 multiplies RGB by cb5[0].rgb and cb5[0].w, so `.w` is a scale
  at that program boundary; the CPU conversion from the authored Exposure track to that lane is still missing.
  Do not assign 0.8 stops or log2(0.8) by guessing which side performs the conversion.

The adaptation program 27 converts weighted luminance to base-2 log bins (128 bins, approximately -23..9).
26 averages accepted log bins then applies `exp2(mean_log + cb4[36].x)`. Program 28 takes `log2(value*5464)`,
clamps it by cb4[0].zw, exponentiates and multiplies by a literal printed `0.000183`, then takes the reciprocal.
It optionally adapts in log space using cb4[0].x and cb4[1].xy, and exponentiates an interpolated cb4[2..33].x
curve before writing a 1x1 UAV. Printed DXBC literals are rounded: the exact binary multiplier must be read
before identifying it as precisely 1/5464. This proves log-space adaptation and an exposure-curve contribution;
it does not identify the CPU bindings of MinEV/MaxEV/EmissiveCurve or connect this UAV to t23/cb1[39].x.

Decisive existing captures: `cache/riders-republic/rr-materials/mem/cb1-world.tsv` records validated 954x582
frame candidates with row39=(1,1,1,1), e.g. physical offset 535692192 in `rr-3-world.raw`. These are one session,
not the Clear_Day noon default. Need its same-frame cb4 for shader 28 (rows 0,1,2..33, buffer/u4 identities)
and cb5[0] for shader 70, then trace the CPU writers back to the authored controller's Exposure. GPU t23/UAV
contents are not recovered by that CPU RAM dump. The pass now reports the sampled grading Exposure, MinEV,
MaxEV and EmissiveCurve with their full paths and an unresolved-mapping notice, leaving core exposure None.
The conversion helper is ready for a resolved frame multiplier; no current archive reader produces one.

### Automatic sun position: exact boundary of the proof

`Controller::at` already implements the proven formula: `tick=120*hour`, then linear interpolation between
the bracketing u16 tick keys (held values for byte tracks). The source manual preview computes ENU direction
`(cos(e)*sin(a), cos(e)*cos(a), sin(e))` from the stated azimuth and authored manual elevation in radians,
then applies the ENU-to-core frame. This formula is **not** the automatic-mode formula.

PS_SunSkyModel/1815 consumes the evaluated sun direction at **cb4[9].xyz** (uses z for LUT sun elevation and
dot-products with the view ray); cb4[7].xyz is the moon, cb4[23] is Mie. Its interface has no hour, tilt, mode or
raw elevation track. Therefore the missing conversion is upstream CPU code, not arithmetic to invent inside
the scattering-table sampler. The registry records LayeredSky class 24D32CF8 controllable fields 7F2FE2D2 at
runtime store 0x940 (azimuth), 4BB6DB5D at 0x950 (tilt), 2997D8D7 at 0x960 (elevation parameter). Those are
field stores, not a decoded function address or proof of the nested controllable's active-value offset.

Existing evidence is `cache/riders-republic/lighting/report3.txt`: evaluated control blocks in `rr-3-world.raw`
at physical 0x2782357c and 0x27823d6c (the report's occlusion-pattern anchors, with leading fields at negative
offsets), inferred tick1045/08:42:30, azimuth111.55, tilt45.7, computed third parameter51.29 versus track0.317.
Four nearby unit vectors exist; none has that elevation, and their sun/moon/star roles are not settled. That
single observation cannot identify the formula. `rr-1-start` has no 64-bit game state, and `rr-2-menu` has no
matching world control blocks: they are not additional sun observations.

The decisive offline observation is the native writer that reads those LayeredSky fields plus time and fills
cb4[9].xyz. Its code is in the already unpacked menu PE
`cache/riders-republic/dump-work/ghidra/RidersRepublic-menu-rebased-0x140000000.pe.bin` (same SHA above;
REPORT.txt says all .text pages present). Follow that writer to resolve the mode enum and any latitude/orbit
calculation, then compare its output vector against the already-on-disk world snapshot at tick1045. No native
writer address or exported disassembly for it was found in the text evidence read this round. A second world
time, if an offline capture later exists, must include the same controller identity and the actual cb4[9]
vector, not only a third scalar named elevation. No Riders Republic launch is requested or authorized.

## Archive coverage and the World boundary (write-only census round)

This is a code audit and newly written code, **not a run result**. No build, test, archive scan or image run was
performed in this round. The counts below are the older build-25394987 inventory in the conversion's
`docs/type-names.md` (six forges, 1,301,684 stored entries, 905 type IDs); they include base/patch copies, not
active map placements or distinct resources after patch resolution. A dash means no applicable entry count.
The new census must supply current counts and bytes before these figures are used for prioritization.

Reused format evidence: the conversion's `docs/lighting.md` §4 and §8 (29-byte event presets, 49-byte forecasts,
and GI's 418 checked one-volume/one-box bodies); `docs/effects.md` §3–4, `docs/entity-grammar.md`,
`docs/registry-named-fields.md`, `docs/terrain.md`, `docs/farview.md`, `docs/type-names.md`; and primase's
`read::{convert,placements,entity,terrain,listing,sky,controller,light}`. These are the local earlier findings
behind the Riders Republic registry/entity/terrain-effects knowledge pages. Old correlations and names are
not promoted into physical units or activation rules. The older lighting note's controller-undecoded and
sky-undecoded statements are superseded by the readers already in this tree.

`read` below means a retained **stated subset** has an audited path; it never means every byte, permutation,
instance or engine effect is supported. `read-but-dropped-before-World` identifies consumed structural data,
which is not automatically a defect. A resource may be a dependency rather than a standalone catalog preview.

| Type | Type ID | Older stored entries | Status and retained subset / open boundary | Owner / lab display |
|---|---|---:|---|---|
| Entity | `0984415E` | 615,110 | read: placed visuals, collision, water, decals, state; remaining bounded components retained once per owner this round | primase reader; geometry/overlays, Outliner and Properties Data |
| EntityGroup | `3F742D26` | 56,837 | read: own Entity and expanded members; original full membership/hierarchy not retained as a group component | primase reader; occurrence rows, source component Data |
| World | `FBB63E47` | 4 | read-but-dropped-before-World in region composition: GridDescriptions consumed; other World fields, including baked sky-transfer maps, not semantically decoded | primase reader; overview plus new standalone layout Data row; unread World tail remains explicit |
| GridPartition | `0E2F4444` | 8 | read-but-dropped-before-World in region composition: cell selection/layout consumed | primase reader; overview/region controls plus new standalone cell-reference Data row; not a drawable object |
| GridCellDataBlock | `AC2BBF68` | 66,359 | read: reached owner/cell/level in AnvilListed; complete original list not copied | primase reader; placement source data, overview |
| Mesh | `415D9568` | 147,289 | read: compiled vertex/index/range data and material links | primase reader; meshes, Outliner, Mesh/Material Properties |
| LODSelector | `51DC6B80` | 48,862 | read: meshes/impostors and authored distance metadata | primase reader; LOD forms and bounds overlay |
| Impostor | `DFCD6E51` | 117 | read: cards and owned atlases; static-card approximation explicit | primase reader; geometry and material Data |
| Material | `85C817C3` | 13,579 | read: parameters/texture/shader links; unsupported programs keep named failures | primase reader; swatches, Material Properties, notices/sentinels |
| MaterialTemplate | `BCFB3C7A` | 432 | read: parameter roles, G-buffer stages/permutations; not every runtime binding is established | primase reader; bound material/shader inspection, no standalone template renderer |
| TextureMap | `A2B7E917` | 46,365 | read: established stored formats, mips/layers; unknown storage profiles refuse | primase reader; Image editor/contact sheets and texture Data |
| TextureSet | `D70E6670` | 10,027 | read: referenced texture slots retained through material/terrain owners | primase reader; owner texture rows; no standalone set composition |
| CompiledMip | `1D4B87A3` | 25,031 | read: external image level attached to parent map | primase reader; parent Image editor's mip selection |
| TerrainNodeLayer | `DCE0537E` | 1 | read: selected TNL nodes in AnvilTerrain | primase reader; near splat, explicit far tile/ring, collision overlay |
| TNLChunk | `4A054F51` | 87,381 | read: decoded selected height/material nodes; not the runtime virtual cache views | primase reader; terrain geometry, layered materials |
| TerrainMaterialBank | `4BE94285` | 1 | read: bank's material/detail ownership | primase reader; terrain material/layer rows |
| TerrainMaterial | `01724188` | 195 | read: established detail parameters and TextureSet links | primase reader; terrain layers/material Data |
| FloorHeightData | `92ADA771` | 2 | read: StoredFloorHeight::component retains coarse blocks; regular area composition uses TNL instead | new standalone floor catalog/Data row; vertical lowering remains caller-controlled, with a notice |
| MeshShape | `B22B3E61` | 25,162 | read: triangles/material indices | primase reader; collision outlines/fill, Data |
| SphereShape | `FA3F7A18` | 241 | read: analytic source shape | primase reader; lowered collision overlay |
| CapsuleShape | `B8599052` | 290 | read: analytic source shape | primase reader; lowered collision overlay |
| BoxShape | `4EC68E98` | 29 | read: analytic source shape | primase reader; lowered collision overlay |
| ReferenceListShape | `2D675BA2` | 3,731 | read: bounded nested shape placements | primase reader; collision overlay; cycles/limits named |
| CollisionMaterial | `74F7311D` | 263 | read **identity/name only**: physical material body remains unread | primase reader for physical fields; lab collision surface/name rows |
| DDecalModel | `50563D58` | 70 | read: model textures/parameters via deferred-decal owners | primase reader; projected geometry/materials, unsupported family notice |
| GrassPatternLibrary | `463CC1B8` | 1 | read: patterns/models retained in AnvilGrass | primase reader; lowered grass geometry/materials; simulation excluded |
| SpotLight | `80320FB8` | 32 | read: stored spotlight via LightInstance; units require caller calibration | primase reader; core lights, light overlay and Properties |
| LayeredSky | `24D32CF8` | 48 | read: established scattering/keyframe profiles | primase reader; scene Environment/Atmosphere, sky and source Data |
| WorldAmbiance | `21C2D472` | 110 | read: owned sky/cube; lens flares and particle/weather simulation omitted | primase reader; sky/cube preview and named omissions |
| TimeOfDayPropertyControllerData | `F0C85018` | 140 | read: established path/key profiles in AnvilSkyControllers | primase reader; evaluated scene lighting; source tracks in Data |
| WeatherPropertyControllerData | `887004DC` | 63 | read: established scalar-path profiles in AnvilSkyControllers | primase reader; selected static state, unsupported paths named |
| GIDynResource | `BAE98B30` | 16,956 | new read: one-volume/one-box grammar, bounded V1/V2/V3 RLE, V4 probe positions; **irradiance unresolved** | primase reader; new `gi` catalog, blue position markers, source Data; not added wholesale to area reads |
| AmbianceAndTimeOfDayPreset | `E8ECCA01` | 290 | new read: season/forecast refs, hour, angle and flag kept separately; angle's role unresolved | primase reader; new `preset` catalog, metadata/notice, no invented placement |
| ForecastPreset | `79B0E782` | 248 | new read: twelve finite stored lanes; lane meanings not inferred from names | primase reader; new `forecast` catalog, metadata/notice |
| Season | `F7485E9F` | 75 | unread: names/reference targets known; complete 306/313-byte grammar and event selection not established here | primase reader |
| WeatherRegionCellData | `F2BB7836` | 7 | unread: region weather ownership/overlap not decoded | primase reader |
| TerrainSediments | `29191D7B` | 5 | unread: sediment/runtime cache producer missing | primase reader |
| TerrainProceduralSet | `2F45393E` | 132 | unread: procedural ownership/cache recipe | primase reader |
| TerrainProceduralMaterial | `4323218D` | 479 | unread: detail-cache material producer | primase reader |
| TerrainProceduralSetBank | `65C3C7D0` | 1 | unread: bank grammar not established here | primase reader |
| RiverWorldComponentData | `11BF3F3A` | 1 | unread: separate from already read/rendered river meshes/materials | primase reader |
| SplashFX | `755ACE14` | 395 | unread: placed Visual has named unsupported status, no particle simulation | primase reader for stored action graph; simulation excluded |
| FX | `824A23BA` | 800 | unread: component bytes now retained; referenced command graph not decoded | primase reader; source Data/unsupported owner marker |
| WorldParticlesDescriptor | `8F3D5BB6` | 14 | unread: complete descriptor grammar not established | primase reader; simulation excluded |
| SpawnPointSettings | `53903478` | 492 | unread: settings are not a world transform; do not invent spawn points | primase reader |
| SpawnEntityBuilderDescriptor | `AC8AF394` | 329 | unread: builder/activation grammar | primase reader |
| Animation / Skeleton / BuildTable | `0FA3067F` / `24AECB7C` / `22ECBE63` | 9,111 / 1,138 / 10,741 | existing read paths counted; characters and animations excluded from this round's implementation | existing reader/display owners |

### Embedded components are a different census

`WaterComponent` (`996DF94F`), `StaticDecalComponent` (`2F3A7F6D`),
`HoudiniDecalInstancerComponent` (`4148CD56`), `StaticDDecalComponent` (`5B413085`),
`RigidBodyComponent` (`C177D702`), `GrassComponent` (`CB28F06D`), `GrassVisual` (`8FA80D8D`),
`LightInstance` (`554C614C`) and the far-view instance flags are embedded objects, **not extra forge entries**.
They already reach typed water/decal/collision/grass/light components or mesh instances. FarFake, FakeForest,
IsNearFake and LongRange are placement classifications/list policy, not four newly discovered archive types.
`WaterComponent`'s lake atlases and geometry, river material lowering, and far-ring selection have different
consumers; an unread RiverWorldComponentData does not mean that all water is unread.

Before this round, `ComponentBody::Raw`, `Unread` and `Refused` could be parsed/delimited and then disappear in
Placer, except for counters. Unhandled bodies now live in `AnvilSourceComponents`, once per owner.
`AnvilSourceOwner` links every occurrence to that one payload, so selecting a later instance retains the data
without retaining an out-of-region first instance. An owner with no static output gets a marker at its actual
stored transform and a named explanation. Length-rule-only is explicitly **not semantic decoding**.
Successfully typed water/decal bodies keep their typed path; a failure in those later typed reads remains a named
refusal (retaining every original successfully typed water/decal body again is not this change).

The notable still-opaque map component families are `FXComponent` (`D2BF93A6`),
`AmbientEffectGeneratorComponent` (`713F0C7D`), `WildlifeSpawnerComponent` (`A8ABFC79`),
`SpawnerComponent` (`E2791E7F`) and `PlayerSpawnActivatorComponent` (`A7033693`). Their counts cannot be taken
from the forge table. Existing `tests/census.rs`'s Entity outcomes remain the full embedded-body census.
The effects note's 226,918 ambient-generator owners / 227,389 instances are an older, narrower observation,
not today's result. Most are sound generators; treating that count as missing particle visuals is wrong.
The registry establishes many length rules, but the polymorphic action/reference semantics and activation
conditions need their own complete body contracts before a visual/spawn reader is written.

### What the new census lines decide

`tests/resource_coverage.rs` is not ignored. Its ordinary inventory test opens each forge index in turn,
recursively below `installs.riders`, without decoding payloads. It fails on directory/index errors, counts
unknown IDs too, and writes `scratch/anvil-resource-coverage.json`. `ANVIL_RESOURCE_TYPE` carries type ID,
status, exact stored-entry count, stored payload bytes, destination, three example identities and the next
observation. Global known zero-count rows are included. A configured installation with no forges fails;
an unconfigured installation uses the existing explicit testdata skip/require policy. Bytes exclude tables
and padding and are **not decoded bytes**. Nothing deduplicates patch copies or equates them to active content.

The separate ordinary profile test writes `scratch/anvil-map-resource-profiles.json` and
`ANVIL_MAP_PROFILE` lines for at most three entries per named open map type per forge, plus new GI/look types.
It records decoded body length, a 96-byte prefix, resource-decode error or named new-parser refusal. This
is bounded sampling, not a claim to parse every occurrence. A prefix and an entry count alone cannot establish
a variable-length body grammar.

| Census line / open row | Decision it supports / exact evidence still needed |
|---|---|
| Every `ANVIL_RESOURCE_TYPE` | Current breadth and storage rank, patch duplication scope, unknown type IDs and concrete entries to inspect. It does not prove rendering. |
| `ANVIL_MAP_PROFILE`, GIDynResource | Whether sampled files close the documented one-volume/one-box profile and RLE sizes. Other profiles keep the exact refusal. Irradiance still needs V2 post-mask grammar, sample origin (especially WhiteRoom), grid flag semantics and the GPU GIDynData producer. |
| `ANVIL_MAP_PROFILE`, look presets | Whether the 29/49-byte profiles and reference forms still close. Season activation and forecast lane mapping require the selected event's Season chain and the actual controller consumer; the angle must not become sun azimuth by guess. |
| World / GridPartition | Which stored layouts are consumed. Standalone catalog reads now retain layouts/cell references for Data inspection; remaining World fields and baked sky-transfer maps need exact field offsets/dimensions from the existing lighting notes, not a generic reference scan. |
| TerrainSediments / ProceduralSet / ProceduralMaterial / bank | Which concrete size/profile families and referenced resources exist. Full bodies plus registry ordered fields must establish sediment/cache ownership; t13–t22, t45/t46 and t50/t51 contracts above still decide runtime surface baking. |
| RiverWorldComponentData | The single resource's exact body/profile, then the typed join to RiverQuadTree instances. Already decoded river mesh/material content is the control. |
| SplashFX / FX / WorldParticlesDescriptor | Which body-size families to inspect and which state/action classes need full extents. A class/action name is insufficient to parse a polymorphic array, and no simulation is authorized by this inventory. |
| SpawnPointSettings / SpawnEntityBuilderDescriptor | Which settings/builders are present, then the Entity component that owns transform and event/state activation. Until that join is established, settings rows are not spawn markers. |
| Season / WeatherRegionCellData | Exact reference-vector/cell-bounds profiles and event/region ownership. Counts do not decide the active look or overlap priority. |
| Embedded Entity outcomes | Raw/refused class extents and exact owner/component IDs for the missing semantic readers. These counts have a component denominator; never add them to the forge-entry totals. |

### Lab display audit

The existing Outliner `hierarchy` includes every remaining entity under Data; Properties' selected-entity Data
panel asks the component for its JSON. No changes to `ui/properties.rs` or `ui/outline.rs` are needed for the
new typed metadata. `AnvilGiDyn` and `AnvilLookPreset` provide structured JSON, and retained source components
provide type, ID, status and exact body bytes. Source records have their own selectable Data identity; an
occurrence's `anvil.source_owner` gives that identity. The scene builder adds GI position markers and named
metadata/unsupported explanations. Markers' blue colour is a diagnostic, never decoded irradiance. Presets
have no authored position, so they get a notice/Data row and no invented viewport origin.

Standalone coarse FloorHeightData and stored World/GridPartition/GridCellDataBlock layout/reference data now
have catalog/Data rows and named notices, without inventing placed geometry. Remaining composition rows are
the additional unread World fields and area-selected GI (rather than loading all ~5.7 GB of GI resources by default).
Dependency types such as CompiledMip, TextureSet, MaterialTemplate, TerrainMaterial and DDecalModel display
through their owning texture/material/terrain/decal; this is not a standalone-resource renderer for each.
Generic raw Data inspection is not a substitute for the still-missing semantic readers in the table.

### First-check failures addressed in the same write-only round

The reported failures were read from the first-check logs; no rerun is claimed here.

| Check | Cause and source change |
|---|---|
| `parse_modules_are_checked` | Controller, impostor, light and sky byte decoders lacked checked-parser guards. Added the guards, checked offset/count arithmetic and slice access; applied the same boundary to new GI/look readers. Fixture allowances are confined to tests; numerical curve/stencil helpers state their bounded arithmetic separately. |
| `worlds_layout` | Component fields named I/O-owned reader modules. Stored records and component class names now live in pure `components::stored` / `components::component_types`; existing `read::*` paths re-export them so callers keep one type identity. |
| `game_names` | An exposure notice used the word for a multiplier of one that also names another engine. Reworded the notice in `passes/sky.rs`. |
| Anvil Clippy | Replaced manual divisibility checks and constant chunk iterators; collapsed the tangent check; moved the impostor/preview/sky/texture test modules after their implementation items. No assertions were weakened. |
| Lab full preparation | The logged binary's surface OptionSpec omitted `sediments`; the current source declares the typed option and the lab states `sediments=omit`. This is a source/pin comparison, not a claim that the test has been rerun. |

## Shaded / Game surface drift (source audit)

This is a source comparison, not measured image drift. It covers the map templates dispatched by
`lower-anvil-material`, `bake-anvil-material` and `bake-anvil-surface`; character templates are excluded.
Evidence reused: `tests/shader_oracle.rs`, the existing installed-template bake fixtures, the conversion's
material-spec sections 12.2, 14–15, 23 and 27, and the resolver's output roles. The output decoder is now
`core::shader::surface::Surface` in both map bake paths as well as the lab. `passes/gbuffer.rs` only associates
role ordinals with actual target registers and checks written/finite lanes; it does not define another output layout.

In the table, B/N/R/M/E/O/U mean base colour / normal / roughness / metalness / emission / occlusion / UV.
`S(x)=x*x*(3-2*x)` and `tint` is the existing HSV adjustment. Each row states the differing source, rather than
claiming that a shader's remaining lighting, blend state or runtime input is represented by PBR.

| Template | Shaded source versus program source; difference kind | Written fix / open |
|---|---|---|
| MAD_Fake | B=t75; N=t76.rg; R=t76.a; M=t76.b; E=0, O=1. Without TextureMap metadata Shaded previously forced linear sampling, while the Game binding uses core.texture's declared transfer (colour space). | Fixed fallback for original and derived maps; installed Fake fixture checks PBR B/R/M against Surface. |
| MAD_Fake_Sediment | Fake's terms; Shaded omits terrain albedo/sediment inputs when their switches are on (missing terms). | Transfer fixed; terrain views remain named omissions. |
| MAD_Tex1 | B=tint(t76,t77.r,HSL), R=S(sat(t75.a+offset)), M=t75.b*metal, O=1+AO*(t77.b-1), E=0; same U on all maps. Shader colour factor is RGB-only `1+VCI*(COLOR0.rgb-1)`; Shaded VCI=1 also multiplies vertex alpha, other nonzero VCI is omitted. | Transfer/static U fixed; vertex colour/alpha remains explicitly approximate. |
| MAD_Tex1_SkinningSupported | Same pixel terms as Tex1; Shaded does not add a new skin formula. | Same fixes/vertex limitation; resolver class-1 output contract added from the saved program. |
| MAD_Tex1_Wind | Tex1 B/O; raw t75.a/b for R/M. Program also writes translucency, and its vertex program moves geometry. | Transfer/static U fixed; translucency absent from PBR, deformation remains Game's program. |
| MAD_Tex1_TerrainBlend | Tex1-like B/N/R/O, M=0; program blends terrain colour by position while Shaded omits that term. | Transfer/static U fixed; terrain sampling open, named approximation; O lighting domain differs below. |
| MAD_Decals1 | B=tint(t76,1,HSL), R=S(sat(t75.a+offset)), M=t75.b*metal; no mask/O term, unchanged TEXCOORD0. Blended program writes alpha where opaque writes roughness. | Transfer fixed; a blended G-buffer has no roughness role: fixed Shaded roughness is an approximation, not an opaque output contract. |
| MAD_Decals1_Graffiti | Decals1 only where EmissivePower=0; team/Fresnel terms otherwise require runtime inputs. | Same fixes; nonzero case keeps named refusal; class-1 output contract added from the saved program. |
| MAD_Tex1_Emissive | B=tinted diffuse; R=normal.a, M=normal.b*metal; program E=B*(t77.r*scale/t23). Shaded derives tinted diffuse*mask and factors scale/stated exposure separately. VCI affects program E too; Shaded vertex colours multiply base only. | Transfer/static U fixed; exposure t23 and per-vertex E remain named limitations. |
| MAD_Tex1_UGE | Emissive variant with t77.a and its own U rows; instance override blends emission scale in the program. | Same fixes; instance blend and vertex E open. |
| MAD_Tex1_Graffiti_Mode | Tex1-shaped B/N/R/M/O with its own rows and raw R/M, only logo/team/custom state off. | Transfer/static U fixed; other states keep named refusal. |
| MAD_Tex1_TeamColoured | Tex1 with Neutral HSL; negative hue wraps in program, unlike the formula's signed fractional hue. | Nonnegative hue path retained; negative hue refuses. Transfer/static U fixed; class-1 output contract added from the saved program. |
| MAD_Tex2 | Two tinted diffuse/normal/mask sets, U per layer, R=S(sat((a+offset-.1)*1.25)), M=normal.b; heights diffuse.a, weights from COLOR0.a and contrast/offset. Program also multiplies colour by vertex intensity, which Shaded omits. | Transfer/static U fixed; vertex colour and generated-image filtering open; class-1 output contract added from the saved program. |
| MAD_Tex2_TintBFromShaderConstants | Tex2, B-layer tint from Neutral constants; same missing vertex factor. | Same fixes/open terms and class-table entry. |
| MAD_Tex2_Graffiti_Mode | Tex2 with raw R and own rows, no colour-factor term, only State Value=0. | Transfer/static U fixed; nonzero runtime palette refused; class-1 output contract added from the saved program. |
| MAD_Grounds | Shaded takes layer A tint/N/raw R/M at Base tiling only under the established neutral tire/blend reduction. Program's general branch mixes tire/parquet and another layer/UV. | Reduction retained; other branches refused, not guessed; transfer/static U fixed; class-1 output contract added from the saved program. |
| MAD_Bark | B=diffuse, N=normal.rg, R=normal.a, M=0, O=mask AO. Program further multiplies O by COLOR0.a and writes specular occlusion/translucency. | M fixed at 0 even without a normal map; exact extra-output equations/values retained in material notices. Transfer/static U fixed; vertex AO and separate lighting terms open. |
| MAD_Tex1_Leaves | B=HSV-tinted diffuse, alpha=opacity.r, N=normal, R=mask.g, M=0, O=mask AO. Program adds instance tint mix, vertex AO and foliage lighting terms. | M fixed at 0 even without a normal map; exact extra-output equations/values retained in material notices. Transfer/static U fixed; instance/vertex terms, missing opacity fallback, fade/thinning remain named. |
| MAD_Clutters_Grass | B=diffuse, alpha=opacity.r, N=normal.rg, R=normal.a, M=0, O=mask AO. Program additionally samples world-space noise tint and writes specular occlusion/translucency/fade. | M fixed at 0 even without a normal map; exact extra-output equations/values retained in material notices. Transfer fixed; world noise, foliage terms and missing opacity remain named. |
| MAD_FakeForest | Shaded writes pow(sat(COLOR0.rgb),2.2) to vertex colours; program computes that after interpolation. Nonlinear operation before versus after interpolation differs. Program also writes translucency=.5. | Open: retain original Game inputs and evaluate power after interpolation for equality; named translucency omission retained. |
| SHD_Nat_Veg_Grass_AO | B=diffuse*(1+AO Mult*(COLOR0.a-1)), R=1, M=0; Shaded rewrites vertex colour to that linear factor, shader also supplies custom normal and specular occlusion=.2. | Transfer fixed; unresolved custom texture operator/normal and specular term remain named; class-4 output contract added from the saved program. |
| MAD_Outskirt | B=t75 at world UV=(source P.xy+offset)*scale; N=t76 at Tiling times U; R=1,M=0. | Existing world-UV lowering required before display; transfer fixed; class-3 output contract added from the saved program. |
| MAD_Tex1_FakeRiver | B=foam-alpha formula with caller's constant s versus program COLOR0.x; N=t75 strength .1, R/M=sat(Metalness). | Transfer/static U fixed; varying s remains approximate; class-1 output contract added from the saved program. |
| MAD_Tex1_Screen | B=.6*sat(sample)^2.2, R=.1,M=0; E=B*(.666667/t23). The shader's explicit pow follows texture sampling; it is not a substitute for the resource's sRGB decode. | Declared transfer fixed before pow; t23 remains caller-stated; class-1 output contract added from the saved program. |
| MAD_Screens | Shaded black/R=1/M=0/E=0 only under reduced logo/video flags; general program reads video/state and another UV. | Reduction retained; other branch refused; class-1 output contract added from the saved program. |
| MAD_SHD_UNLIT_UndergroundBox | B=lerp(ColorB,ColorA,caller view factor), R=1,M=0,E=0 versus program factor from view direction. | View dependence remains named approximation; class-1 output contract added from the saved program. |
| MAD_Tex1_Graffiti_Mode_DecalFX | B=team^2.2 plus atlas alpha, R=1,M=0; E=B*(EmissiveMult*mask.g^2/t23), mask uses its own U. Exact reduction requires opaque team and TeamBlendRatio>=1. | Transfer/static U fixed; unreduced stripes/custom state refused, exposure stated; class-1 output contract added from the saved program. |
| MAD_Tex1_Rocks | Own program B/N/R/O, layout B M=0/E=0, three UV sets. Previous second hand decoder replaced by Surface reading; terrain AO/sediments are stated views. | Shared decode/transfer fixed; terrain, sampler and finite-resolution choices remain approximate. |
| MAD_Tex1_UniqueNormal | Own program, Rocks input roles but class-1 outputs; R from detail normal alpha. | Shared decode/transfer and HDR E retention fixed; same engine-view limitations. |
| MAD_Tex1_UniqueNormalGradient | Own program blends micro/medium detail by camera distance; Shaded bakes one distance. | Shared decode/transfer fixed; dynamic camera distance and class-1 output contract added from the saved program. |
| MAD_Tex1_UniqueNormal_DiffuseBlend | Own program B/N/R/M/O, masks at distinct tilings; Shaded previously independently decoded output lanes. | Shared decode/transfer fixed; unset AO maps retain refusal/choice; bake/filtering precision remains. |
| MAD_Road | Own program stripe paint at one period; R/M from normal and parameter. Program E=B*(mask*scale/t23), previously discarded by Shaded bake. | Fixed HDR E texture/strength; period transform also applies to E. Existing Road program fixture checks actual installed PBR against Surface. Class-1 output contract added from the saved program. |
| MAD_Hero_Ice / MAD_IceBrick | Own program B/N/R/M/E at stated sun/eye; previous bake discarded nonzero E. | HDR E/shared decode/transfer fixed; angle/parallax stand-ins remain named; class-1 output contracts added from the saved programs. |
| MAD_Tex1_Graffiti_Monster | Own program normal sum/detail tilings; Fresnel E depends on eye. Shaded discarded E even after evaluating it. | HDR E/shared decode/transfer fixed; fixed eye/object origin and sampler assumptions remain; no invented dynamic state. |
| MAD_Cliffs_TriPlanar / MAD_OutBorder / MAD_Tex1_UniqueNormalGrad_Triplanar | Surface chart evaluates own program at actual world P/N and original UV/tangent/colour inputs; N rebased to chart TBN. Independent B/N/R/M/O lane decode could diverge from Game. | Shared decode fixed; non-neutral specular/translucency, nonzero E or out-of-range albedo refuse chart rather than silently clip. Static camera/terrain choices remain named. |
| MAD_Raw_TerrainBlend | No lowered output: program requires terrain page/cache/sediment resources not supplied by the surface contract. | Named refusal unchanged (t13–t22, t45/t46, t50/t51); no fake PBR match. |

### Common differences still open

The formerly open class rows above are now assigned from the saved programs' normal-pack instructions,
not from o1.w looking like metalness. Successful formula lowerings of other, unlisted classes still report
their missing contract. The new regression assertions read the actual encoded class on each existing
oracle's random program outputs; map UV bakes also refuse a class that disagrees with the declared contract.

Occlusion is another semantic difference: the existing output roles specify **all external light** while
`core.pbr` stores **ambient** occlusion. Both bake paths and ordinary PBR lowering now report this when they
retain an O map; no new light multiplier is guessed. Specular occlusion, translucency and layout-A o3.y's
six-bit payload remain separate terms, never packed into metalness or ordinary AO.

Texture generation evaluates nonlinear HSV/roughness/mask products at stored texels, sometimes resampling a
second map by nearest texel; Game evaluates after filtering each original texture at its own footprint.
Those operations do not commute. Exact per-fragment equality for varying textures/mips needs the original
program/material operation graph, not another sampled-constant recipe. RGBA8 base/ORM/normal quantisation
and finite bake density are also limits. HDR base colour clipping in the UV bake is now named; HDR emission
has its own range-preserving strength. Surface charts refuse unrepresentable albedo instead of claiming equality.

Static UV rows now have one implementation (`S R uv + translation`), used by the Game binder and both bake
preparation and formula lowering. Uniform-scale rotation plus translation is retained rather than silently
dropped. Unequal-scale rotation cannot fit TextureRef's `R S` and keeps the exact surface-bake reason; moving
transforms retain the unknown-time notice. Periodic UV-square bakes check all four matrix coefficients, not
only their diagonal. Sampler addressing, exposure t23, instance overrides, terrain caches and source-to-core
normal/UV conventions remain choices/refusals already named by the passes.

Written regression coverage uses existing assets only: the existing modes.dxbc signature checks output-target
reordering, lane masks and non-finite values; installed MAD_Fake checks sRGB fallback against Surface;
installed MAD_Tex1 encodes sampled UV into the program's albedo and compares the actual lowered TextureRef
with Surface; installed MAD_Road checks HDR E and ORM from the actual lowering against the same program's
Surface reading. Installed fixtures use the usual explicit testdata skip/require policy. No new fixture binary,
run, image comparison, format pass or successful check is claimed in this write-only round.

### Decidable drift follow-up

Source evidence reused in this follow-up: the saved `rr-materials/recipes/<template>/ps_400000.dxbc.asm`
files, `MAD_Decals1_Graffiti/ps_404004.dxbc.asm`, `MAD_Tex1_Graffiti_Mode_DecalFX/ps_404000.dxbc.asm`,
the Leaves `t16742-08e817` and Clutters `t256868-054b67` disassemblies, and material-spec sections 12,
14.4–5, 15.2, 15.4 and 27.14. These are existing offline files; none was regenerated or executed this round.

The pack reads a sign, uses `bfi(..., width=1, offset=0, sign, literal)`, adds the normal's two high bits
with weights 2 and 4, converts to float and writes o1.xyz divided by 255. Literal 8 fixes class 1;
24 fixes class 3; 32 fixes class 4. The normal payload cannot alter those class bits. This establishes
the following previously missing contracts independently of varying normals, texture samples and material values:

| Saved template program | Contract now written |
|---|---|
| MAD_Tex1_SkinningSupported, MAD_Tex1_TeamColoured, MAD_Tex2, MAD_Tex2_TintBFromShaderConstants, MAD_Tex2_Graffiti_Mode, MAD_Grounds | Class 1: o1.w metalness, o2.z albedo-scaled emission, o3.x all-light occlusion. |
| MAD_Road, MAD_Hero_Ice, MAD_IceBrick, MAD_Tex1_UniqueNormalGradient, MAD_Tex1_Screen, MAD_Screens, MAD_Screens_Simple, MAD_SHD_UNLIT_UndergroundBox, MAD_Tex1_FakeRiver | Same class-1 contract. A valid class does not imply that all engine inputs or dynamic video branches can be baked. |
| MAD_Decals1_Graffiti, MAD_Tex1_Graffiti_Mode_DecalFX | Class 1, checked in the saved cut-out permutations named above. |
| MAD_Outskirt | Class 3: o1.w is AO=1, o2.zw=(1,0), no metalness/emission. |
| SHD_Nat_Veg_Grass_AO | Class 4: o1.w=1, o2.z=.2, o2.w=0, no metalness/emission. |

| Drift row | Written result | Still open |
|---|---|---|
| Vertex RGB, alpha and emission | Per-material notices now retain `B=tintedDiffuse*(1+VCI*(COLOR0.rgb-1))`, coverage `diffuse.a*Alpha` without COLOR0.a, and `E=B*(maskLane*scale/t23)` including the same RGB factor. The actual VCI, Alpha, mask lane and scale are printed. Tex2's notice separates its alpha blend weight from the RGB factor. | Core vertex colour supports the default RGBA product or Ignored, not this RGB-only/affine fragment product or an emission vertex factor. No new core field was invented; the retained program remains the executable representation. |
| Bark/Leaves/Clutters metalness | PBR metallic factor is explicitly 0, including when the normal map is missing; it previously defaulted to 1 in that case. Existing program signatures/Surface and actual lowering are compared in a missing-map regression. | Missing normal images are still missing; zero metalness does not fabricate them. |
| Bark vertex AO | Notice retains `O=(1+AOonoff*(mask.b-1))*COLOR0.a`, o2.z=1, o2.w=0, actual AOonoff. | Core's AO has no vertex-alpha factor and applies to ambient light only. |
| Leaves AO/foliage/coverage | Notice retains `B=instanceTint*sat(COLOR0.a+VertexAO)`, O's vertex-alpha product, `specularOcclusion=mask.b*SpecularOcclusion`, `translucency=mask.r*Translucency`, opacity from slot-18 red, near/dead-tree controls and the unresolved cb3[0].z cut-out reference. | Instance hash/state, vertex-alpha AO, specular occlusion/translucency and dither lack equivalent PBR terms. |
| Clutters foliage | Notice retains the full mask-red translucency, constant specular occlusion **including zero**, AO mask-blue formula, world-noise HSV blend and wind constants. Blended target 2.w is now Unknown coverage, never opaque translucency. | World-noise sampling, per-instance t86 fade and separate transmission/specular occlusion remain explicit. |
| Wind/FakeForest/Grass_AO | Wind notices retain row/value pairs (vector values preserved), the retained vertex-program boundary and the pixel debug switch. FakeForest names power-after-interpolation versus power-before-interpolation; Grass_AO names its linear alpha-AO factor and .2 specular occlusion. | Original Game colour inputs must survive any Shaded mesh rewrite. Specular occlusion is not lowered to core.Specular: that changes dielectric F0/Fresnel, not the downstream occlusion term. |
| Moving UV operators | Notices retain Scale/Translation/Rotation and TranslationSpeedU/V/RotationSpeed, property key and cb3 row. The Game constant refusal and bake refusal carry the rates too. | Operator activation bytes are not retained by MaterialValue; rate-to-clock conversion/units are not established. Core has no UV-animation component; ObjectParameter::TextureScroll is a supplied current scale/offset, not a clock/rate component. |
| Screens flipbook UV | Notice names the cb0[0].x clock, FPS, delay, SpriteCountXY and the modulo/cell formula; it distinguishes the clock-independent black reduction from refused video states. | Frame playback and missing video/grid inputs remain unsupported rather than represented as a continuous UV scroll. |
| Program texture transfer | The Game texture binder now shares the same TextureMap-or-core.texture fallback as Shaded/bakes, including layered textures. | It still trusts TextureMap metadata where present; this is not a role-based colour-space guess. |

Notices are attached to `AnvilRenderStatus.reason` as well as PassLog, with template and material identity,
so the parameter equations remain inspectable after `anvil.material` is consumed. They are not new shading
channels. Existing program oracles now check encoded classes; Bark/Leaves AO with non-neutral vertex alpha,
Leaves' saturation threshold and foliage outputs, Clutters' zero specular-occlusion case, Wind translucency,
and emitted colour including vertex RGB are also written as regressions. No new shader fixture binary was added.
