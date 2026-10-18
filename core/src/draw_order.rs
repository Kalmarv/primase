//! The order in which surfaces that overlap are drawn, as the source states it (`core.draw_order`, on material
//! entities).

use serde::Serialize;
use serde_json::Value;

use crate::pbr::{AlphaMode, PbrMaterial};
use crate::{Component, Entity, JsonError, World};

/// What a surface is, for the draw order: it drives depth bias, shadows and a target's material class, not the
/// sequence (that is [`DrawOrder::queue`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawLayer {
    Opaque,
    /// Alpha tested.
    Cutout,
    /// Drawn over the surface beneath it, coplanar or nearly so (a game's decal bucket): a consumer biases its depth
    /// and does not cast its shadow.
    Decal,
    /// Blended.
    Transparent,
}

/// `core.draw_order`: where a material's surfaces come in the source's draw sequence. `queue` is the source's own
/// sequence across every layer, lower first, on the conventional render-queue scale (opaque 2000, alpha tested
/// 2450, transparent 3000; a source's own values where it has them, as Unity's render queue). Within one queue,
/// surfaces draw in entity order, then primitive order: the source order readers keep.
///
/// `depth_write`: whether the source draws the surface writing depth (a blended surface may: Unity's ZWrite on in
/// the opaque queues), so a consumer orders and composites it as the source does.
///
/// A material without it draws by its alpha mode ([`DrawOrder::of`]), and is never a decal: that is a fact the
/// source states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DrawOrder {
    pub layer: DrawLayer,
    pub queue: i32,
    pub depth_write: bool,
}

impl DrawOrder {
    /// The conventional queues of the layers a material's alpha mode implies.
    pub const OPAQUE: i32 = 2000;
    pub const CUTOUT: i32 = 2450;
    pub const TRANSPARENT: i32 = 3000;

    /// A draw order with the conventional depth write of its layer (glTF's): opaque and alpha-tested surfaces write
    /// depth, decals and blended ones do not. For a source that states the layer and queue but not the depth write.
    pub fn conventional(layer: DrawLayer, queue: i32) -> DrawOrder {
        DrawOrder {
            layer,
            queue,
            depth_write: matches!(layer, DrawLayer::Opaque | DrawLayer::Cutout),
        }
    }

    /// Material `e`'s draw order: its `core.draw_order`, else what its `pbr.material`'s alpha mode implies (opaque
    /// without one), conventionally ([`DrawOrder::conventional`]).
    pub fn of(world: &World, e: Entity) -> DrawOrder {
        if let Some(&d) = world.get::<DrawOrder>(e) {
            return d;
        }
        match world.get::<PbrMaterial>(e).map(|m| m.alpha_mode) {
            Some(AlphaMode::Mask { .. }) => Self::conventional(DrawLayer::Cutout, Self::CUTOUT),
            Some(AlphaMode::Blend) => Self::conventional(DrawLayer::Transparent, Self::TRANSPARENT),
            Some(AlphaMode::Opaque) | None => Self::conventional(DrawLayer::Opaque, Self::OPAQUE),
        }
    }
}

impl Component for DrawOrder {
    const NAME: &'static str = "core.draw_order";
    /// 2: `depth_write`.
    const VERSION: u32 = 2;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_material_without_one_draws_by_its_alpha_mode_and_is_never_a_decal() {
        let mut w = World::new();
        let blend = w.spawn();
        w.insert(
            blend,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                ..PbrMaterial::default()
            },
        );
        let mask = w.spawn();
        w.insert(
            mask,
            PbrMaterial {
                alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
                ..PbrMaterial::default()
            },
        );
        let bare = w.spawn();
        let decal = w.spawn();
        // a decal the source states writes depth (the convention would not)
        w.insert(
            decal,
            DrawOrder {
                layer: DrawLayer::Decal,
                queue: 2500,
                depth_write: true,
            },
        );
        // without one: the alpha mode's layer and queue, and glTF's depth write
        assert_eq!(
            DrawOrder::of(&w, blend),
            DrawOrder::conventional(DrawLayer::Transparent, 3000)
        );
        assert!(!DrawOrder::of(&w, blend).depth_write);
        assert_eq!(DrawOrder::of(&w, mask).queue, 2450);
        assert!(DrawOrder::of(&w, mask).depth_write);
        assert_eq!(DrawOrder::of(&w, bare).layer, DrawLayer::Opaque);
        assert!(DrawOrder::of(&w, bare).depth_write);
        assert!(!DrawOrder::conventional(DrawLayer::Decal, 2500).depth_write);
        assert_eq!(
            DrawOrder::of(&w, decal),
            DrawOrder {
                layer: DrawLayer::Decal,
                queue: 2500,
                depth_write: true
            }
        );
        assert_eq!(
            w.get::<DrawOrder>(decal).unwrap().to_json().unwrap(),
            Some(serde_json::json!({"layer": "decal", "queue": 2500, "depth_write": true}))
        );
    }
}
