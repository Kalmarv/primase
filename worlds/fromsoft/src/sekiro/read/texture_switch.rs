//! Elden Ring's texture switch: when a placement draws its textures' coarse forms (`core.texture_switch`, read with
//! the low-resolution twins, [`super::area::Options::low_textures`]), and the view its distances are for
//! (`core.texture_switch_view`).
//!
//! What the game does (eldenring.exe 1.17.3, traced in its code): every drawn instance keeps a texture level, created
//! at 1 (the coarse form). Each frame it takes the distance from the camera to the instance's world-space box (0
//! inside), times `tan(fovY / 2) / tan(43 deg / 2)` (the game's reference field of view; its reference height is off),
//! and moves the level to 1 at `(B + P) * s` or more and back to 0 below `(B - P) * s`, where `B` and `P` are the
//! `tex_lv01_BorderDist` and `tex_lv01_PlayDist` of the instance's `PartsDrawParam` row and `s` its LOD type's
//! draw-distance scale. A border of 0 is no switch. Which row: an asset's is its model's `refDrawParamId`
//! (`AssetEnvironmentGeometryParam`, the row the game's geometry-info setup reads); a map piece's is the MSB part's
//! `parts_draw_param_id` where it sets `use_parts_draw_param` (observed: it fits all 20 map-piece draws of two
//! captures; a map piece without the flag has no row established here and no switch).
//!
//! The scale: the game fills a table per LOD type from row `100 + lodType` of one of two params, picked at run time
//! (param 0xc1, or `LoadBalancerDrawDistScaleParam` when a check returns 1), at the load balancer's current level,
//! and takes 1.0 where the row is missing. The read takes the scale only where it does not depend on that choice:
//! `LoadBalancerDrawDistScaleParam`'s row holds one value at every level (or is missing: 1.0) and
//! `LoadBalancerNewDrawDistScaleParam_win64` (the other PC member of the family) holds no such row. Otherwise it
//! refuses, naming the row.
//!
//! Not modelled (the game's runtime state, stated here): the memory load balancer's factor on both thresholds (1.0,
//! or 0.5, 0.25, 0.1, 0 under memory pressure), the 3 s ramp of the return threshold after the system is enabled,
//! and the level forced to 1 while it is off. The read states the steady state without memory pressure. Texture
//! quality does not take part.
use crate::invalid;
use fromsoft_formats::param::{AssetGeometryParam, DrawDistScaleParam, Param, PartsDrawParam};
use fromsoft_formats::{Archive, Install, Result, dcx::Decoder, game_params::read_param, msb};
use primase_core::{TextureSwitch, TextureSwitchView};
use std::collections::BTreeSet;

/// The game's reference vertical field of view for the switch distances, set by its renderer setup.
pub const REFERENCE_FOV_Y_DEGREES: f32 = 43.0;

/// The view resource a read with switches declares.
pub fn view() -> TextureSwitchView {
    TextureSwitchView {
        reference_fov_y: REFERENCE_FOV_Y_DEGREES.to_radians(),
    }
}

/// What the switch rows of the read's placements came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Why the params could not be read (the regulation needs its key); no placement has a switch then.
    pub unread: Option<String>,
    /// Placements given `core.texture_switch`.
    pub switched: usize,
    /// Placements whose row has a border of 0: no switch.
    pub border_zero: usize,
    /// Assets whose model has no geometry row or names no `PartsDrawParam` row (or one the param lacks).
    pub without_row: usize,
    /// Map pieces without `use_parts_draw_param`: no row established.
    pub pieces_unestablished: usize,
    /// Assets whose MSB part names a `PartsDrawParam` row of its own other than its model's (the model's is used).
    pub asset_rows_differ: usize,
}

/// The three params the switch reads.
pub struct Params {
    parts: PartsDrawParam,
    assets: AssetGeometryParam,
    scale: DrawDistScaleParam,
    /// The row ids of `LoadBalancerNewDrawDistScaleParam_win64`.
    new_scale_rows: BTreeSet<i32>,
}

impl Params {
    /// The params from the install's regulation.
    pub fn read(install: &Install, archive: &Archive, decoder: &Decoder<'_>) -> Result<Params> {
        let param = |leaf: &str| read_param(install, archive, decoder, leaf);
        Ok(Params {
            parts: PartsDrawParam::from_param(&param("PartsDrawParam.param")?)?,
            assets: AssetGeometryParam::from_param(&param("AssetEnvironmentGeometryParam.param")?)?,
            scale: DrawDistScaleParam::from_param(&param("LoadBalancerDrawDistScaleParam.param")?)?,
            new_scale_rows: param("LoadBalancerNewDrawDistScaleParam_win64.param")
                .map(|p: Param| p.rows.iter().map(|r| r.id).collect())?,
        })
    }

    /// The scale of LOD type `lod_type`, where both candidate sources give one value ([module docs](self)).
    fn scale(&self, lod_type: u8) -> Result<f32> {
        let id = 100 + i32::from(lod_type);
        if self.new_scale_rows.contains(&id) {
            return Err(invalid(
                "texture switch",
                0,
                format!(
                    "LoadBalancerNewDrawDistScaleParam_win64 holds row {id}: which of the two scale params the game reads is a runtime choice not modelled"
                ),
            ));
        }
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
        if row.tex_lv01_border == 0.0 {
            report.border_zero += 1;
            return Ok(None);
        }
        report.switched += 1;
        Ok(Some(TextureSwitch {
            border: row.tex_lv01_border,
            play: row.tex_lv01_play,
            scale: self.scale(row.lod_type)?,
        }))
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
mod tests {
    use super::*;
    use fromsoft_formats::param::{AssetGeometry, DrawDistScale, PartsDraw};

    fn params(scale: [f32; 21], new_rows: &[i32]) -> Params {
        Params {
            parts: PartsDrawParam {
                rows: vec![
                    PartsDraw {
                        id: 8100,
                        tex_lv01_border: 80.0,
                        tex_lv01_play: 5.0,
                        lod_type: 1,
                    },
                    PartsDraw {
                        id: 7,
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
                rows: vec![DrawDistScale {
                    id: 101,
                    levels: scale,
                }],
            },
            new_scale_rows: new_rows.iter().copied().collect(),
        }
    }

    fn draw(use_parts_draw_param: bool, parts_draw_param_id: u16) -> msb::DrawControls {
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
        let p = params([1.0; 21], &[]);
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
                unread: None,
                switched: 3,
                border_zero: 1,
                without_row: 2,
                pieces_unestablished: 1,
                asset_rows_differ: 1,
            }
        );
    }

    /// The scale is taken only where it does not depend on the game's runtime choices: one value at every level of
    /// the old param's row (1.0 without a row) and no row in the new one.
    #[test]
    fn the_scale_does_not_depend_on_runtime_state() {
        let mut halved = [0.5; 21];
        assert_eq!(params(halved, &[]).scale(1).unwrap(), 0.5);
        assert_eq!(params(halved, &[]).scale(2).unwrap(), 1.0);
        halved[3] = 1.0;
        let e = params(halved, &[]).scale(1).unwrap_err().to_string();
        assert!(e.contains("row 101 differs"), "{e}");
        let e = params([1.0; 21], &[101]).scale(1).unwrap_err().to_string();
        assert!(e.contains("holds row 101"), "{e}");
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
    }
}
