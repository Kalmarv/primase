# primase-frostbite

Frostbite in a world: the entries of a data folder (EBX documents, textures, shader block depots, program databases,
permutations, MeshSets, skeletons) read into components with `frostbite-formats`, the geometry decoded into core
meshes, and the passes that give the dialect core meanings. The containers' own readers and writers are in
`frostbite-formats` (`https://github.com/Kalmarv/frostbite-formats`); the EBX gates are described there.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_frostbite::passes::register(&mut passes);
    println!("{}", passes.describe("resolve-frostbite-shaders").expect("registered pass"));
}
```

This example needs no game files.
