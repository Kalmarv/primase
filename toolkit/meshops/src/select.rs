//! Spatial selection decisions over caller-supplied points, independent of a game's reader.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensestSquare {
    pub rect: [f32; 4],
    pub kept: usize,
    pub points: usize,
}

/// The grid-aligned square retaining the most points among windows anchored on occupied
/// cells. This deliberately preserves the opener's occupied-corner search, not a search
/// over all possible corners. Cells/windows are half open; lexicographically first corner
/// wins ties. Side must be a positive whole number of cells. Invalid points/units refuse.
pub fn densest_square(points: &[[f32; 2]], side: f32, cell: f32) -> Result<Option<DensestSquare>, String> {
    if !side.is_finite() || side <= 0.0 || !cell.is_finite() || cell <= 0.0 {
        return Err("densest square: side and cell must be finite and positive".into());
    }
    let span = f64::from(side) / f64::from(cell);
    if span < 1.0 || span > i32::MAX as f64 || (span - span.round()).abs() > 1e-6 {
        return Err("densest square: side must be a positive whole number of cells".into());
    }
    let span = span.round() as i64;
    let mut cells = BTreeMap::<(i64, i64), usize>::new();
    for (i, point) in points.iter().enumerate() {
        let mut at = [0i64; 2];
        for (axis, &value) in point.iter().enumerate() {
            let index = (f64::from(value) / f64::from(cell)).floor();
            if !index.is_finite() || index < i32::MIN as f64 || index > i32::MAX as f64 {
                return Err(format!(
                    "densest square: point {i} axis {axis} is outside the finite grid"
                ));
            }
            at[axis] = index as i64;
        }
        *cells.entry((at[0], at[1])).or_default() += 1;
    }
    let mut best = None;
    for &(x, z) in cells.keys() {
        let kept = cells
            .range((x, i64::MIN)..=(x + span - 1, i64::MAX))
            .filter(|((_, cz), _)| (z..z + span).contains(cz))
            .map(|(_, n)| n)
            .sum::<usize>();
        if best.is_none_or(|(_, most)| kept > most) {
            best = Some(((x, z), kept));
        }
    }
    let Some(((x, z), kept)) = best else {
        return Ok(None);
    };
    let (x, z) = (x as f32 * cell, z as f32 * cell);
    let rect = [x, z, x + side, z + side];
    if !rect.iter().all(|v| v.is_finite()) {
        return Err("densest square: rectangle overflows".into());
    }
    Ok(Some(DensestSquare {
        rect,
        kept,
        points: points.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn density_matches_occupied_corners_with_negative_cells_and_stable_ties() {
        let points = [[-1.0, -1.0], [-0.5, -0.5], [0.0, 0.0], [9.0, 9.0]];
        assert_eq!(
            densest_square(&points, 2.0, 1.0).unwrap(),
            Some(DensestSquare {
                rect: [-1.0, -1.0, 1.0, 1.0],
                kept: 3,
                points: 4
            })
        );
        assert_eq!(
            densest_square(&[[5.0, 5.0], [-1.0, -1.0]], 1.0, 1.0)
                .unwrap()
                .unwrap()
                .rect,
            [-1.0, -1.0, 0.0, 0.0]
        );
        assert_eq!(densest_square(&[], 2.0, 1.0).unwrap(), None);
        assert_eq!(
            densest_square(&[[0.0, 0.0], [2.0, 0.0]], 2.0, 1.0)
                .unwrap()
                .unwrap()
                .kept,
            1
        );
    }
    #[test]
    fn invalid_units_and_points_are_not_silently_clamped() {
        for (side, cell) in [(0.0, 1.0), (1.0, 0.0), (1.5, 1.0), (f32::INFINITY, 1.0)] {
            assert!(densest_square(&[], side, cell).is_err());
        }
        assert!(densest_square(&[[f32::NAN, 0.0]], 2.0, 1.0).is_err());
    }
}
