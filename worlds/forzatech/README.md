# primase-forzatech

Forza Horizon 6 (ForzaTech) over the format readers of `forzatech-formats`: the placements of one track (Tokyo is
`media/Tracks/Brio`) in an x/z box and one season, hand-placed and from the template libraries, with their models,
their material chains (instance, material and shader, through the track's `bin.zip` and its asset manifest) and the
textures those name, from the track's GeoChunks and the install's texture library. The reader keeps every LOD and
draw-distance band as stored; passes select what is drawn at a distance and from the main view, and build collision
from the drawn triangles.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_forzatech::register(&mut passes);
    println!("{}", passes.describe("select-forzatech-drawn").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup (`installs.fh6`); the inputs the tests read
are in [tests/inputs](tests/inputs/README.md).

License: GPL-3.0-or-later.
