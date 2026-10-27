# rme-formats

Readers for the data of Tony Hawk's SHRED (PS3, 2010), which runs on Robomodo's RME engine: bytes in, plain records out.

| module | reads |
|---|---|
| `pak`, `cstr` | PAK v4 archives (big endian; names kept as bytes; stored entries or CSTR v1 blocks of raw DEFLATE) |
| `scene` | a level mesh quartet (`.scp`, `.mlp`, `.mmp`, `.mgp`): models, positions, indices, submeshes, placements |
| `attributes`, `skin` | packed vertex attributes (RSX CMP normals, halves, colours, UVs) and processed skin records |
| `scp_tables`, `scp_splines`, `scp_cameras`, `scp_lights`, `scp_locators`, `scp_occluders` | an SCP's nodes, world matrices and blob sections: spot chunks, splines, cameras, lights, locators, occluders |
| `material`, `pixel_program`, `texture`, `library` | MBP/MDP material banks, fragment programs (uniform names and a microcode classification), 84-byte texture headers with their packaged pixel copies, and the three joined |
| `database`, `session`, `surfaces`, `manifest`, `names` | `MAIN.DB` (SQLite 3) tables, the level/session/goal/task rows, collision surface names, which archives hold a level, spline names |
| `level_params` | the constant-argument calls of a level's Lua 5.1 parameter script |
| `reference_rig` | the Morpheme reference rigs of an animation container |

```rust
use rme_formats::{pak::Pak, scene};

let pak = Pak::open("LEVELS/CHARLIE.PAK")?;
for (stem, entries) in scene::quartets(&pak)? {
    let scene = scene::decode_quartet(&pak, entries)?;
    println!("{}: {} models, {} placements", String::from_utf8_lossy(&stem), scene.models.len(), scene.placements.len());
}
# Ok::<(), rme_formats::Error>(())
```

## What it guarantees

- Lengths, counts and offsets are checked against the bytes they index; a malformed file is refused with an error that
  names the offset (and, for a quartet, the file within it).
- Fields whose role is unproven are kept as stored (opaque words, unknown tables listed with their byte spans), not
  guessed. Names keep their original bytes, case and separators.
- Decoded values are not repaired: a non-finite matrix element, a position outside its stored bounds, an index out of
  range or a non-unit normal is refused (or, for a mesh variant it does not support, that mesh is marked refused while its
  siblings decode).
- Matrices are `Affine` (3x4, column by column) in the game's native frame: left-handed, Y up. Converting frames is the
  caller's.
- The reader does file I/O only in `Pak::open`; everything else takes bytes.

## What it does not do

- It does not decode pixels: `texture` returns the `CellGcmTexture` descriptor (read by `rsx-reader`), the first stored
  mip and the stored bytes.
- It does not decrypt the game's scripts archive, choose which archives make a level, or pick a session.
- It reads the PS3 disc layout only (32-bit big-endian pointers).

## Testing against your own files

`cargo test` runs the synthetic tests. The tests named `*_disc` read the extracted disc (the folder holding
`PS3_GAME/USRDIR`) and print `SKIP` unless `installs.shred` is set in the machine file the `primase-testdata` loader
reads; some also use oracle files listed with their registry entries.

## Licence

GPL-3.0-or-later.
