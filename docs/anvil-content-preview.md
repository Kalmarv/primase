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
The registry shading class is 1. Unknown engine constants/resources still refuse by name. MAD_OutBorder and
RawTerrainBlend's engine virtual textures remain unsupported; their full input/output contracts are not established.
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
