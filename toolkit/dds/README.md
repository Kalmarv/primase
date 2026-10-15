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

The legacy writer supports BC1/2/3 and uses a stable compatibility header verified
against independent C# fixtures. It writes the supplied mip chain unchanged and
does not preserve sRGB tagging; use DX10 when that tag matters. Neither writer
serializes role or normal annotations.

Run `cargo test -p primase-dds -- --nocapture`. Tests include synthetic headers,
format mappings, malformed inputs and encoded-level roundtrips. Environment-driven
censuses use `PRIMASE_SCENES_GTA` and `PRIMASE_SCENES_SEKIRO`, recursively selecting
DDS beneath `textures` directories. The legacy byte-identity gate selects DXT
sources only; other formats participate in read and DX10 roundtrip gates.

Header references: [DDS DX10](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dds-header-dxt10),
[DDS pixel format](https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dds-pixelformat).

Censuses deduplicate input files by SHA256 within each configured root, reporting
path counts separately from checked distinct contents. All paths are read and hashed.
