# primase-shred

Tony Hawk: SHRED (PS3, 2010), RME archives and rigid scene geometry.

`read::pak::Pak::open` reads one PAK v4; `from_bytes` accepts caller-owned bytes. Metadata validates the header,
name hashes, supported attributes/alignment, zero on-disc runtime offsets, bounded NUL-terminated names and exact
payload tiling. Names remain byte strings, including original case and separators. Sections include their marker
entries; entries before a marker have no section. Allocation alignment is a runtime property, not a file constraint.
Stored entries have `compressed_size == 0` and occupy `size` bytes; compressed entries occupy `compressed_size` bytes.

`payload(index)` borrows stored bytes or lazily decodes one CSTR v1. `encoded(index)` exposes its encoded bytes.
`read::cstr::decode` checks the endian tag, version, threshold, block count, block bounds, exact DEFLATE consumption,
each output length, total output length and exact stream end. Errors retain byte offsets. Neither API extracts paths
to disk. The archive bytes remain in memory; decoded payloads are not cached.

The format contracts come from the disc's PAK/CSTR headers and its native RME loader. Raw DEFLATE decoding uses
the toolkit's flate2 adapter, independently of that loader. Tests include hand-built framing and RFC 1951 blocks,
malformed/truncated inputs, and a real-disc census. The latter is a structural check of the original data, not an
independent semantic decode of the enclosed resources.

Configure `installs.shred` in the local test-data file as the extracted disc root containing `PS3_GAME/USRDIR`.
Run `cargo test --release -p primase-shred -- --nocapture` with that configuration. Smoke depth selects eight archives
by a seeded rotation of sorted paths; full depth opens every archive and decodes every stream, checking the known
disc census: 336 archives, 67,320 entry occurrences, 31,227 streams, 111,391 blocks, 4,221 stored blocks.

`read::scene::decode(scp, mlp, mmp, mgp)` reads one decoded quartet in source coordinates. It checks resource IDs,
slice coverage through EOF (only zero alignment padding), finite affine matrices, identity model-local matrices,
stride-16 positions, stored AABB containment (0.05 source units), BE u16 triangle lists, index bounds and exact
submesh partitions. Unknown formats fail with the file and byte offset. Stride-36 embedded models are individually
refused, retaining their identity, submesh material references and placements; rigid siblings still decode.

`read::level::read_level(&[(label, &pak), ...], frame)` assembles all quartets of precisely the supplied archives.
The program chooses the base/layers. There is one prototype per resource occurrence, one primitive per rigid
submesh, one material entity per global ID and one instance per placement. Submesh vertex ranges are narrowed and
indices rebased. Mirrored placements retain their affine transforms; a mirroring source frame reverses prototype
winding. Refused prototypes have `shred.refused_mesh` instead of fabricated geometry. IDs live in `shred.mesh_id`
and `shred.material_id`. ID zero's bank ambiguity is not resolved by assigning material content.

The caller **must state a `primase_core::frame::SourceFrame`**. Y-up is evidenced by the geometry recon, but units,
handedness and north have not been proven. There is no default frame and no claim that the census's identity frame
establishes physical metres. Conversion of positions, transforms and winding uses core's frame operations.

`read::texture::TextureCatalog::new(&[(label, &pak), ...])` indexes all texture headers in precisely the supplied
archives. `resolve(extensionless_path)` returns every matching record, including repeated packages, streamed and
prefetch copies; it compares slash and backslash equally and preserves case. `records()` exposes each copy's source
archive/section, exact 84-byte header, unresolved words, GCM description and available pixel entries. Missing pixels
remain explicit. MAIN/LOCAL and CACHE_MAIN/CACHE_LOCAL are paired by their source section convention. Duplicate
pixel entries in the matching sections are refused rather than selected.

`decode(record_index, Payload::Primary | Payload::CubeTail, color_space, role)` returns core textures, one per cube
face, through the toolkit GCM codec. The caller selects the copy and its colour meaning. Prefetch and `_m1.txp`
use the source's mip tail (smaller axis at most 128, or the single mip), retaining base pitch for linear textures.
Identity-remapped BC1/BC3 remain compressed; ARGB/B8 and remapped BCn become RGBA8. The 2D API refuses volumes;
`decode_volume` returns an explicit `gcm::Volume` with depth retained at each mip, Z-major slices and no automatic
slice selection. It supports uncompressed XYZ Morton storage and BC1/BC3 VTC storage through the toolkit codec.
This API does not assign textures to materials or choose LOD.

`texture_disc` uses smoke/full depth: eight seeded archives or the whole disc. It decodes every available header/pixel
pair, expands every supported mip through the pixel codec, counts volume/header-only entries, and compares each
prefetch copy with its streamed mip tail and each cube `_m1` with prefetch. This is an independent copy stored by
the game; hand-worked fixtures separately check header parsing, pairing and pixel meaning.

`volume_disc` separately decodes all volume copies and compares their stored prefetch versions. It measures RGB
differences across neighbouring Z slices against a deterministic control that shuffles pixel correspondences within
the next slice (preserving its histogram), with a synthetic positive control. For the source's 32-cube grading LUTs,
it compares all six RGB axis assignments against the canonical RGB ramp. These are content checks of this disc,
not universal promises that arbitrary 3D textures must be smooth or close to an identity colour transform. Smoke
selects eight volume-bearing PAKs from a seeded rotation; full covers all volume headers. The layout descriptions
come from NVIDIA's published VTC extension and RPCS3's RSX consumer at commit
`979a52415003fd7a74ddd216e1a5782b3731281f`; no RPCS3 implementation code was copied. The codec documents its
unproven layout combinations as explicit refusals rather than inferring them from these content metrics.

The returned `LevelReport` counts all occurrences (including packaged copies and refused models) and lists every
refused model and unread attribute/table region with its source and decoded-file offset. MMP +12 and MGP attributes
are not yet emitted as streams; the subsequent attribute chunk owns those semantics. SCP table strides are checked
against the native loader, but their meanings and nested objects remain unread. SCP's opaque coverage notice
intentionally overlaps interpreted fields and is **not** a count of distinct unread bytes.

`read::convert::ShredReader` implements core's conversion reader. It currently decodes/caches geometry during `read`,
returns a planning world without meshes and with explicit material dependencies, and copies only the roots' closure
on `load`. `prototype_box` reports a converted box from the cached rigid geometry. Incremental archive I/O is not
implemented. The adapter emits only `core` and `shred` namespaces.

The hand-worked quartet tests cover two resources, two submeshes of one resource, shared materials, instancing,
reflection, frame scaling/mirroring, malformed inputs, truncated metadata and refusal within a mixed quartet.
`geometry_disc` always traverses every quartet, regardless of smoke/full depth: expected totals are 657 quartets
in 324 PAKs, 21,309 resources (17,897 rigid, 3,412 embedded), 35,114 submeshes and 168,887 placements. These are checks
against source tables and bounds, not an independent rendering proof. Lab/game visual verification remains separate.
