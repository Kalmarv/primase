# DDS containers

`read(&[u8]) -> Result<Texture>`, `write_legacy(&Texture)` and
`write_dx10(&Texture) -> Result<Vec<u8>>` retain encoded mip bytes.
`from_dxgi` and `to_dxgi` expose the strict format/colour-space mapping.

The reader supports legacy DXT1/3/5, ATI1/2, BC4U/S, BC5U/S, exact uncompressed
mask layouts and typed DXGI formats. Legacy DDS reads as linear because it lacks
a colour-space tag. DX10 preserves sRGB. Role defaults to Other; normal metadata
is absent. L8A8 retains its two stored channels as Rg8Unorm without expansion.

Only tightly packed 2D images are supported. Arrays, cubes, volumes, 1D resources,
nonzero DX10 alpha/misc metadata, padded rows, numeric FourCCs, premultiplied
DXT2/4 and typeless/unknown formats return explicit errors. Invalid sizes, short
mips, excessive levels, invalid pitch/linear size and trailing bytes are errors.
Reserved header words are not retained. A zero mip count means one level.

The legacy writer supports BC1/2/3 and sets DDS_HEADER flags from the actual
texture shape. DDSD_MIPMAPCOUNT is set only when there is more than one level. It writes the supplied mip chain unchanged and
does not preserve sRGB tagging; use DX10 when that tag matters. Neither writer
serializes role or normal annotations.

Run `cargo test -p primase-dds -- --nocapture`. Tests include synthetic headers,
format mappings, malformed inputs and encoded-level roundtrips. The real-data
census reads Sekiro's own DDS files (`oracles.scripts.sekiro/pbr/census/dds`, the
real-data configuration in CONTRIBUTING.md). The legacy payload-identity gate
selects DXT sources only, measuring header field differences separately; other
formats participate in read and DX10 roundtrip gates.

Header references: [DDS header](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dds-header), [DDS DX10](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dds-header-dxt10),
[DDS pixel format](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dds-pixelformat).

Censuses deduplicate input files by SHA256 within each configured root, reporting
path counts separately from checked distinct contents. All paths are read and hashed.
