//! Which level of detail a placement draws, by distance ([`LodSwitch`], `core.lod_switch`), and the view its distances
//! are stated for ([`LodSwitchView`], `core.lod_switch_view`).
//!
//! The levels are the prototype's [`crate::Lods`] (level `k` the form at detail `k`). A source that chooses among them
//! per placement while it draws states its rule here, and a renderer applies it every frame from the placement's
//! [`crate::box_distance`] scaled by [`LodSwitchView::factor`]. A writer, which cannot switch per frame, keeps one form
//! with `select-lod`.

use serde::Serialize;
use serde_json::Value;

use crate::{Component, Entity, Instance, JsonError, Resource, World};

/// One boundary between two neighbouring levels: level `k + 1` begins at `border` (times [`LodSwitch::scale`]), and
/// `play` is a band past it ([`LodBand`]), not scaled.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LodBoundary {
    pub border: f32,
    pub play: f32,
}

/// What a boundary's `play` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LodBand {
    /// The level follows the distance alone: level `k + 1` from `border * scale + play` on, the finer level fading into
    /// it from `border * scale` ([`LodSwitch::fade`]).
    CrossFade,
    /// The level moves with hysteresis from the one drawn before ([`LodSwitch::next`]): the boundary above it is crossed
    /// at `border * scale + play`, the one below it at `border * scale - play`, any other at `border * scale`.
    Hysteresis,
}

/// When a placement draws each level of its prototype's levels of detail (`core.lod_switch`), on the placement (an
/// entity with `core.instance`). Distances are in the world's units, from the camera to the placement's world box,
/// scaled by [`LodSwitchView::factor`]. `boundaries[k]` is the boundary between levels `k` and `k + 1`, in order (a
/// border that is never reached, such as a source's 9999 m, is stated as it is).
///
/// Elden Ring (eldenring.exe 1.17.3, read from the executable's code, not checked against frames of the running game):
/// the game evaluates the hysteresis rule from the level remembered between frames, and it remembers only that level's
/// low two bits, so a placement at level 4 or 5 is next evaluated from level 0 or 1; and from level 4 it compares the
/// first border unscaled. Both are the game's quirks, observed by decompilation, and [`LodSwitch::next`] does not
/// reproduce them: it states the rule the game's code applies at levels 0 to 3.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LodSwitch {
    pub boundaries: Vec<LodBoundary>,
    /// A factor on every border, positive (a source's per-kind scale of its draw distances).
    pub scale: f32,
    pub band: LodBand,
}

impl LodSwitch {
    /// The level at scaled distance `distance` with no level drawn before: the largest `k` whose boundary below it is
    /// passed, at `border * scale + play` with [`LodBand::CrossFade`] and at `border * scale` with
    /// [`LodBand::Hysteresis`] (the plain boundaries: a view with no history takes no side of a band).
    pub fn level_at(&self, distance: f32) -> u8 {
        let passed = |b: &LodBoundary| match self.band {
            LodBand::CrossFade => distance >= b.border * self.scale + b.play,
            LodBand::Hysteresis => distance >= b.border * self.scale,
        };
        level(self.boundaries.iter().take_while(|b| passed(b)).count())
    }

    /// The level after one evaluation at `distance` from level `current` drawn before: with [`LodBand::Hysteresis`] the
    /// boundary just above `current` is passed at `border * scale + play`, the one just below at `border * scale -
    /// play`, every other at `border * scale`; with [`LodBand::CrossFade`], [`LodSwitch::level_at`].
    pub fn next(&self, current: u8, distance: f32) -> u8 {
        if self.band == LodBand::CrossFade {
            return self.level_at(distance);
        }
        let current = usize::from(current);
        let passed = |(k, b): (usize, &LodBoundary)| {
            let border = b.border * self.scale;
            let threshold = if k == current {
                border + b.play
            } else if k + 1 == current {
                border - b.play
            } else {
                border
            };
            distance >= threshold
        };
        level(
            self.boundaries
                .iter()
                .enumerate()
                .take_while(|&kb| passed(kb))
                .count(),
        )
    }

    /// With [`LodBand::Hysteresis`], whether the level at `distance` depends on the level drawn before: `distance` lies
    /// within `play` of a scaled border, where [`LodSwitch::next`] can give either side. Never with
    /// [`LodBand::CrossFade`].
    pub fn in_band(&self, distance: f32) -> bool {
        self.band == LodBand::Hysteresis
            && self.boundaries.iter().any(|b| {
                let border = b.border * self.scale;
                b.play > 0.0 && distance >= border - b.play && distance < border + b.play
            })
    }

    /// With [`LodBand::CrossFade`], how far the level drawn at `distance` has faded into the next: `(distance - border *
    /// scale) / play` within the band past its boundary above, 0 elsewhere.
    pub fn fade(&self, distance: f32) -> f32 {
        if self.band != LodBand::CrossFade {
            return 0.0;
        }
        let at = usize::from(self.level_at(distance));
        match self.boundaries.get(at) {
            Some(b) if b.play > 0.0 && distance >= b.border * self.scale => {
                ((distance - b.border * self.scale) / b.play).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

fn level(count: usize) -> u8 {
    u8::try_from(count).unwrap_or(u8::MAX)
}

impl Component for LodSwitch {
    const NAME: &'static str = "core.lod_switch";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// World distances of the placement, not local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// On a placement; at least one boundary, at most 254; `scale` finite and positive; every border finite and not
    /// negative, not below the one before it; every play finite and not negative.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Instance>(e) {
            return Err(
                "a level-of-detail switch on an entity that is not a placement (no core.instance)".into(),
            );
        }
        if self.boundaries.is_empty() || self.boundaries.len() > 254 {
            return Err(format!("{} boundaries, not 1 to 254", self.boundaries.len()));
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Err(format!("scale {} is not a finite positive number", self.scale));
        }
        let mut before = 0.0f32;
        for (k, b) in self.boundaries.iter().enumerate() {
            if !(b.border.is_finite() && b.border >= before) {
                return Err(format!(
                    "boundary {k}'s border {} is not finite and at least {before}",
                    b.border
                ));
            }
            if !(b.play.is_finite() && b.play >= 0.0) {
                return Err(format!(
                    "boundary {k}'s play {} is not a finite number of 0 or more",
                    b.play
                ));
            }
            before = b.border;
        }
        Ok(())
    }
}

/// The view the distances of every [`LodSwitch`] are stated for (`core.lod_switch_view`, a world resource): a camera
/// of vertical field of view `reference_fov_y` measures them as they are, a narrower one as nearer. A world with level
/// switches has it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LodSwitchView {
    /// Radians, more than 0 and less than pi.
    pub reference_fov_y: f32,
}

impl LodSwitchView {
    /// What a camera of vertical field of view `fov_y` (radians) multiplies its distances by before comparing them to
    /// the borders: `tan(fov_y / 2) / tan(reference_fov_y / 2)`.
    pub fn factor(&self, fov_y: f32) -> f32 {
        (fov_y / 2.0).tan() / (self.reference_fov_y / 2.0).tan()
    }
}

impl Resource for LodSwitchView {
    const NAME: &'static str = "core.lod_switch_view";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// A field of view strictly between 0 and pi.
    fn verify(&self, _w: &World) -> Result<(), String> {
        let f = self.reference_fov_y;
        if !(f.is_finite() && f > 0.0 && f < std::f32::consts::PI) {
            return Err(format!(
                "reference vertical field of view {f} is not between 0 and pi radians"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch(band: LodBand) -> LodSwitch {
        LodSwitch {
            boundaries: vec![
                LodBoundary {
                    border: 10.0,
                    play: 2.0,
                },
                LodBoundary {
                    border: 30.0,
                    play: 4.0,
                },
                LodBoundary {
                    border: 9999.0,
                    play: 0.0,
                },
            ],
            scale: 2.0,
            band,
        }
    }

    /// With no history the borders are plain: 20 m and 60 m at scale 2.
    #[test]
    fn with_no_history_the_plain_borders_choose() {
        let s = switch(LodBand::Hysteresis);
        assert_eq!(
            [19.9, 20.0, 59.9, 60.0, 1e6].map(|d| s.level_at(d)),
            [0, 1, 1, 2, 3]
        );
    }

    /// From level 1 the boundary above is passed at 60 + 4 and the one below at 20 - 2; others at their border.
    #[test]
    fn the_boundaries_next_to_the_level_drawn_take_the_play() {
        let s = switch(LodBand::Hysteresis);
        assert_eq!([17.9, 18.0, 63.9, 64.0].map(|d| s.next(1, d)), [0, 1, 1, 2]);
        // from 0: up at 20 + 2; the next boundary plain
        assert_eq!([21.9, 22.0, 60.0].map(|d| s.next(0, d)), [0, 1, 2]);
        // from 2: down below 60 - 4, and the first boundary plain
        assert_eq!([56.0, 55.9, 19.9].map(|d| s.next(2, d)), [2, 1, 0]);
        assert!(s.in_band(19.0) && s.in_band(63.0) && !s.in_band(30.0) && !s.in_band(64.0));
    }

    /// With cross-fade the level is the distance's alone, from border * scale + play, and the band fades.
    #[test]
    fn a_cross_fade_follows_the_distance_and_fades_across_the_band() {
        let s = switch(LodBand::CrossFade);
        assert_eq!([21.9, 22.0, 63.9, 64.0].map(|d| s.level_at(d)), [0, 1, 1, 2]);
        assert_eq!(s.next(2, 21.0), 0);
        assert_eq!([19.0, 21.0, 62.0].map(|d| s.fade(d)), [0.0, 0.5, 0.5]);
        assert!(!s.in_band(21.0));
    }

    #[test]
    fn verify_refuses_what_it_cannot_apply() {
        let mut w = World::new();
        let placed = w.spawn();
        let prototype = w.spawn();
        w.insert(
            placed,
            Instance {
                prototype,
                variant: 0,
            },
        );
        assert_eq!(switch(LodBand::Hysteresis).verify(placed, &w), Ok(()));
        assert!(switch(LodBand::Hysteresis).verify(prototype, &w).is_err());
        for bad in [
            LodSwitch {
                boundaries: vec![],
                ..switch(LodBand::Hysteresis)
            },
            LodSwitch {
                scale: 0.0,
                ..switch(LodBand::Hysteresis)
            },
            LodSwitch {
                boundaries: vec![
                    LodBoundary {
                        border: 30.0,
                        play: 0.0,
                    },
                    LodBoundary {
                        border: 10.0,
                        play: 0.0,
                    },
                ],
                ..switch(LodBand::Hysteresis)
            },
            LodSwitch {
                boundaries: vec![LodBoundary {
                    border: 10.0,
                    play: -1.0,
                }],
                ..switch(LodBand::Hysteresis)
            },
        ] {
            assert!(bad.verify(placed, &w).is_err(), "{bad:?}");
        }
        let view = LodSwitchView {
            reference_fov_y: 43f32.to_radians(),
        };
        assert!((view.factor(43f32.to_radians()) - 1.0).abs() < 1e-6);
        assert!(LodSwitchView { reference_fov_y: 0.0 }.verify(&w).is_err());
    }
}
