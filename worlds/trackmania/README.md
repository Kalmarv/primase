# primase-trackmania

TrackMania Nations Forever over the GameBox reader (`gbx-formats`): a map (`.Challenge.Gbx`) placed block by block
from the install's own block models (its packs and `GameData`), with their meshes, materials, textures and collision,
the terrain its collection's default zone leaves, and the spawn locations. Passes lower the materials to glTF's
model, the spawns to markers and the physics surfaces to surface classes.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_trackmania::register(&mut passes);
    println!("{}", passes.describe("lower-trackmania-materials").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup (`installs.tmnf`).

License: GPL-3.0-or-later.
