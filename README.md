# primase

A Rust library for reading, transforming and writing 3D game worlds. It brings supported source data into a shared
`World`, so adding a reader or writer expands the possible conversions without a separate exporter for every pair.

```text
read → World (shared + namespaced data) → selected passes → write → later stages
```

Readers read supported source information faithfully; they do not bake it for a destination. Passes select and
transform that information. Writers encode prepared data they support, report what they omit, and reject invalid
data that prevents valid output. Compilation, stitching and other later stages remain explicit operations.

## Architecture

| Layer | What belongs here |
| --- | --- |
| [`core`](core/) | Shared meanings: geometry, materials, lights, collision and world state; the `World` store and pass/conversion machinery. |
| [`toolkit`](toolkit/) | Reusable mechanisms such as compression, texture processing and mesh operations, independent of a particular game or destination. |
| [`formats`](formats/) | Interchange formats and their readers, passes and writers. |
| [`worlds`](worlds/) | Game, engine-family and destination knowledge: source readers, namespaced data and domain passes/writers. |
| [`cli`](cli/) | A small command line for trying readers, passes and writers together. |

A game crate may depend on its engine's crate, never the reverse. Each worlds crate names only its own game,
engine or destination. Reusable destination preparation belongs alongside that destination's writer, as separately
selected passes. Complete conversion programs live in consumer repositories and own their recipes and choices.

## What this means

- **You choose what gets produced.** Region, world state, quality, baking, collision generation and placement are
  program choices. Library operations can still have sensible, documented algorithm defaults.
- **One read can feed different destinations.** Supported information stays in core or a source namespace until
  selected passes transform it. Writers do not need to know which reader supplied it; coverage determines what can transfer.
- **Losses are visible.** Reports retain known approximations, exclusions and unsupported output, including per-part
  details. Preserving meaning matters; matching an old tool's encoded bytes is not a general requirement.
- **New code has a home.** Shared concepts go in core, neutral mechanisms in toolkit, domain knowledge in formats
  or worlds, and recipe choices in the consuming program. A standard shared concept need not wait for a second reader.

## Using the library

A consumer registers the operations it needs and supplies its reader, writer and explicit pipeline. This small
sketch chooses to turn curves into polylines with a 1 cm tolerance; a real route also selects its source and
destination preparation passes.

```rust
use primase_core::convert::{convert, ConvertReport, Reader, Result, Writer};
use primase_core::pass::{PassManager, PassOptions, PassRegistry, Pipeline};

fn export(reader: &mut dyn Reader, writer: Box<dyn Writer>) -> Result<ConvertReport> {
    let mut registry = PassRegistry::new();
    primase_meshops::register(&mut registry);

    let options = PassOptions::from([(
        "flatten-curves".into(),
        [("tolerance".into(), "0.01".into())].into(),
    )]);
    let passes = Pipeline {
        passes: vec![registry.make("flatten-curves", &options["flatten-curves"])?],
    };
    let select = Pipeline::default(); // No selection: retain all read content.
    let manager = PassManager::new(registry);
    convert(reader, writer, &manager, &select, &passes)
}
```

Inspect the returned report's `notices()`; failures expose a partial report through `ConvertError`.
Use `convert_parts` for incremental loading with explicit partition planning, or compose library operations directly.

Status: the library is being aligned with this model; remaining boundary exceptions are tracked in
[`architecture.toml`](architecture.toml).

See [CONTRIBUTING](CONTRIBUTING.md) for rules, checks and real-data setup, and [DESIGN](docs/DESIGN.md) for detail.

[GPL-3.0-or-later](LICENSE)
