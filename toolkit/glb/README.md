# GLB codec

The glTF 2.0 binary container and the mesh encoding that every crate reading or writing GLBs shares. Frames are
glTF's (metres, right-handed, +Y up); nothing is converted.

- `parse` / `assemble` / `Document`: the container, strictly. The header length is the file's, chunk lengths are
  multiples of 4, the JSON chunk comes first, at most one BIN chunk follows, and nothing else is in the file.
  Assembling pads the JSON with spaces and the buffer with zeros. Over 4 GiB is refused.
- `Gltf`: the strict reader. Meshes go into `primase_core::Mesh`. Anything that could change the geometry and that
  the reader doesn't understand is an error naming the item: sparse accessors, morph targets, skins, external
  buffers, required extensions, unknown properties.
- `load_mesh_files`: a reader's load step. Every `core.mesh_file` prototype gets its `core.mesh`, and a GLB that
  several prototypes share is parsed once. `mesh_file_box`: a mesh file's box from its POSITION accessors' bounds,
  reading only the header and the JSON chunk.
- `Buffers`: a primitive's streams and indices as accessors in one buffer, each view 4-byte aligned, in the order
  they are added. Colours are float or normalized unsigned; indices are unsigned short while the vertex count
  allows, else unsigned int. Skin streams are left out and named.
- `encode_mesh`: one mesh, without materials, as a GLB of its own. Its node sits at the origin under a group node,
  and the node's extras show the mesh's `core.mesh` view. That is what the glTF writer (`formats/gltf`) writes for
  an unplaced, material-less prototype. A test there checks
  that the two agree apart from the generator.

## Example

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = primase_glb::assemble(br#"{"asset":{"version":"2.0"}}"#, &[])?;
    let chunks = primase_glb::parse(&bytes)?;
    assert!(chunks.bin.is_none());
    println!("{} JSON bytes", chunks.json.len());
    Ok(())
}
```

This example needs no game files.
