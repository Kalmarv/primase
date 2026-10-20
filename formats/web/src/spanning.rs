//! The rule that files a placement by element centre when its pivot does not locate its geometry, from prototype
//! boxes a caller gives (a reader that loads geometry per cell knows them before loading).

use std::collections::BTreeMap;

use primase_core::{Entity, Instance, Transform, World};

use crate::Result;

/// A prototype's box in its own space: `[min, max]`, metres.
pub type LocalBox = [[f64; 3]; 2];

/// How far a pivot may lie outside its placement's own world-space box before the pivot is taken to say nothing about
/// where the geometry is.
pub const PIVOT_MARGIN: f64 = 1.0;

/// Placements whose pivot does not locate their geometry, to be filed by element centre, and the counts behind them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spans {
    /// (a) or (b): the placements to split.
    pub marked: Vec<Entity>,
    /// Placements whose prototype has a box.
    pub measured: usize,
    /// Any part of the world box in another cell than another part (the broad rule, reported only).
    pub straddling: usize,
    /// (a) The world box's extent on a ground axis exceeds the cell size.
    pub wider_than_a_cell: usize,
    /// (b) The pivot lies more than [`PIVOT_MARGIN`] outside the world box (geometry in world space, placed at the
    /// origin).
    pub pivot_outside: usize,
}

/// Placements whose world-space geometry box, from their prototype's `local_box` through their transform, is wider
/// than a cell (`tile_size`) on a ground axis, or does not contain their pivot: pivot filing would give one cell all
/// of a model placed at the origin with world-space vertices. A small prop across a cell edge meets neither and keeps
/// its pivot filing and instancing. `local_box` is asked once per prototype; `None`: no geometry, not measured.
pub fn spanning(
    world: &World,
    tile_size: f64,
    mut local_box: impl FnMut(Entity) -> Result<Option<LocalBox>>,
) -> Result<Spans> {
    let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].map(primase_core::frame::ground);
    let ground = |p: [f64; 3]| -> [f64; 2] {
        std::array::from_fn(|k| (0..3).map(|i| p[i] * axes[i][k] as f64).sum::<f64>())
    };
    let mut boxes = BTreeMap::<Entity, Option<LocalBox>>::new();
    let mut spans = Spans::default();
    for (e, i) in world.iter::<Instance>() {
        let Some(t) = world.get::<Transform>(e) else {
            continue;
        };
        let b = match boxes.get(&i.prototype) {
            Some(b) => *b,
            None => {
                let b = local_box(i.prototype)?;
                boxes.insert(i.prototype, b);
                b
            }
        };
        let Some(b) = b else { continue };
        spans.measured += 1;
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let (mut glo, mut ghi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for corner in 0..8 {
            let local: [f64; 3] = std::array::from_fn(|k| b[(corner >> k) & 1][k]);
            let p: [f64; 3] = std::array::from_fn(|r| {
                f64::from(t.cols[3][r]) + (0..3).map(|c| f64::from(t.cols[c][r]) * local[c]).sum::<f64>()
            });
            let g = ground(p);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            for k in 0..2 {
                glo[k] = glo[k].min(g[k]);
                ghi[k] = ghi[k].max(g[k]);
            }
        }
        let cells = |v: f64| (v / tile_size).floor();
        if (0..2).any(|k| cells(glo[k]) != cells(ghi[k])) {
            spans.straddling += 1;
        }
        let wide = (0..2).any(|k| ghi[k] - glo[k] > tile_size);
        let pivot = t.cols[3].map(f64::from);
        let outside = (0..3).any(|k| pivot[k] < lo[k] - PIVOT_MARGIN || pivot[k] > hi[k] + PIVOT_MARGIN);
        spans.wider_than_a_cell += usize::from(wide);
        spans.pivot_outside += usize::from(outside);
        if wide || outside {
            spans.marked.push(e);
        }
    }
    Ok(spans)
}
