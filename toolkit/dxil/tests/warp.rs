//! Evaluation against Microsoft's WARP rasterizer on the fixtures (see `common/warp.rs`): `PRIMASE_WARP` (the WARP
//! oracle program), `PRIMASE_DXC` and `PRIMASE_WARP_SCRATCH` (a folder for case files and results). Skips unless all
//! three are set.
mod common;
use common::warp::{Case, Result, compare};
use primase_testdata::real_data;
use std::collections::BTreeMap;
use std::path::Path;

fn tools() -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
    Some((
        real_data("PRIMASE_DXC")?,
        real_data("PRIMASE_WARP")?,
        real_data("PRIMASE_WARP_SCRATCH")?,
    ))
}
fn f(x: f32) -> u32 {
    x.to_bits()
}

#[test]
fn fixtures_agree_with_warp() -> Result<()> {
    let Some((dxc, warp, scratch)) = tools() else {
        return Ok(());
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut total = 0;
    let mut all = Vec::new();
    // blend: uv, weight, id; constants scale, bias, flags; one texture.
    let mut cases = Vec::new();
    for (k, (uv, w, id)) in [
        ([0.25f32, 0.75], 0.4f32, 7u32),
        ([0.6, 0.1], 0.9, 3),
        ([0.0, 1.0], -2.0, 0),
    ]
    .into_iter()
    .enumerate()
    {
        for front in [false, true] {
            cases.push(Case {
                name: format!("blend{k}{}", if front { "f" } else { "b" }),
                front,
                inputs: BTreeMap::from([(1, [f(uv[0]), f(uv[1]), f(w), 0]), (2, [id, 0, 0, 0])]),
                buffers: BTreeMap::from([(
                    3,
                    vec![
                        [f(2.), f(4.), f(1.5), 0],
                        [f(0.5), f(0.25), f(0.1), 0],
                        [5, 0, 0, 0],
                    ],
                )]),
                textures: BTreeMap::from([(2, [0.3, 0.7, 0.2 + k as f32 * 0.1, 1.0])]),
            });
        }
    }
    let o = compare(&dir.join("blend.dxil"), &cases, &dxc, &warp, &scratch)?;
    total += o.lanes;
    all.extend(o.mismatches);
    // ops: a, n constants; t input.
    let mut cases = Vec::new();
    for (k, (a, n, t)) in [
        ([2.5f32, 3., 8., 4.], [-7i32, 5], [0.75f32, 0.25]),
        ([-1.25, 0.5, 0.125, 9.], [100, 3], [0.1, 0.9]),
        ([3.5, -2., 1024., 0.0625], [-1, 0], [0.5, 0.5]),
    ]
    .into_iter()
    .enumerate()
    {
        cases.push(Case {
            name: format!("ops{k}"),
            front: true,
            inputs: BTreeMap::from([(1, [f(t[0]), f(t[1]), 0, 0])]),
            buffers: BTreeMap::from([(0, vec![a.map(f32::to_bits), [n[0] as u32, n[1] as u32, 0, 0]])]),
            textures: BTreeMap::new(),
        });
    }
    let o = compare(&dir.join("ops.dxil"), &cases, &dxc, &warp, &scratch)?;
    total += o.lanes;
    all.extend(o.mismatches);
    println!("{total} output lanes compared with WARP, {} differ", all.len());
    for m in &all {
        println!("MISMATCH {m}");
    }
    assert!(total > 0 && all.is_empty());
    Ok(())
}
