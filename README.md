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

A consumer builds the passes it chooses through their typed constructors and supplies its reader, writer and
explicit pipeline. This small sketch chooses to turn curves into polylines with a 1 cm tolerance; a real route also
selects its source and destination preparation passes.

```rust
use primase_core::convert::{convert, ConvertReport, Reader, Result, Writer};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_meshops::FlattenCurves;

fn export(reader: &mut dyn Reader, writer: Box<dyn Writer>) -> Result<ConvertReport> {
    let passes = Pipeline {
        passes: vec![Box::new(FlattenCurves::new(0.01)?)],
    };
    let select = Pipeline::default(); // No selection: retain all read content.
    // A registry names passes for pipelines written as text; a pipeline built from typed passes needs none.
    let manager = PassManager::new(PassRegistry::new());
    convert(reader, writer, &manager, &select, &passes)
}
```

Inspect the returned report's `notices()`; failures expose a partial report through `ConvertError`.
Use `convert_parts` for incremental loading with explicit partition planning, or compose library operations directly.

## From a Sekiro area to a skate. map

![PLACEHOLDER: an in-game screenshot of the Sekiro area as a skate. map](docs/sekiro-to-skate.png)

*Placeholder: the in-game screenshot comes with the whole-route proof.*

One program, four stages:

- **Read.** `AreaReader` reads an area of Sekiro as the game stores it: placements, models, materials and their
  layered shaders, lights, spawn points and collision. It decides nothing for a destination; what has no shared
  meaning stays in the `fromsoft` namespace.
- **Select.** Passes run once on the world as read and lower Sekiro's data to core meanings: collision by hit class,
  static meshes posed, the finest level of detail, layered materials baked to plain textures, materials, lights and
  spawn points.
- **Prepare.** Passes run on each part before it is written and prepare it for skate.: textures sampled at the
  albedo's tiling (the dialect has no texture transform), raised to the skate dialect, clipped to the region, the
  floor placed, the spawn chosen, water, the pause map, textures capped and tinted, decal overlays stacked, blend
  chains ordered and budgeted, the collision laid out.
- **Write.** `MapSink` encodes the prepared part as a map folder for ReSkate Studio. What it leaves out or
  approximates is a notice in the report; a value it cannot encode is an error. Compiling the folder into a playable
  level is ReSkate Studio's step.

`convert_parts` drives them: it reads, runs the selection, plans the parts (one here, since a ReSkate map is laid out
whole), loads each, runs its preparation, checks it against what the writer writes, and writes it. Every choice is a
typed value, an enum or a checked number, so a choice that is not valid fails while the pipeline is built, before
anything is read.

<!-- quoted from cli/examples/sekiro_to_reskate.rs: keep the two the same -->
```rust
/// Ashina Castle.
const AREA: &str = "m11_01";
/// The part of it converted: `[west, south, east, north]`, metres east and north in the world frame.
const REGION: [f32; 4] = [-170.0, 180.0, -50.0, 300.0];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [install, oodle, out] = args.as_slice() else {
        return Err("usage: sekiro_to_reskate <Sekiro install> <oo2core_6 DLL> <out root>".into());
    };
    let oodle = load_oodle(Path::new(oodle))?;

    // Read: the area as the game stores it, placements reaching within 30 m of the region.
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let options = primase_fromsoft::sekiro::Options {
        region: Some(REGION),
        region_margin: 30.0,
        ..Default::default()
    };
    let mut reader = AreaReader::new(install, AREA, decoder, options);

    // Write: one ReSkate map folder, `<out root>/ashina`.
    let encoding = EncodingOptions {
        out_root: out.into(),
        name: Some("ashina".into()),
        max_render_edge: Some(48.0),
        ..EncodingOptions::default()
    };
    let select = select()?;
    let passes = prepare_reskate(&encoding)?;
    println!("select: {select}\npasses: {passes}");
    let writer = Box::new(MapSink::new(Path::new(""), &encoding));

    // A ReSkate map is laid out whole: the selected area is one part.
    let manager = PassManager::new(PassRegistry::new());
    let report = convert_parts(
        &mut reader,
        &mut WholeWorld::default(),
        writer,
        &manager,
        &select,
        &passes,
    )?;
    for notice in report.notices() {
        println!("{notice}");
    }
    Ok(())
}

/// Run once on the area as read: Sekiro's own data lowered to core meanings.
fn select() -> Result<Pipeline> {
    // Collision within 50 m of the region: the ground parts (hit filters 8 and 27), HitMtrl classes 22 and 36 as
    // water, blockers (29) and 40 left out; objects leave out both, and bodies of motion type 2.
    let reach = Metres::new(50.0)?;
    let water = [HitClass::new(22)?, HitClass::new(36)?];
    let left_out = [HitClass::new(29)?, HitClass::new(40)?];
    let mut collision = LowerFromSoftCollision::new(HitFilters::new([8, 27])?).with_reach(reach);
    for c in water {
        collision = collision.with_class(c, ClassChoice::Water)?;
    }
    for c in left_out {
        collision = collision.with_class(c, ClassChoice::LeaveOut)?;
    }
    let objects = LowerFromSoftObjectCollision::new(ConvexBodies::Tessellate)
        .with_leave_out(water.into_iter().chain(left_out).collect())
        .with_leave_out_motion([2].into())
        .with_convex_surface(ConvexSurface::Render)
        .with_reach(reach);
    let geometry = LowerFromSoftGeometry::new(Pose::Rigid)
        .with_uv_tiling(Tiling::Apply)
        .with_winding(Winding::Normals)
        .with_degenerates(Degenerates::Drop)
        .with_leave_out([SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow].into())
        .with_blocker_cards(Cards::Record);
    let lights =
        LowerFromSoftLights::new(Factor::new(1.0)?, Fraction::new(0.15)?).with_min_range(Metres::new(0.5)?);
    Ok(Pipeline {
        passes: vec![
            Box::new(collision),
            Box::new(objects),
            Box::new(DropFromSoftFar),
            Box::new(ApplyRigidSkinning),
            Box::new(geometry),
            Box::new(SelectLod {
                choice: LodChoice::Finest,
            }),
            // layered materials baked to plain textures over the whole area, then every material lowered
            Box::new(BakeFromSoftLayers::new(PixelCap::new(512)?, Seconds::new(0.0)?)),
            Box::new(LowerFromSoftMaterials::new(None)),
            Box::new(lights),
            Box::new(LowerFromSoftExtras),
        ],
    })
}

/// Run on each part before it is written: the core world prepared as the ReSkate writer encodes it.
fn prepare_reskate(e: &EncodingOptions) -> Result<Pipeline> {
    let name = e.name.as_deref().unwrap_or("map");
    Ok(Pipeline {
        passes: vec![
            Box::new(NormalizeNormals),
            // the skate dialect has no texture transform: each texture sampled at the albedo's tiling
            Box::new(DropFromSoftTextureTransforms::new()),
            Box::new(RaiseSkate::new(RaiseOptions::new(
                name,
                LightCalibration::Standard,
            )?)),
            Box::new(ClipReskateRegion::new(REGION, 30.0)?),
            Box::new(PrepareReskateFloor::new(100.0, 250.0)?),
            Box::new(PrepareReskateGrinds::studio()),
            Box::new(PlaceReskateSpawn::centre()),
            Box::new(SnapReskateMarkers::for_encoding(e)?),
            Box::new(PrepareReskateWater::new(
                WaterExtent::Region { margin: 400.0 },
                OceanMaterials::Stated,
            )?),
            Box::new(PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 2160,
                extent: PauseExtent::Region,
            })?),
            Box::new(CapTextures::new(256)?.with_mode(CapMode {
                normals: NormalDownscale::Box,
                ..CapMode::default()
            })),
            Box::new(PrepareReskateTextures),
            Box::new(PrepareReskateUvRepeats::new(
                RepeatDecals::Omit,
                RepeatBlended::Cutout,
            )),
            Box::new(PrepareReskateOverlays::new(0.004)?.for_encoding(e)?),
            Box::new(PrepareReskateBlendOrder::new(200.0)?.for_encoding(e)?),
            Box::new(PrepareReskateCollision::new(
                CollisionSurfaces::Coalesce,
                CollisionWeld::Millimetre,
                e,
            )?),
            Box::new(SummarizeReskateMap::for_encoding(e)?),
            Box::new(PlanReskateBlendBudget::pool()),
        ],
    })
}
```

This is [`cli/examples/sekiro_to_reskate.rs`](cli/examples/sekiro_to_reskate.rs) without its imports and Oodle
loader; `cargo test --workspace` builds it. With the game installed:

```text
cargo run --release -p primase-cli --example sekiro_to_reskate -- <Sekiro install> <oo2core_6 DLL> <out root>
```

The screenshot is a complete program's output: these passes and the program's own, for what it chooses beyond the
library: ledges chained into grind rails, travel markers named for players with the spawn among them, collision
added where the game has none under a rendered surface, foliage drawn as plain surfaces, and markers snapped down to
the ground.

Status: the library is being aligned with this model; remaining boundary exceptions are tracked in
[`architecture.toml`](architecture.toml).

See [CONTRIBUTING](CONTRIBUTING.md) for rules, checks and real-data setup, and [DESIGN](docs/DESIGN.md) for detail.

[GPL-3.0-or-later](LICENSE)
