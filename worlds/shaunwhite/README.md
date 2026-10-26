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
