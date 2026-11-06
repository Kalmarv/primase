//! Elden Ring's texture switch: when a placement draws its textures' coarse forms (`core.texture_switch`, read with
//! the low-resolution twins, [`super::area::Options::low_textures`]), and the view its distances are for
//! (`core.texture_switch_view`).
//!
//! What the game does (eldenring.exe 1.17.3, traced in its code): every drawn instance keeps a texture level, created
//! at 1 (the coarse form). Each frame it takes the distance from the camera to the instance's world-space box (0
//! inside), times `tan(fovY / 2) / tan(43 deg / 2)` (the game's reference field of view), and moves the level to 1 at
//! `(B + P) * s * kA` or more and back to 0 below `(B - P) * s * kA * h`, where `B` and `P` are the
//! `tex_lv01_BorderDist` and `tex_lv01_PlayDist` of the instance's `PartsDrawParam` row, `s` its LOD type's
//! draw-distance scale and `kA`, `h` the runtime factors below. A border of 0 is no switch. Which row: an asset's is its model's `refDrawParamId`
//! (`AssetEnvironmentGeometryParam`, the row the game's geometry-info setup reads); a map piece's is the MSB part's
//! `parts_draw_param_id` where it sets `use_parts_draw_param` (observed: it fits all 20 map-piece draws of two
//! captures; a map piece without the flag has no row established here and no switch).
//!
//! The scale: the game fills a table per LOD type from row `100 + lodType` (the level switch's from row `lodType`,
//! `super::lod_switch`) of `LoadBalancerDrawDistScaleParam` at the load balancer's current level, and takes 1.0
//! where the row is missing. The code can read another param in its place (index 0xc1, past the 193 of the game's own
//! param table) when a check returns anything but 1; the check returns a value only it writes, set to 1 when it is 0,
//! which holds 1 in a dumped running image, and no other code or data of the executable refers to it (read from its
//! code and data). So `LoadBalancerNewDrawDistScaleParam_win64` is not read. The read takes the scale only where it
//! does not depend on the load balancer's level: the row holds one value at every level (or is missing: 1.0).
//! Otherwise it refuses, naming the row.
//!
//! The inputs, each stated in the world's `core.texture_switch_view` or refused by name:
//!
//! - The reference height: the game divides the factor by `height / ref_height` (two pixel counts of the view) where
//!   `ref_height` is set, and sets it to 0 (its renderer setup at 0xb1c48f): `reference_height` 0, no height term.
//! - Texture quality ([`TextureQuality`], an option of the read): the switch's code reads no quality setting (the
//!   writers of its enable flag are the map load's steps and the character-creation menu; its runtime factors are the
//!   two below), but one half of the flag is set at start-up by an obfuscated function not followed. The rule is
//!   checked against captures at `MAX` only, so the read takes `MAX` (its default) and refuses any other quality.
//!
//! The state, stated as the world's [`TextureSwitchState::SETTLED`]: `kA` (the state's `load`) is the memory load
//! balancer's factor, 1.0 when memory is not under pressure (0.5, 0.25, 0.1 or 0 under it); `h` (the state's `ramp`)
//! rises from 0 to 1 over 3 s once the switch is enabled, and every level is held at 1 while it is off. The game turns
//! it off from the start of each map load until the load's `STEP_HorseWait`, and while the character-creation menu
//! is open. So the read's state is the game's in play, at least 3 s after a map load or the menu, with memory not under
//! pressure; a frame outside that state is not what the read describes.
use crate::invalid;
use fromsoft_formats::param::{AssetGeometryParam, DrawDistScaleParam, PartsDraw, PartsDrawParam};
use fromsoft_formats::{Archive, Install, Result, dcx::Decoder, game_params::read_param, msb};
use primase_core::{TextureSwitch, TextureSwitchState, TextureSwitchView};

/// The game's reference vertical field of view for the switch distances, set by its renderer setup.
pub const REFERENCE_FOV_Y_DEGREES: f32 = 43.0;

/// The view resource a read with switches declares: the game's reference field of view, no reference height (the
/// game's 0), in the settled state ([module docs](self)).
pub fn view() -> TextureSwitchView {
    TextureSwitchView {
        reference_fov_y: REFERENCE_FOV_Y_DEGREES.to_radians(),
        reference_height: 0,
        state: TextureSwitchState::SETTLED,
    }
}

/// The open point of the read's texture quality: the half of the switch's enable flag the game sets at start-up, through
/// a function this read does not follow. The report names it ([`super::area::Report::notices`]).
pub const QUALITY_OPEN_POINT: &str = "the start-up half of the switch's enable flag (settings +0x6f3) is set through an \
     obfuscated function (eldenring.exe 1.17.3, 0xe46930) not followed: whether texture quality reaches the switch there \
     is open";

/// Elden Ring's texture quality setting (`TextureQuality` in its `GraphicsConfig.xml`), an input of a read with the
/// switch. The game's own default is the preset its hardware detection picks; the read's is [`TextureQuality::Max`],
/// the setting the rule was checked at, and it refuses any other ([`TextureQuality::check`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextureQuality {
    Low,
    Medium,
    High,
    #[default]
    Max,
}

impl primase_core::pass::Choice for TextureQuality {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("low", TextureQuality::Low),
        ("medium", TextureQuality::Medium),
        ("high", TextureQuality::High),
        ("max", TextureQuality::Max),
    ];
}

impl TextureQuality {
    /// `Ok` at the quality the rule is checked at; otherwise the read's refusal, naming the quality.
    pub fn check(self) -> Result<()> {
        if self == TextureQuality::Max {
            Ok(())
        } else {
            Err(invalid(
                "texture switch",
                0,
                format!(
                    "texture quality {self:?}: the switch is checked against the game at MAX only (the switch's code \
                     reads no quality setting, but the start-up half of its enable flag is not followed)"
                ),
            ))
        }
    }
}

/// What the switch rows of the read's placements came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The texture quality the switch was read at, where it was read.
    pub quality: Option<TextureQuality>,
    /// Why the params could not be read (the regulation needs its key); no placement has a switch then.
    pub unread: Option<String>,
    /// Placements given `core.texture_switch`.
    pub switched: usize,
    /// Placements whose row has a border of 0: no switch.
    pub border_zero: usize,
    /// Placements without a row, by reason.
    pub rows: RowReport,
}

/// How the placements' `PartsDrawParam` rows were found (`Params::row`), for each switch that reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowReport {
    /// Assets whose model has no geometry row or names no `PartsDrawParam` row (or one the param lacks).
    pub without_row: usize,
    /// Map pieces without `use_parts_draw_param`: no row established.
    pub pieces_unestablished: usize,
    /// Assets whose MSB part names a `PartsDrawParam` row of its own other than its model's (the model's is used).
    pub asset_rows_differ: usize,
}

/// The params the texture switch and the level switch (`super::lod_switch`) read.
#[derive(Debug)]
pub struct Params {
    parts: PartsDrawParam,
    assets: AssetGeometryParam,
    scale: DrawDistScaleParam,
}

impl Params {
    /// The params from the install's regulation.
    pub fn read(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<Params> {
        let param = |leaf: &str| read_param(install, archive, decoder, leaf);
        Ok(Params {
            parts: PartsDrawParam::from_param(&param("PartsDrawParam.param")?)?,
            assets: AssetGeometryParam::from_param(&param("AssetEnvironmentGeometryParam.param")?)?,
            scale: DrawDistScaleParam::from_param(&param("LoadBalancerDrawDistScaleParam.param")?)?,
        })
    }

    /// The scale of LOD type `lod_type` in the rows from `base` (the level switch's 0, the texture switch's 100), where it
    /// does not depend on the load balancer's level ([module docs](self)).
    pub(super) fn scale(&self, base: i32, lod_type: u8) -> Result<f32> {
        let id = base + i32::from(lod_type);
        let Some(row) = self.scale.row(id) else {
            return Ok(1.0);
        };
        let first = row.levels[0];
        if row.levels.iter().any(|&v| v != first) {
            return Err(invalid(
                "texture switch",
                0,
                format!(
                    "LoadBalancerDrawDistScaleParam row {id} differs by load-balancer level ({:?}): the level is the game's runtime state",
                    row.levels
                ),
            ));
        }
        Ok(first)
    }

    /// The switch of a placement of `model` (`AEGnnn_mmm` or a map piece's `mNNNNNN`) by a part of `kind` with the
    /// draw controls `draw`, counted in `report`; `None` where it has none.
    pub fn switch(
        &self,
        kind: msb::PartKind,
        draw: &msb::DrawControls,
        model: &str,
        report: &mut Report,
    ) -> Result<Option<TextureSwitch>> {
        let Some(row) = self.row(kind, draw, model, &mut report.rows)? else {
            return Ok(None);
        };
        if row.tex_lv01_border == 0.0 {
            report.border_zero += 1;
            return Ok(None);
        }
        report.switched += 1;
        Ok(Some(TextureSwitch {
            border: row.tex_lv01_border,
            play: row.tex_lv01_play,
            scale: self.scale(100, row.lod_type)?,
        }))
    }

    /// The `PartsDrawParam` row of a placement of `model` by a part of `kind` with the draw controls `draw`: an asset's
    /// model's `refDrawParamId`, a map piece's part's row where it sets `use_parts_draw_param`; counted in `report`
    /// where there is none. A map piece naming a row the param lacks is an error.
    pub(super) fn row(
        &self,
        kind: msb::PartKind,
        draw: &msb::DrawControls,
        model: &str,
        report: &mut RowReport,
    ) -> Result<Option<&PartsDraw>> {
        let row = match kind {
            msb::PartKind::Asset => {
                let id = asset_row(model);
                let Some(row) = id
                    .and_then(|id| self.assets.row(id))
                    .filter(|a| a.draw_param >= 0)
                    .and_then(|a| self.parts.row(a.draw_param))
                else {
                    report.without_row += 1;
                    return Ok(None);
                };
                if draw.use_parts_draw_param && i32::from(draw.parts_draw_param_id) != row.id {
                    report.asset_rows_differ += 1;
                }
                row
            }
            msb::PartKind::MapPiece => {
                if !draw.use_parts_draw_param {
                    report.pieces_unestablished += 1;
                    return Ok(None);
                }
                let id = i32::from(draw.parts_draw_param_id);
                self.parts.row(id).ok_or_else(|| {
                    invalid(
                        "texture switch",
                        0,
                        format!("{model}: PartsDrawParam has no row {id}"),
                    )
                })?
            }
            _ => return Ok(None),
        };
        Ok(Some(row))
    }
}

/// The `AssetEnvironmentGeometryParam` row of the asset model `AEGnnn_mmm`: `nnn * 1000 + mmm`.
fn asset_row(model: &str) -> Option<i32> {
    let rest = model
        .get(..3)
        .filter(|h| h.eq_ignore_ascii_case("aeg"))
        .and(model.get(3..))?;
    let (group, number) = rest.split_once('_')?;
    let digits =
        |s: &str| (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<i32>().ok())?;
    Some(digits(group)? * 1000 + digits(number)?)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use fromsoft_formats::param::{AssetGeometry, DrawDistScale, LodBoundary};

    pub(in crate::sekiro::read) fn params(scale: [f32; 21]) -> Params {
        Params {
            parts: PartsDrawParam {
                rows: vec![
                    PartsDraw {
                        id: 8100,
                        lod_boundaries: [
                            LodBoundary {
                                border: 5.0,
                                play: 1.0,
                            },
                            LodBoundary {
                                border: 20.0,
                                play: 2.0,
                            },
                            LodBoundary {
                                border: 30.0,
                                play: 0.0,
                            },
                            LodBoundary {
                                border: 9999.0,
                                play: 0.0,
                            },
                            LodBoundary {
                                border: 9999.0,
                                play: 0.0,
                            },
                        ],
                        cross_fade: false,
                        shadow_lod_offset: 0,
                        tex_lv01_border: 80.0,
                        tex_lv01_play: 5.0,
                        lod_type: 1,
                    },
                    PartsDraw {
                        id: 7,
                        lod_boundaries: [
                            LodBoundary {
                                border: 10.0,
                                play: 0.5,
                            },
                            LodBoundary {
                                border: 5.0,
                                play: 0.0,
                            },
                            LodBoundary {
                                border: 30.0,
                                play: 0.0,
                            },
                            LodBoundary {
                                border: 40.0,
                                play: 0.0,
                            },
                            LodBoundary {
                                border: 50.0,
                                play: 0.0,
                            },
                        ],
                        cross_fade: true,
                        shadow_lod_offset: 1,
                        tex_lv01_border: 0.0,
                        tex_lv01_play: 1.0,
                        lod_type: 0,
                    },
                ],
            },
            assets: AssetGeometryParam {
                rows: vec![
                    AssetGeometry {
                        id: 810_000,
                        draw_param: 8100,
                    },
                    AssetGeometry {
                        id: 810_001,
                        draw_param: -1,
                    },
                ],
            },
            scale: DrawDistScaleParam {
                rows: vec![
                    DrawDistScale {
                        id: 101,
                        levels: scale,
                    },
                    DrawDistScale {
                        id: 1,
                        levels: [0.75; 21],
                    },
                ],
            },
        }
    }

    pub(in crate::sekiro::read) fn draw(
        use_parts_draw_param: bool,
        parts_draw_param_id: u16,
    ) -> msb::DrawControls {
        msb::DrawControls {
            use_parts_draw_param,
            parts_draw_param_id,
            ..Default::default()
        }
    }

    /// An asset takes its model's row, a map piece its part's where it sets the flag; a border of 0, a missing row
    /// and a piece without the flag have no switch, each counted.
    #[test]
    fn a_placement_takes_its_row() {
        let p = params([1.0; 21]);
        let mut r = Report::default();
        let asset =
            |model: &str, d: msb::DrawControls, r: &mut Report| p.switch(msb::PartKind::Asset, &d, model, r);
        let piece = |model: &str, d: msb::DrawControls, r: &mut Report| {
            p.switch(msb::PartKind::MapPiece, &d, model, r)
        };
        assert_eq!(
            asset("AEG810_000", draw(false, 0), &mut r).unwrap(),
            Some(TextureSwitch {
                border: 80.0,
                play: 5.0,
                scale: 1.0
            })
        );
        assert_eq!(asset("AEG810_001", draw(false, 0), &mut r).unwrap(), None);
        assert_eq!(asset("AEG999_000", draw(false, 0), &mut r).unwrap(), None);
        assert!(asset("AEG810_000", draw(true, 7), &mut r).unwrap().is_some());
        assert!(piece("m000032", draw(true, 8100), &mut r).unwrap().is_some());
        assert_eq!(piece("m000033", draw(true, 7), &mut r).unwrap(), None);
        assert_eq!(piece("m000034", draw(false, 0), &mut r).unwrap(), None);
        assert!(piece("m000035", draw(true, 5), &mut r).is_err());
        assert_eq!(
            p.switch(msb::PartKind::Collision, &draw(true, 8100), "h000001", &mut r)
                .unwrap(),
            None
        );
        assert_eq!(
            r,
            Report {
                quality: None,
                unread: None,
                switched: 3,
                border_zero: 1,
                rows: RowReport {
                    without_row: 2,
                    pieces_unestablished: 1,
                    asset_rows_differ: 1,
                },
            }
        );
    }

    /// The scale is taken only where it does not depend on the load balancer's level: one value at every level of its
    /// row (1.0 without a row), from the row of the switch's base.
    #[test]
    fn the_scale_does_not_depend_on_runtime_state() {
        let mut halved = [0.5; 21];
        assert_eq!(params(halved).scale(100, 1).unwrap(), 0.5);
        assert_eq!(params(halved).scale(100, 2).unwrap(), 1.0);
        assert_eq!(params(halved).scale(0, 1).unwrap(), 0.75);
        halved[3] = 1.0;
        let e = params(halved).scale(100, 1).unwrap_err().to_string();
        assert!(e.contains("row 101 differs"), "{e}");
        assert_eq!(asset_row("AEG210_334"), Some(210_334));
        assert_eq!(
            (
                asset_row("aeg001_002"),
                asset_row("m000032"),
                asset_row("AEG21_334")
            ),
            (Some(1002), None, None)
        );
        assert!((view().reference_fov_y - 43f32.to_radians()).abs() < 1e-7);
        assert_eq!(
            (view().reference_height, view().state),
            (0, TextureSwitchState::SETTLED)
        );
    }

    /// The read takes the switch at MAX, its default, and refuses any other quality by name.
    #[test]
    fn only_the_checked_quality_is_read() {
        assert_eq!(TextureQuality::default(), TextureQuality::Max);
        assert!(TextureQuality::Max.check().is_ok());
        for q in [TextureQuality::Low, TextureQuality::Medium, TextureQuality::High] {
            let e = q.check().unwrap_err().to_string();
            assert!(e.contains(&format!("texture quality {q:?}")), "{e}");
        }
    }
}
