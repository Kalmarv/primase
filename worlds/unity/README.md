# primase-unity

Unity players read through `unity-formats` into the world: scenes, assets, hierarchy, meshes, textures,
materials, lights, terrains, rigs, clips and Animator data, from loose files or configured bundles. The read
converts Unity's frame and retains source state in `unity.*`. Passes select what draws, lower PBR, terrain,
collision, lights, ambient and controllers, and resolve compiled shaders with explicit variant and colour-space
choices. Stored `[HDR]` colour properties retain their linear HDR values; other colours use the player's transfer.
Multi-image textures bind only where their dimensional shape agrees with the program.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_unity::register(&mut passes);
    println!("{}", passes.describe("lower-unity-pbr").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
