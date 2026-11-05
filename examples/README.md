# primase-examples

Programs composing primase's readers, passes, writers and later compile stages. This package has no library API;
recipes and quality choices are visible in each program. Build the examples from the workspace root:

```sh
cargo build -p primase-examples --examples
```

[sekiro_to_reskate](examples/sekiro_to_reskate.rs) is a complete conversion, from game files to a compiled map. It needs
a Sekiro install and archive keys, the game's Oodle library, ReSkate Studio and a skate. install at the paths the
program states. After setting those paths, run:

```sh
cargo run --release -p primase-examples --example sekiro_to_reskate
```

The example sources document their inputs and output paths.

[elden_outfit_gltf](examples/elden_outfit_gltf.rs) and [elden_stormveil_gltf](examples/elden_stormveil_gltf.rs) write
glTF binaries from an Elden Ring install: the player's starting outfit (initializer row 3002, the female
presentation) and a slice of Stormveil Castle at a stated hour. Each states its choices (the outfit row; the region,
the environment file and the hour) as constants, reads, runs registered passes and the glTF writer, and prints the
passes' reports and what the writer left out, and writes what it ran beside the output (`<output>.preparation.txt`):
its `[selection]`, `[read]`, `[lowering]` and `[writer]` stages, each stage's texts one per line as each states itself,
for a program that reads and prepares the same asset the same way. They find the
install and the archive keys through the `GAME_DATA` configuration (`elden`, declared in `tests/inputs/mod.rs`) and
take the output file as their argument:

```sh
cargo run --release -p primase-examples --example elden_outfit_gltf -- outfit.glb
cargo run --release -p primase-examples --example elden_stormveil_gltf -- stormveil.glb
```

License: GPL-3.0-or-later.
