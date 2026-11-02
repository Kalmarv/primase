# primase-blender

Writes a prepared world for Blender, its materials live shader node graphs of core's material model rendering in
Cycles: nothing core states as live is baked. Each texture slot is sampled as it states, with its UV set, transform,
wrap and filters; vertex colour, alpha mode, sidedness and emission are drawn by nodes. Every material writes its base
colour, emission and coverage as render passes (AOVs), so a render can be compared with what core states.

Three stages:

- `write_file`: the world to a folder: `scene.glb` (primase-gltf's writer, unchanged: geometry, placements, lights),
  `materials.json` (each material's node graph and which primitive draws with which) and `textures/`.
- `save::save`: Blender, in the background, imports `scene.glb` with its own glTF importer, builds each graph into its
  material and saves a `.blend`.
- `render::render`: Cycles renders a `.blend` from a stated camera, every setting stated: the image, a multilayer EXR
  and each pass as raw floats.

What has no form in Blender is counted in the report, never chosen around: minification and mip filters, colour
textures Cycles filters before decoding them, occlusion maps, normal maps in Blender's own tangents, specular f90.

## Example

```rust,no_run
use primase_blender::{WriteOptions, write_file};
use primase_core::World;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = write_file(&World::new(), &WriteOptions::default(), "scene".as_ref())?;
    println!("{report}");
    Ok(())
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup (`tools.blender`); the inputs the tests read
are in [tests/inputs](tests/inputs/README.md).

License: GPL-3.0-or-later.
