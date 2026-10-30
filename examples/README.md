# primase-examples

Programs composing primase's readers, passes, writers and later compile stages. This package has no library API;
recipes and quality choices are visible in each program. Build the examples from the workspace root:

```sh
cargo build -p primase-examples --examples
```

[sekiro_to_reskate](examples/sekiro_to_reskate.rs) is the complete example quoted in the root README. It needs
a Sekiro install and archive keys, the game's Oodle library, ReSkate Studio and a skate. install at the paths the
program states. After setting those paths, run:

```sh
cargo run --release -p primase-examples --example sekiro_to_reskate
```

[sekiro_ktx2_tiles](examples/sekiro_ktx2_tiles.rs) additionally needs `--features ktx2` for the web writer's texture
encoder. The example sources document their inputs and output paths.

License: GPL-3.0-or-later.
