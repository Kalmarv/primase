//! A shader's output baked to a texture that tiles: a block of K x K cells (one cell: one unit of the program's U and
//! V), made periodic with period K by cross-fading its first cell with the cell one period on.
//!
//! The program is evaluated once ([`evaluate`]) over K + 1 cells per axis: a grid of (K + 1) * texels pixels with
//! period K + 1, so pixel x samples U = (x + 0.5) / texels, as it does on a mesh whose U runs one per cell. The bake is
//! K * texels pixels per axis. Outside the first cell of an axis it is the program's value E there. In the first cell
//! (x < texels), at s = (x + 0.5) / texels, it is s * E(x) + (1 - s) * E(x + K * texels): the program's own value
//! fading in from its value one period on, so the last texel of the block (E at K * texels - 1) meets the first
//! (mostly E at K * texels) as the program continues there, and the seam is at the start of the block, spread over a
//! cell. Both axes cross-fade, bilinearly (the corner cell mixes four values).
//!
//! The cross-fade is the one approximation: a program that is already periodic with period K (E(x + K * texels) =
//! E(x)) bakes exactly; any other is changed in the first cell of each axis only, to the blend above.

use crate::evaluate::Constants;
use crate::{Error, Evaluation, Grid, Input, PixelShader, Texture, evaluate_with};
use primase_core::parallel::Threads;
use std::collections::BTreeMap;

fn error(detail: impl Into<String>) -> Error {
    Error {
        bytecode: "evaluate_periodic",
        at: 0,
        detail: detail.into(),
    }
}

/// Evaluate `shader` as a texture that tiles with period `cells` (K, at least 2) cells of `texels` pixels each
/// (module documentation): each requested target is (K * texels)^2 values in row-major order, or `None` when the
/// program does not write it. The evaluation grid, (K + 1) * texels pixels per axis, is at most 4096. `textures`,
/// `inputs` and `targets` are as [`evaluate`] takes them; the grid's U and V ([`crate::Lane::U`], `V`) run one per
/// cell. The instruction and sample counts are the grid's.
pub fn evaluate_periodic(
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    cells: u32,
    texels: u32,
    targets: &[(u32, u8)],
) -> Result<Evaluation, Error> {
    evaluate_periodic_with(
        Threads::ONE,
        shader,
        constants,
        textures,
        inputs,
        cells,
        texels,
        targets,
    )
}

/// [`evaluate_periodic`] with the grid evaluated on `threads` threads ([`crate::evaluate_with`]): the same bake on any
/// count.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_periodic_with(
    threads: Threads,
    shader: &dyn PixelShader,
    constants: &Constants,
    textures: &BTreeMap<u32, Texture>,
    inputs: &BTreeMap<(String, u32), Input>,
    cells: u32,
    texels: u32,
    targets: &[(u32, u8)],
) -> Result<Evaluation, Error> {
    if cells < 2 {
        return Err(error(format!("{cells} cells: a periodic bake needs at least 2")));
    }
    if texels == 0 {
        return Err(error("a cell needs at least one texel"));
    }
    // the grid: one cell more than the bake per axis, at most 4096 pixels
    let size = cells
        .checked_add(1)
        .and_then(|c| c.checked_mul(texels))
        .filter(|&n| n <= 4096)
        .ok_or_else(|| {
            error(format!(
                "{} cells of {texels} texels need a grid of more than 4096",
                u64::from(cells) + 1
            ))
        })?;
    let period = (size / texels) as f32;
    let grid = evaluate_with(
        threads,
        shader,
        constants,
        textures,
        inputs,
        Grid {
            size,
            period: [period; 2],
        },
        targets,
    )?;
    // the bake's side, which is also the shift of one period
    let (size, out, texels) = (size as usize, (cells * texels) as usize, texels as usize);
    // per output index along an axis: (weight of E there, weight of E one period on)
    let weights: Vec<(f32, f32)> = (0..out)
        .map(|x| {
            if x < texels {
                let s = (x as f32 + 0.5) / texels as f32;
                (s, 1. - s)
            } else {
                (1., 0.)
            }
        })
        .collect();
    let targets = grid
        .targets
        .into_iter()
        .map(|t| {
            t.map(|e| {
                let at = |x: usize, y: usize| e[y * size + x];
                let mut values = Vec::with_capacity(out * out);
                for (y, &(wy0, wy1)) in weights.iter().enumerate() {
                    for (x, &(wx0, wx1)) in weights.iter().enumerate() {
                        let mut v = at(x, y).map(|c| c * wx0 * wy0);
                        for (w, (px, py)) in [
                            (wx1 * wy0, (x + out, y)),
                            (wx0 * wy1, (x, y + out)),
                            (wx1 * wy1, (x + out, y + out)),
                        ] {
                            if w != 0. {
                                let o = at(px, py);
                                for c in 0..4 {
                                    v[c] += o[c] * w;
                                }
                            }
                        }
                        values.push(v);
                    }
                }
                values
            })
        })
        .collect();
    Ok(Evaluation {
        targets,
        instructions: grid.instructions,
        samples: grid.samples,
    })
}
