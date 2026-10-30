# primase-reskate

The skate. destination: `skate.*` components, preparation passes, skate-scene interchange and the writer of
normalized map folders that ReSkate Studio compiles. The consumer chooses preparation, budgets, spawn, water,
collision and compile settings. `compile` and `postcompile` expose the later stages separately; the map writer
encodes the prepared world and reports what it leaves out. Costume and assembly support live beside map support.

## Example

```rust
use primase_core::pass::PassRegistry;

fn main() {
    let mut passes = PassRegistry::new();
    primase_reskate::register(&mut passes);
    println!("{}", passes.describe("raise-skate").expect("registered pass"));
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
