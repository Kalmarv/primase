# primase-frostbite

Frostbite containers and documents, as the game and its tooling use them: superbundle TOCs, bundle regions and
manifests, the native DB, data folders and CAS archives, shader block depots, shader lookup tables, and EBX documents
(read into typed values, written again, and carried between documents).

## EBX gates

`tests/ebx.rs` has three gates:

- **Synthetic baselines** (`tests/fixtures/ebx/*.ebx`): documents laid out from the writer's own layout rules, read to
  hand-worked values and written back byte for byte. They cover structure and detect regressions; they are not
  independent correctness evidence.
- **Carry**: values moved from one document into another keep their instances, imports and arrays through a rewrite.
- **Real data** (the real-data configuration, CONTRIBUTING.md), with three inputs:
  - the corpus oracle set's `skate/ebx`: a folder of stock documents extracted from the game's `Data`, mirroring their asset
    paths (for example `characters/customization/configs/cas_main_bundlereftable.ebx`). Every document must keep
    every field's value through read -> write -> read.
  - `tools.reskate_cli` and `installs.skate`: the reference reader and writer, and the game root it reads (the
    install, whose `Data` folder holds the archives). The writer's byte layout is compared with the reference's own
    rewrite of each document: where the reference reproduces the stock bytes, ours must too; documents where the
    reference itself differs from stock are counted and listed by name. This comparison measures agreement with the
    port's source; value preservation on stock data is the independent check.
  - One stock document's fields are spot-checked against the reference reader's `ebx-values` decode.

The synthetic baselines and the real-data census are separate tests; the census skips when its inputs are unset.

### Extracting the corpus

Use the reference reader's own extraction command, which writes the asset's stored bytes:

```text
reskate_cli ebx-raw <game root> <asset name> <folder>/<asset name>.ebx
```

Take a spread of documents (a level, an object, a bundle reference table). The spot check uses
`characters/customization/configs/cas_main_bundlereftable` when it is present; its expected values were read from
`reskate_cli ebx-values <game root> characters/customization/configs/cas_main_bundlereftable`.
