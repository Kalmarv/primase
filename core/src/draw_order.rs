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
/// A material without it draws by its alpha mode ([`DrawOrder::of`]), and is never a decal: that is a fact the
/// source states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DrawOrder {
    pub layer: DrawLayer,
    pub queue: i32,
}

impl DrawOrder {
    /// The conventional queues of the layers a material's alpha mode implies.
    pub const OPAQUE: i32 = 2000;
    pub const CUTOUT: i32 = 2450;
    pub const TRANSPARENT: i32 = 3000;

    /// Material `e`'s draw order: its `core.draw_order`, else what its `pbr.material`'s alpha mode implies (opaque
    /// without one).
    pub fn of(world: &World, e: Entity) -> DrawOrder {
        if let Some(&d) = world.get::<DrawOrder>(e) {
            return d;
        }
        match world.get::<PbrMaterial>(e).map(|m| m.alpha_mode) {
            Some(AlphaMode::Mask { .. }) => DrawOrder {
                layer: DrawLayer::Cutout,
                queue: Self::CUTOUT,
            },
            Some(AlphaMode::Blend) => DrawOrder {
                layer: DrawLayer::Transparent,
                queue: Self::TRANSPARENT,
            },
            Some(AlphaMode::Opaque) | None => DrawOrder {
                layer: DrawLayer::Opaque,
                queue: Self::OPAQUE,
            },
        }
    }
}

impl Component for DrawOrder {
    const NAME: &'static str = "core.draw_order";
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
        w.insert(
            decal,
            DrawOrder {
                layer: DrawLayer::Decal,
                queue: 2500,
            },
        );
        assert_eq!(DrawOrder::of(&w, blend).layer, DrawLayer::Transparent);
        assert_eq!(DrawOrder::of(&w, mask).queue, 2450);
        assert_eq!(DrawOrder::of(&w, bare).layer, DrawLayer::Opaque);
        assert_eq!(
            DrawOrder::of(&w, decal),
            DrawOrder {
                layer: DrawLayer::Decal,
                queue: 2500
            }
        );
        assert_eq!(
            w.get::<DrawOrder>(decal).unwrap().to_json().unwrap(),
            Some(serde_json::json!({"layer": "decal", "queue": 2500}))
        );
    }
}
