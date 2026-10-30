# primase-monogame

Lifts a MonoGame effect's GLSL programs through `iridia-mojo`, supplying the effect tables' constant layouts,
sampler slots and vertex-attribute usages. XNB and MGFX parsing live in `monogame-formats`.
`programs::program` returns an iridia program for the requested shader; DirectX 11 effects are refused because
they carry DXBC rather than GLSL. The demo needs an MGFX effect's bytes from a DesktopGL game's content.

## Example

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("effect.mgfx")?;
    let effect = monogame_formats::mgfx::Effect::parse(&bytes)?;
    let program = primase_monogame::programs::program(&effect, 0)?;
    println!("{:?}", program.stage);
    Ok(())
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
