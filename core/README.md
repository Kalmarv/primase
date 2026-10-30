# primase-core

Shared meanings and the `World` store: geometry, textures, materials, lights, rigs, animation, state axes,
resource references and provenance. Readers attach components; passes declare what they require and change;
conversion checks the prepared world against the writer's target and retains notices in its report.

`World::clone` and `get_shared` share component values through `Arc` until a write. `core.texture_forms` describes
alternative texture images separately from geometry LODs; `SelectTextureForm` makes the caller's choice.
`IrradianceVolume` describes spatial first-order irradiance. `Atmosphere` (resource version 3) holds fog, fixed
exposure or `AutoExposure`, including its `ExposureMeter`, bounds and temporal adaptation; source-unit volumetric
fog stays descriptive until a lowering establishes its units.

## Example

```rust
use primase_core::{Name, World};

fn main() {
    let mut world = World::new();
    let object = world.spawn();
    world.insert(object, Name("example".into()));
    let copy = world.clone();
    assert_eq!(copy.get::<Name>(object).unwrap().0, "example");
}
```

See [CONTRIBUTING](../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
