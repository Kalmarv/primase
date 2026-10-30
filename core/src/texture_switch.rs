//! Which form of its textures a placement draws, by distance ([`TextureSwitch`], `core.texture_switch`), and the view
//! and state the distances are evaluated in ([`TextureSwitchView`], `core.texture_switch_view`).
//!
//! The textures' forms are [`crate::TextureForms`] (level 0 the texture's own, level 1 its coarser form). A source that
//! switches between them per placement while it draws states the rule here, and a renderer applies it every frame:
//! [`TextureSwitch::next`] from the placement's [`box_distance`] scaled by [`TextureSwitchView::factor`], in the
//! view's [`TextureSwitchState`]. A writer, which cannot switch per frame, keeps one form with `select-texture-form`.

use serde::Serialize;
use serde_json::Value;

use crate::{Component, Entity, Instance, JsonError, Resource, Transform, World};

/// The level a placement's textures start at, before the first evaluation: the coarser form.
pub const TEXTURE_SWITCH_START: u8 = 1;

/// When a placement draws its textures' level-1 forms (`core.texture_switch`), on the placement (an entity with
/// `core.instance`). Distances are in the world's units, measured from the camera to the placement's world box
/// ([`box_distance`]) and scaled by [`TextureSwitchView::factor`]. The level starts at [`TEXTURE_SWITCH_START`] and
/// moves with hysteresis: from 0 to 1 at a distance of `(border + play) * scale * load` or more, from 1 to 0 below
/// `(border - play) * scale * load * ramp`, otherwise it stays (`load` and `ramp` the view's [`TextureSwitchState`]).
///
/// The level picks the form of every texture the placement's prototype binds that has `core.texture_forms`: level 0
/// the texture's own, level 1 its form at detail 1. A binding the source draws coarse whatever the level binds that form
/// itself (a texture with no forms), so it does not follow. A placement without the component draws level 0.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TextureSwitch {
    /// The distance the level changes at, positive.
    pub border: f32,
    /// The margin either side of `border` (hysteresis), not negative. It may reach `border`: a placement then never
    /// returns to level 0 once it left it.
    pub play: f32,
    /// A factor on both thresholds, positive (a source's per-kind scale of its draw distances).
    pub scale: f32,
}

impl TextureSwitch {
    /// The level after one evaluation at scaled distance `distance` in `state`, from level `current` (0 or 1; any
    /// other level is taken as 1).
    pub fn next(&self, current: u8, distance: f32, state: TextureSwitchState) -> u8 {
        let threshold = if current == 0 {
            (self.border + self.play) * self.scale * state.load
        } else {
            (self.border - self.play) * self.scale * state.load * state.ramp
        };
        u8::from(distance >= threshold)
    }
}

fn positive(value: f32, what: &str) -> Result<(), String> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(format!("{what} {value} is not a finite positive number"))
    }
}

impl Component for TextureSwitch {
    const NAME: &'static str = "core.texture_switch";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// World distances of the placement, not local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// On a placement; `border` and `scale` finite and positive, `play` finite and not negative.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Instance>(e) {
            return Err("a texture switch on an entity that is not a placement (no core.instance)".into());
        }
        positive(self.border, "border")?;
        positive(self.scale, "scale")?;
        if !(self.play.is_finite() && self.play >= 0.0) {
            return Err(format!("play {} is not a finite number of 0 or more", self.play));
        }
        Ok(())
    }
}

/// The view the distances of every [`TextureSwitch`] are stated for, and the state they are evaluated in
/// (`core.texture_switch_view`, a world resource): a camera of vertical field of view `reference_fov_y` measures them
/// as they are, a narrower one as nearer; with a `reference_height`, a view taller than it as nearer too. A world with
/// texture switches has it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TextureSwitchView {
    /// Radians, more than 0 and less than pi.
    pub reference_fov_y: f32,
    /// The view height in pixels the distances are stated for; 0 is none: the view's height takes no part.
    pub reference_height: u32,
    /// The factors on the thresholds the source states the switch in.
    pub state: TextureSwitchState,
}

/// The source's factors on a [`TextureSwitch`]'s thresholds, which its own runtime may move: the state a renderer
/// evaluates the switch in, stated by the source with what it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TextureSwitchState {
    /// On both thresholds, from 0 to 1.
    pub load: f32,
    /// On the return threshold (level 1 to 0) only, from 0 to 1.
    pub ramp: f32,
}

impl TextureSwitchState {
    /// Neither factor moves a threshold.
    pub const SETTLED: TextureSwitchState = TextureSwitchState { load: 1.0, ramp: 1.0 };
}

impl TextureSwitchView {
    /// What a camera of vertical field of view `fov_y` (radians) drawing a view `height` pixels tall multiplies its
    /// distances by before comparing them to the thresholds: `tan(fov_y / 2) / tan(reference_fov_y / 2)`, divided by
    /// `height / reference_height` where `reference_height` is not 0.
    pub fn factor(&self, fov_y: f32, height: u32) -> f32 {
        let fov = (fov_y / 2.0).tan() / (self.reference_fov_y / 2.0).tan();
        if self.reference_height == 0 {
            fov
        } else {
            fov / (height as f32 / self.reference_height as f32)
        }
    }
}

impl Resource for TextureSwitchView {
    const NAME: &'static str = "core.texture_switch_view";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// A field of view strictly between 0 and pi; each factor of the state from 0 to 1.
    fn verify(&self, _w: &World) -> Result<(), String> {
        let f = self.reference_fov_y;
        if !(f.is_finite() && f > 0.0 && f < std::f32::consts::PI) {
            return Err(format!(
                "reference vertical field of view {f} is not between 0 and pi radians"
            ));
        }
        for (what, v) in [("load", self.state.load), ("ramp", self.state.ramp)] {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("state {what} {v} is not from 0 to 1"));
            }
        }
        Ok(())
    }
}

/// The world-space axis-aligned box of the local boxes `boxes` (min, max) placed by `t`: the union of the eight
/// transformed corners of each. `None` when `boxes` is empty.
pub fn world_box(
    boxes: impl IntoIterator<Item = ([f32; 3], [f32; 3])>,
    t: &Transform,
) -> Option<([f32; 3], [f32; 3])> {
    let mut out: Option<([f32; 3], [f32; 3])> = None;
    for (lo, hi) in boxes {
        for corner in 0..8 {
            let local = [
                if corner & 1 == 0 { lo[0] } else { hi[0] },
                if corner & 2 == 0 { lo[1] } else { hi[1] },
                if corner & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let p: [f32; 3] = std::array::from_fn(|k| {
                t.cols[0][k] * local[0] + t.cols[1][k] * local[1] + t.cols[2][k] * local[2] + t.cols[3][k]
            });
            let (min, max) = out.get_or_insert((p, p));
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    out
}

/// The distance from `point` to the axis-aligned box `min`..`max`: to its nearest point, 0 inside.
pub fn box_distance(point: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    (0..3)
        .map(|k| {
            let d = point[k] - point[k].clamp(min[k], max[k]);
            d * d
        })
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWITCH: TextureSwitch = TextureSwitch {
        border: 30.0,
        play: 5.0,
        scale: 1.0,
    };

    const SETTLED: TextureSwitchState = TextureSwitchState::SETTLED;

    /// Level 0 leaves at border + play, level 1 returns below border - play; between them each stays.
    #[test]
    fn the_level_moves_with_hysteresis() {
        assert_eq!(SWITCH.next(0, 34.9, SETTLED), 0);
        assert_eq!(SWITCH.next(0, 35.0, SETTLED), 1);
        assert_eq!(SWITCH.next(1, 25.0, SETTLED), 1);
        assert_eq!(SWITCH.next(1, 24.9, SETTLED), 0);
        for d in [25.0, 30.0, 34.9] {
            assert_eq!(SWITCH.next(0, d, SETTLED), 0, "{d}");
            assert_eq!(SWITCH.next(1, d, SETTLED), 1, "{d}");
        }
        // a placement starts coarse and comes back only inside border - play
        assert_eq!(SWITCH.next(TEXTURE_SWITCH_START, 30.0, SETTLED), 1);
        // the scale moves both thresholds
        let far = TextureSwitch { scale: 2.0, ..SWITCH };
        assert_eq!((far.next(0, 69.9, SETTLED), far.next(0, 70.0, SETTLED)), (0, 1));
        assert_eq!((far.next(1, 50.0, SETTLED), far.next(1, 49.9, SETTLED)), (1, 0));
    }

    /// The state's load moves both thresholds, its ramp the return one alone; a ramp of 0 never returns.
    #[test]
    fn the_state_moves_the_thresholds() {
        let half = TextureSwitchState { load: 0.5, ramp: 1.0 };
        assert_eq!((SWITCH.next(0, 17.4, half), SWITCH.next(0, 17.5, half)), (0, 1));
        assert_eq!((SWITCH.next(1, 12.5, half), SWITCH.next(1, 12.4, half)), (1, 0));
        let ramping = TextureSwitchState { load: 1.0, ramp: 0.5 };
        assert_eq!(
            (SWITCH.next(0, 34.9, ramping), SWITCH.next(0, 35.0, ramping)),
            (0, 1)
        );
        assert_eq!(
            (SWITCH.next(1, 12.5, ramping), SWITCH.next(1, 12.4, ramping)),
            (1, 0)
        );
        let off = TextureSwitchState { load: 1.0, ramp: 0.0 };
        assert_eq!(SWITCH.next(1, 0.0, off), 1);
    }

    /// The distance is to the nearest point of the box, 0 inside; the box is the union of every transformed corner.
    #[test]
    fn the_distance_is_to_the_world_box() {
        let (lo, hi) = ([-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]);
        assert_eq!(box_distance([0.0, 1.0, 0.0], lo, hi), 0.0);
        assert_eq!(box_distance([4.0, 1.0, 0.0], lo, hi), 3.0);
        assert_eq!(box_distance([4.0, 6.0, 0.0], lo, hi), 5.0);
        // a quarter turn about y, then moved 10 along x: the box keeps its size, rotated
        let t = Transform {
            cols: [
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
            ],
        };
        let boxes = [
            ([0.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            ([0.0, 0.0, 0.0], [1.0, 3.0, 1.0]),
        ];
        assert_eq!(world_box(boxes, &t), Some(([10.0, 0.0, -2.0], [11.0, 3.0, 0.0])));
        assert_eq!(world_box([], &t), None);
    }

    /// The view's factor is 1 at its own field of view and grows with a wider one; Elden Ring's reference (43 deg)
    /// against a 48 deg camera is the 1.130 of its captured frames. Without a reference height the view's height takes
    /// no part; with one, a view twice as tall halves the factor.
    #[test]
    fn a_wider_camera_measures_farther() {
        let view = TextureSwitchView {
            reference_fov_y: 43f32.to_radians(),
            reference_height: 0,
            state: SETTLED,
        };
        assert!((view.factor(43f32.to_radians(), 1440) - 1.0).abs() < 1e-6);
        assert!((view.factor(48f32.to_radians(), 1440) - 1.130).abs() < 5e-4);
        assert_eq!(
            view.factor(48f32.to_radians(), 720),
            view.factor(48f32.to_radians(), 1440)
        );
        let tall = TextureSwitchView {
            reference_height: 720,
            ..view
        };
        assert!((tall.factor(43f32.to_radians(), 1440) - 0.5).abs() < 1e-6);
        assert!((tall.factor(43f32.to_radians(), 720) - 1.0).abs() < 1e-6);
        let w = World::new();
        assert_eq!(view.verify(&w), Ok(()));
        for bad in [0.0, -1.0, std::f32::consts::PI, f32::NAN] {
            let bad_view = TextureSwitchView {
                reference_fov_y: bad,
                ..view
            };
            assert!(bad_view.verify(&w).is_err(), "{bad}");
        }
        for state in [
            TextureSwitchState { load: 1.5, ramp: 1.0 },
            TextureSwitchState {
                load: 1.0,
                ramp: -0.1,
            },
            TextureSwitchState {
                load: f32::NAN,
                ramp: 1.0,
            },
        ] {
            assert!(
                TextureSwitchView { state, ..view }.verify(&w).is_err(),
                "{state:?}"
            );
        }
    }

    /// The component sits on a placement and holds finite, positive thresholds.
    #[test]
    fn a_switch_is_on_a_placement_with_positive_thresholds() {
        let mut w = World::new();
        let prototype = w.spawn();
        let placed = w.spawn();
        w.insert(
            placed,
            Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
            },
        );
        w.insert(
            placed,
            Instance {
                prototype,
                variant: 0,
            },
        );
        assert_eq!(SWITCH.verify(placed, &w), Ok(()));
        assert!(SWITCH.verify(prototype, &w).is_err());
        for bad in [
            TextureSwitch {
                border: 0.0,
                ..SWITCH
            },
            TextureSwitch { scale: 0.0, ..SWITCH },
            TextureSwitch { play: -1.0, ..SWITCH },
            TextureSwitch {
                border: f32::NAN,
                ..SWITCH
            },
        ] {
            assert!(bad.verify(placed, &w).is_err(), "{bad:?}");
        }
        // play at or beyond the border is allowed: such a placement never returns to its own textures
        assert_eq!(TextureSwitch { play: 40.0, ..SWITCH }.verify(placed, &w), Ok(()));
    }
}
