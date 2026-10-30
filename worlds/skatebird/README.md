# primase-skatebird

SkateBIRD over `primase-unity`: reset points, mission givers, scripted props and grind curves, including
source activation state. `SkatebirdReader` composes the Unity level read with the game-specific reads. Passes
place markers, omit mission props and bake the compiled Bumped Specular Voronoi shader after Unity material
lowering; their choices are the program's.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_skatebird::register(&mut passes);
    println!("{}", passes.describe("place-skatebird-markers").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
