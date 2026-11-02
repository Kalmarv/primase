//! Elden Ring's level-of-detail switch: which level of its model a placement draws, by distance (`core.lod_switch`), and
//! the view its distances are for (`core.lod_switch_view`).
//!
//! What the game does (eldenring.exe 1.17.3, read from its code; not compared with frames of the running game): each
//! frame, per drawn model and view, it takes the eye's distance to the model's world box times a factor of the view,
//! and the level from the model's `PartsDrawParam` row: the boundaries `lv01` to `lv45` (border `B`, play `P`), every
//! border times `s`, row `lodType` of the load balancer's draw-distance scales (the texture switch's rows are `100 +
//! lodType`; [`super::texture_switch`]). With the row's `enableCrossFade` the level is the distance's alone, from `s * B
//! + P`, fading across the band; without it the level moves with hysteresis from the one drawn before, `s * B + P` on
//! the boundary above it and `s * B - P` below it ([`LodBand`]). The game remembers only the level's low two bits and
//! compares the first border unscaled from level 4: its quirks, named in `core.lod_switch`'s documentation, not stated
//! here. Shadow views add the row's `shadowDrawLodOffset`; the read states the main view's rule.
//!
//! Which row: the one the texture switch reads ([`super::texture_switch::Params`]); that the level's row object is the
//! texture level's is read from the code, the assets' and map pieces' rows as the texture switch establishes them. The
//! view factor sits where the texture switch's field-of-view factor does relative to the eye, so the reference field of
//! view is taken as the texture switch's 43 degrees (inferred). The scale is taken where it does not depend on the load
//! balancer's level, as the texture switch's is.
#[cfg(test)]
use super::texture_switch::tests::{draw, params};
use super::texture_switch::{Params, REFERENCE_FOV_Y_DEGREES, RowReport};
use fromsoft_formats::{Result, msb};
use primase_core::{LodBand, LodBoundary, LodSwitch, LodSwitchView};

/// The view resource a read with level switches declares: the reference field of view, as the texture switch's.
pub fn view() -> LodSwitchView {
    LodSwitchView {
        reference_fov_y: REFERENCE_FOV_Y_DEGREES.to_radians(),
    }
}

/// What the level-switch rows of the read's placements came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Why the params could not be read (the regulation needs its key); no placement has a switch then.
    pub unread: Option<String>,
    /// Placements given `core.lod_switch`, and of them those whose row cross-fades.
    pub switched: usize,
    pub cross_fade: usize,
    /// Placements without a row, by reason, as the texture switch counts them.
    pub rows: RowReport,
    /// Placements whose row has a negative or non-finite border or play, or borders that decrease from one boundary to
    /// the next (the game's two rules read such a row differently): none is stated.
    pub unstated: usize,
    /// Placements whose LOD type's scale row differs by the load balancer's level (the game's runtime state), and those
    /// rows: none is stated.
    pub scale_by_level: usize,
    pub scale_rows: std::collections::BTreeSet<i32>,
}

impl Params {
    /// The level switch of a placement of `model` by a part of `kind` with the draw controls `draw`, counted in
    /// `report`; `None` where it has none.
    pub fn lod_switch(
        &self,
        kind: msb::PartKind,
        draw: &msb::DrawControls,
        model: &str,
        report: &mut Report,
    ) -> Result<Option<LodSwitch>> {
        let Some(row) = self.row(kind, draw, model, &mut report.rows)? else {
            return Ok(None);
        };
        let boundaries: Vec<LodBoundary> = row
            .lod_boundaries
            .iter()
            .map(|b| LodBoundary {
                border: b.border,
                play: b.play,
            })
            .collect();
        let usable =
            |b: &LodBoundary| b.border.is_finite() && b.border >= 0.0 && b.play.is_finite() && b.play >= 0.0;
        if !boundaries.iter().all(usable) || boundaries.windows(2).any(|w| w[1].border < w[0].border) {
            report.unstated += 1;
            return Ok(None);
        }
        let Ok(scale) = self.scale(0, row.lod_type) else {
            report.scale_by_level += 1;
            report.scale_rows.insert(i32::from(row.lod_type));
            return Ok(None);
        };
        report.switched += 1;
        report.cross_fade += usize::from(row.cross_fade);
        Ok(Some(LodSwitch {
            boundaries,
            scale,
            band: if row.cross_fade {
                LodBand::CrossFade
            } else {
                LodBand::Hysteresis
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placement takes its row's boundaries, its band and the scale of row `lodType`; a row whose borders decrease is
    /// stated as none, counted; the rows are found as the texture switch finds them.
    #[test]
    fn a_placement_takes_its_rows_boundaries_band_and_scale() {
        let p = params([1.0; 21]);
        let mut r = Report::default();
        let s = p
            .lod_switch(msb::PartKind::Asset, &draw(false, 0), "AEG810_000", &mut r)
            .unwrap()
            .unwrap();
        assert_eq!(
            (s.band, s.scale, s.boundaries.len()),
            (LodBand::Hysteresis, 0.75, 5)
        );
        assert_eq!(
            s.boundaries[1],
            LodBoundary {
                border: 20.0,
                play: 2.0
            }
        );
        assert_eq!(
            p.lod_switch(msb::PartKind::MapPiece, &draw(true, 7), "m000033", &mut r)
                .unwrap(),
            None
        );
        assert_eq!(
            p.lod_switch(msb::PartKind::Asset, &draw(false, 0), "AEG999_000", &mut r)
                .unwrap(),
            None
        );
        assert_eq!(
            r,
            Report {
                unread: None,
                switched: 1,
                cross_fade: 0,
                rows: RowReport {
                    without_row: 1,
                    ..RowReport::default()
                },
                unstated: 1,
                ..Report::default()
            }
        );
        assert!((view().reference_fov_y - 43f32.to_radians()).abs() < 1e-7);
    }
}
