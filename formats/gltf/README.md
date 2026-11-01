# primase-gltf

Reads glTF 2.0 / GLB into a world and writes prepared worlds as GLB: shared prototypes, placements, PBR
materials, embedded images, lights (`KHR_lights_punctual`), rigs and clips. The writer writes standard glTF and the
Khronos extensions it names, nothing of primase's own: what glTF has no form for is counted by component name, never
put in extras. It chooses nothing a pass should: a layered material (`core.layered`) is written from its `pbr.material`
and left out, counted, without one (a pass before the writer states it, a layer as the material or the blend baked).
A writer for a format built on glTF extends what it writes through `extend` (the web format's tiles). `prepare` supplies material and
missing-texcoord passes; `GlbParts` writes conversions incrementally and `preview` retains choices in a manifest and
recipe.
The container codec is `primase-glb`. The shared frame is glTF's: metres, right-handed, +Y up.
Writers report omissions and approximations; they do not load files or run preparation passes.

## Example

```rust
use primase_core::World;
use primase_gltf::{WriteOptions, write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (glb, _report) = write(&World::new(), &WriteOptions::default())?;
    assert_eq!(&glb[..4], b"glTF");
    println!("{} bytes", glb.len());
    Ok(())
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
