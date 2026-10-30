# primase-gta

GTA V data read through `rage-engine-formats`: an install, regions, placements, drawables, textures, lights,
collision and shader records. Source data stays in `gta.*` until selected passes lower geometry, materials,
lights, water, clips and collision or select regions, stand-ins and interiors. Archive keys are caller-supplied.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_gta::register(&mut passes);
    println!("{}", passes.describe("lower-gta-pbr").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
