# primase-peak

PEAK's script-provided shader globals over the Unity reader: `Peak.FogShaderParams`' fog texture, the light
volume assembled as `LightVolume.Decompress` does, and the values their scripts hand every shader. The reader
records which enabled scripts run on active objects. Values the stored scene cannot decide stay explicitly
unset; wind is never set on initial load and `_WeatherBlend` starts at zero.

This demo needs a type tree package and the player's data folder with the named scene available. Read the Unity
level into the same world before adding PEAK's globals; this entry point augments those existing records.

## Example

```rust
use primase_core::World;
use unity_formats::{TypeTreePackage, player::Player};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let package = TypeTreePackage::parse(&std::fs::read("typetrees.tpk")?)?;
    let player = Player::open(Path::new("PEAK_Data"), package)?;
    let mut world = World::new();
    primase_unity::level::read_level(&player, "level4", &mut world)?;
    let report = primase_peak::read_globals(&player, "level4", &mut world)?;
    println!("{} active fog scripts", report.fog_shader_params);
    Ok(())
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
