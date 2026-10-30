# primase-anvil

Riders Republic data read through `anvil-formats` into namespaced components, then lowered by explicitly
selected passes: geometry, materials and layers, collision, lights, terrain and shader bindings. The source
reader retains supported authored information; a consumer selects regions, states and destination preparation.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_anvil::passes::register(&mut passes);
    println!("{}", passes.describe("lower-anvil-material").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
