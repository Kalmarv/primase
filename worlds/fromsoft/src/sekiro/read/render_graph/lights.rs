//! The engine's light records as `core` light tables ([`BufferSource::Lights`](primase_core::shader::BufferSource)):
//! Sekiro's `PointLight` (208 bytes) and `SpotLight` (192 bytes), the structured buffers its light-organize,
//! tile-lighting, per-light and forward programs read, each field by what the programs' code does with it (fxc's
//! disassembly of every program of the install that declares them: 79 renderer and effect programs, 852 forward
//! materials). A byte no program reads (`SpecColor.w`, `TransLightParam.z`, the pads, the spot's `ShadingMode`) has no
//! field. Elden Ring declares the same two structures, but what its programs read of them is not yet read: its buffers
//! stay unknown.

use fromsoft_formats::Game;
use primase_core::shader::{LightField, LightTable, LightTableKind, LightValue};

/// Records each buffer holds: the frames bind both with 1024 elements.
const CAPACITY: u32 = 1024;

const WRAP: &str = "1 / a size of the light in the engine's own model: the tile-lighting and forward programs soften \
    the diffuse wrap and widen the specular lobe by the distance times it; the recorded records of bank lights hold 1 / \
    the bank's word at offset 164; core.light's radius is not established to be that size";
const TRANSLUCENCY: &str = "the translucency term's strength (x) and exponent over 32 (y), which the tile-lighting pass \
    and the translucent (`Tr`) forward materials add to a light's diffuse; core.light has no translucency";
const LIGHT_TYPE: &str = "0 in every record of the recorded frames; the organize program marks a light for the \
    tile-lighting pass only when it is 0";
const SHADOW: &str = "a shadowed light's place in the shadow atlas, which only the shadowed (`Sdw`) program variants \
    read; the recorded frames hold no shadowed light (the fields are 0)";

fn field(offset: u32, value: LightValue) -> LightField {
    LightField { offset, value }
}

fn unknown(floats: u8, name: &str, why: &str) -> LightValue {
    LightValue::Unknown {
        floats,
        name: name.into(),
        why: why.into(),
    }
}

/// The light table a program's structured buffer `name` of `stride`-byte elements is, in `game`; `None` for any other
/// buffer, and for every Elden Ring buffer.
pub(super) fn table(game: Game, name: &str, stride: u32) -> Option<LightTable> {
    if game != Game::Sekiro {
        return None;
    }
    let light_type = || LightValue::Constant {
        value: 0.0,
        why: LIGHT_TYPE.into(),
    };
    match (name, stride) {
        ("g_LightBuffer", 208) => Some(LightTable {
            kind: LightTableKind::Point,
            stride,
            capacity: CAPACITY,
            fields: vec![
                field(0, LightValue::ViewPosition),
                field(12, LightValue::InverseRangeSquared),
                field(16, LightValue::Colour),
                field(28, unknown(1, "DiffColor.w", WRAP)),
                field(32, LightValue::Colour),
                field(48, unknown(2, "TransLightParam.xy", TRANSLUCENCY)),
                field(60, LightValue::Range),
                field(64, light_type()),
                field(80, unknown(4, "ProjParam", SHADOW)),
                field(96, unknown(4, "ShadowCenterPos", SHADOW)),
                field(112, unknown(24, "ViewportScaleOffset", SHADOW)),
            ],
        }),
        ("g_SpotLightBuffer", 192) => Some(LightTable {
            kind: LightTableKind::Spot,
            stride,
            capacity: CAPACITY,
            fields: vec![
                field(0, LightValue::ViewPosition),
                field(12, LightValue::Range),
                field(16, LightValue::Colour),
                field(28, unknown(1, "DiffColor.w", WRAP)),
                field(32, LightValue::Colour),
                field(48, unknown(2, "TransLightParam.xy", TRANSLUCENCY)),
                field(64, LightValue::ViewToCone),
                field(128, light_type()),
                field(132, LightValue::ViewDepthMin),
                field(136, LightValue::ViewDepthMax),
                field(144, LightValue::ViewBoundingSphere),
                field(160, unknown(1, "ShadowParam.x", SHADOW)),
                field(176, unknown(4, "ViewportScaleOffset", SHADOW)),
            ],
        }),
        _ => None,
    }
}

/// The light table a constant `name` counts the records of, in `game` (`cbLightAccParam`'s `g_lightCount` and
/// `g_spotLightCount`, the bounds of the organize program's loops over the two buffers); `None` otherwise, and for every
/// Elden Ring constant.
pub(super) fn count(game: Game, name: &str) -> Option<LightTableKind> {
    match (game, name) {
        (Game::Sekiro, "g_lightCount") => Some(LightTableKind::Point),
        (Game::Sekiro, "g_spotLightCount") => Some(LightTableKind::Spot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tables agree with themselves and their buffers' element sizes; Elden Ring has none yet.
    #[test]
    fn sekiros_light_tables_agree_with_their_buffers() {
        for (name, stride) in [("g_LightBuffer", 208), ("g_SpotLightBuffer", 192)] {
            let t = table(Game::Sekiro, name, stride).expect(name);
            assert_eq!(t.verify(Some(stride)), Ok(()), "{name}");
            assert!(table(Game::EldenRing, name, stride).is_none());
            assert!(table(Game::Sekiro, name, stride + 16).is_none());
        }
        assert_eq!(count(Game::Sekiro, "g_lightCount"), Some(LightTableKind::Point));
        assert_eq!(count(Game::EldenRing, "g_lightCount"), None);
    }
}
