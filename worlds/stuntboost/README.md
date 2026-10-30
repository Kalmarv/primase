# primase-stuntboost

STUNTBOOST over MonoGame: levels and their room/sky parts, models, baked atlases, materials, nodes and spawns.
`read_level` requires an explicit metres-per-unit scale: props are authored in centimetres while the board is
a fingerboard. Passes lower collision, spawns, materials and shaders and omit hidden nodes as the caller chooses.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_stuntboost::register(&mut passes);
    println!("{}", passes.describe("lower-stuntboost-materials").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
