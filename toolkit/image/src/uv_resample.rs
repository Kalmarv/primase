//! Re-rasterising a texture from one set of texture coordinates into another's space: [`rebake`] draws triangles into
//! a new image, each texel taking the source texture's value at the coordinates the triangle gives it. The mechanism
//! of `rebase-texcoord-set`; it knows no mesh, material or game.
//!
//! **Which texels a triangle claims.** The output is `width` x `height` texels over the unit square (texel `(x, y)`
//! centred at `((x + 0.5) / width, (y + 0.5) / height)`, v down). A triangle claims the texels whose centres lie
//! inside it. A centre exactly on an edge belongs to one of the two triangles sharing the edge: the one that runs the
//! edge towards increasing y (or, on a horizontal edge, towards decreasing x) once its corners are put in
//! counter-clockwise order, and every edge is evaluated from its lexicographically smaller end, so the two triangles
//! of a shared edge decide it from exactly opposite numbers and neighbouring triangles claim every texel once. A
//! triangle that claims no texel centre (thinner than a texel, or a point) claims the texel holding its centroid,
//! when that lies in the output. A texel already claimed is not claimed again; [`Rebaked::overlapped`] counts the
//! claims refused, which is how a caller finds that coordinates overlap.
//!
//! **What a claimed texel holds.** The source coordinates at the texel centre, interpolated linearly across the
//! triangle from the corners' (for a centroid claim, clamped into the triangle), transformed by the slot's
//! transform, then the source sampled there bilinearly through its wrap, over the stored channel values (no
//! transfer function is applied: filtering averages what is stored). `adjust` may change the sampled value
//! per triangle (a normal map's tangent frame turns with the triangle), in the channel range 0-255.
//!
//! **Padding.** After the triangles, `padding` rounds each texel that nothing claimed, next to a claimed or padded
//! one, takes the mean of those neighbours (the eight around it, rounded half up), so filtering at a triangle's
//! edge reads its own colours and not the fill. Texels still unset hold `fill`.
use crate::{Image, Samples};
use primase_core::pbr::{UvTransform, Wrap};

/// A triangle to draw: its corners in the output's unit square, and the source coordinates at those corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvTriangle {
    pub to: [[f32; 2]; 3],
    pub from: [[f32; 2]; 3],
}

/// An image drawn by [`rebake`].
#[derive(Clone, Debug, PartialEq)]
pub struct Rebaked {
    pub image: Image,
    /// Texels claimed by a triangle.
    pub claimed: u64,
    /// Claims refused because another triangle held the texel.
    pub overlapped: u64,
    /// Texels given by padding.
    pub padded: u64,
}

/// The texel `index` of an axis of `n` texels, under `wrap`.
pub(crate) fn wrapped(index: i64, n: i64, wrap: Wrap) -> usize {
    let i = match wrap {
        Wrap::Repeat => index.rem_euclid(n),
        Wrap::Clamp => index.clamp(0, n - 1),
        Wrap::Mirror => {
            let m = index.rem_euclid(2 * n);
            if m < n { m } else { 2 * n - 1 - m }
        }
    };
    i as usize
}

/// `image` (8-bit samples) at `uv`, bilinear over the four nearest texel centres, each axis under its wrap; the
/// channel values 0-255.
pub fn sample_bilinear(image: &Image, wrap: [Wrap; 2], uv: [f32; 2]) -> Result<[f32; 4], String> {
    let Samples::U8(px) = &image.samples else {
        return Err("only 8-bit pixels are resampled".to_owned());
    };
    let (w, h) = (i64::from(image.width), i64::from(image.height));
    if w == 0 || h == 0 {
        return Err("an image with no texels".to_owned());
    }
    let x = f64::from(uv[0]) * w as f64 - 0.5;
    let y = f64::from(uv[1]) * h as f64 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let at = |xi: i64, yi: i64| {
        let (cx, cy) = (wrapped(xi, w, wrap[0]), wrapped(yi, h, wrap[1]));
        let i = (cy * w as usize + cx) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    Ok(std::array::from_fn(|k| {
        let top = f64::from(a[k]) * (1.0 - fx) + f64::from(b[k]) * fx;
        let bottom = f64::from(c[k]) * (1.0 - fx) + f64::from(d[k]) * fx;
        (top * (1.0 - fy) + bottom * fy) as f32
    }))
}

/// Twice the signed area of `a b c`, evaluated from the lexicographically smaller of `a`, `b` so that the same edge
/// run in opposite directions gives exactly opposite numbers.
fn edge(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    let forward = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if (a[0], a[1]) <= (b[0], b[1]) {
        forward(a, b)
    } else {
        -forward(b, a)
    }
}

/// The texels `to` claims in a `width` x `height` output, as `(x, y)` in row-major order (see the module
/// documentation).
pub fn claimed_texels(to: &[[f32; 2]; 3], width: u32, height: u32) -> Vec<(u32, u32)> {
    let scale = [f64::from(width), f64::from(height)];
    let mut p: [[f64; 2]; 3] =
        std::array::from_fn(|i| [f64::from(to[i][0]) * scale[0], f64::from(to[i][1]) * scale[1]]);
    let area = edge(p[0], p[1], p[2]);
    let mut out = Vec::new();
    if area != 0.0 && area.is_finite() {
        if area < 0.0 {
            p.swap(1, 2);
        }
        let lo = |k: usize| p.iter().map(|q| q[k]).fold(f64::INFINITY, f64::min);
        let hi = |k: usize| p.iter().map(|q| q[k]).fold(f64::NEG_INFINITY, f64::max);
        let x0 = (lo(0) - 0.5).ceil().max(0.0);
        let x1 = (hi(0) - 0.5).floor().min(f64::from(width) - 1.0);
        let y0 = (lo(1) - 0.5).ceil().max(0.0);
        let y1 = (hi(1) - 0.5).floor().min(f64::from(height) - 1.0);
        // an edge decides a texel centre on it when it runs towards increasing y, or along -x
        let owns = |a: [f64; 2], b: [f64; 2]| b[1] > a[1] || (b[1] == a[1] && b[0] < a[0]);
        let edges = [(1, 2), (2, 0), (0, 1)];
        let mut y = y0;
        while y <= y1 {
            let mut x = x0;
            while x <= x1 {
                let c = [x + 0.5, y + 0.5];
                let inside = edges.iter().all(|&(a, b)| {
                    let w = edge(p[a], p[b], c);
                    w > 0.0 || (w == 0.0 && owns(p[a], p[b]))
                });
                if inside {
                    out.push((x as u32, y as u32));
                }
                x += 1.0;
            }
            y += 1.0;
        }
    }
    if out.is_empty() {
        let centroid = [
            (f64::from(to[0][0]) + f64::from(to[1][0]) + f64::from(to[2][0])) / 3.0 * scale[0],
            (f64::from(to[0][1]) + f64::from(to[1][1]) + f64::from(to[2][1])) / 3.0 * scale[1],
        ];
        if centroid.iter().all(|c| c.is_finite())
            && (0.0..scale[0]).contains(&centroid[0])
            && (0.0..scale[1]).contains(&centroid[1])
        {
            out.push((centroid[0].floor() as u32, centroid[1].floor() as u32));
        }
    }
    out
}

/// The source coordinates of `tri` at `at` (in the output's texel space): linear in the corners', with the weights
/// clamped into the triangle.
fn coordinates_at(tri: &UvTriangle, scale: [f64; 2], at: [f64; 2]) -> [f32; 2] {
    let p: [[f64; 2]; 3] = std::array::from_fn(|i| {
        [
            f64::from(tri.to[i][0]) * scale[0],
            f64::from(tri.to[i][1]) * scale[1],
        ]
    });
    let area = edge(p[0], p[1], p[2]);
    let weights = if area == 0.0 || !area.is_finite() {
        [1.0 / 3.0; 3]
    } else {
        let w = [
            edge(p[1], p[2], at) / area,
            edge(p[2], p[0], at) / area,
            edge(p[0], p[1], at) / area,
        ];
        let clamped = w.map(|x| x.max(0.0));
        let sum: f64 = clamped.iter().sum();
        if sum > 0.0 {
            clamped.map(|x| x / sum)
        } else {
            [1.0 / 3.0; 3]
        }
    };
    std::array::from_fn(|k| {
        (0..3)
            .map(|i| weights[i] * f64::from(tri.from[i][k]))
            .sum::<f64>() as f32
    })
}

/// A texture to draw from: its pixels, how it wraps, and the transform its slot applies to the coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub image: &'a Image,
    pub wrap: [Wrap; 2],
    pub transform: Option<UvTransform>,
}

/// The image [`rebake`] makes: its size in texels, the rounds of padding, and the value of texels nothing reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub size: (u32, u32),
    pub padding: u32,
    pub fill: [u8; 4],
}

/// Draw `triangles` of `source` into a canvas (see the module documentation). `adjust(i, px)` turns the value
/// sampled for triangle `i`.
pub fn rebake(
    source: &Source<'_>,
    triangles: &[UvTriangle],
    canvas: &Canvas,
    mut adjust: impl FnMut(usize, [f32; 4]) -> [f32; 4],
) -> Result<Rebaked, String> {
    let Canvas {
        size: (width, height),
        padding,
        fill,
    } = *canvas;
    if width == 0 || height == 0 {
        return Err("an output with no texels".to_owned());
    }
    let (w, h) = (width as usize, height as usize);
    let mut pixels: Vec<u8> = fill.iter().copied().cycle().take(w * h * 4).collect();
    let mut set = vec![false; w * h];
    let (mut claimed, mut overlapped) = (0u64, 0u64);
    let scale = [f64::from(width), f64::from(height)];
    for (i, tri) in triangles.iter().enumerate() {
        for (x, y) in claimed_texels(&tri.to, width, height) {
            let at = y as usize * w + x as usize;
            if set[at] {
                overlapped += 1;
                continue;
            }
            set[at] = true;
            claimed += 1;
            let uv = coordinates_at(tri, scale, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
            let uv = source.transform.map_or(uv, |t| t.apply(uv));
            let px = adjust(i, sample_bilinear(source.image, source.wrap, uv)?);
            for (k, v) in px.iter().enumerate() {
                pixels[at * 4 + k] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    let mut padded = 0u64;
    for _ in 0..padding {
        let before = set.clone();
        for y in 0..h {
            for x in 0..w {
                if before[y * w + x] {
                    continue;
                }
                let mut sum = [0u32; 4];
                let mut n = 0u32;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if (dx, dy) == (0, 0) || nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let ni = ny as usize * w + nx as usize;
                        if before[ni] {
                            n += 1;
                            for (k, s) in sum.iter_mut().enumerate() {
                                *s += u32::from(pixels[ni * 4 + k]);
                            }
                        }
                    }
                }
                if n > 0 {
                    for (k, s) in sum.iter().enumerate() {
                        pixels[(y * w + x) * 4 + k] = ((s * 2 + n) / (2 * n)) as u8;
                    }
                    set[y * w + x] = true;
                    padded += 1;
                }
            }
        }
    }
    Ok(Rebaked {
        image: Image {
            width,
            height,
            samples: Samples::U8(pixels),
        },
        claimed,
        overlapped,
        padded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Image {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend([x as u8 * 16, y as u8 * 16, 0, 255]);
            }
        }
        Image {
            width: w,
            height: h,
            samples: Samples::U8(px),
        }
    }

    fn source(image: &Image) -> Source<'_> {
        Source {
            image,
            wrap: [Wrap::Clamp; 2],
            transform: None,
        }
    }

    fn canvas(size: u32, padding: u32, fill: u8) -> Canvas {
        Canvas {
            size: (size, size),
            padding,
            fill: [fill; 4],
        }
    }

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let Samples::U8(px) = &image.samples else {
            unreachable!()
        };
        let i = (y * image.width + x) as usize * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn two_triangles_of_a_square_claim_every_texel_once() {
        let (a, b, c, d) = ([0., 0.], [1., 0.], [1., 1.], [0., 1.]);
        // from 2 x 2: a triangle that claims no centre takes the texel of its centroid, so at 1 x 1 the one texel is
        // claimed by both
        for size in [2, 3, 8, 13] {
            let first = claimed_texels(&[a, b, c], size, size);
            let second = claimed_texels(&[a, c, d], size, size);
            let mut all: Vec<_> = first.iter().chain(&second).copied().collect();
            all.sort_unstable();
            let want: Vec<_> = (0..size).flat_map(|y| (0..size).map(move |x| (x, y))).collect();
            assert_eq!(
                all, want,
                "{size} x {size}: the diagonal's texels belong to one triangle"
            );
        }
    }

    #[test]
    fn a_shared_edge_goes_to_one_triangle_whichever_way_the_corners_run() {
        // a diagonal through texel centres of a 4 x 4 output: (0.5, 0.5), (1.5, 1.5), ...
        let (a, b, c, d) = ([0., 0.], [1., 0.], [1., 1.], [0., 1.]);
        let forward = claimed_texels(&[a, c, b], 4, 4);
        let reverse = claimed_texels(&[a, b, c], 4, 4);
        assert_eq!(forward, reverse);
        let other = claimed_texels(&[a, d, c], 4, 4);
        for t in &forward {
            assert!(
                !other.contains(t),
                "{t:?} is claimed by both sides of the diagonal"
            );
        }
    }

    #[test]
    fn a_sliver_claims_the_texel_of_its_centroid() {
        let tri = [[0.30, 0.30], [0.31, 0.30], [0.30, 0.31]];
        assert_eq!(claimed_texels(&tri, 8, 8), vec![(2, 2)]);
        // outside the output: nothing
        let outside = [[1.5, 1.5], [1.51, 1.5], [1.5, 1.51]];
        assert!(claimed_texels(&outside, 8, 8).is_empty());
    }

    #[test]
    fn bilinear_sampling_follows_each_wrap() {
        let image = gradient(4, 1);
        // between the first two texel centres
        let v = sample_bilinear(&image, [Wrap::Clamp; 2], [0.25, 0.5]).unwrap();
        assert_eq!(v[0], 8.0);
        // a whole period over leaves the value; clamp holds the edge texel
        let repeated = sample_bilinear(&image, [Wrap::Repeat, Wrap::Clamp], [1.375, 0.5]).unwrap();
        assert_eq!(repeated[0], 16.0);
        let clamped = sample_bilinear(&image, [Wrap::Clamp; 2], [1.375, 0.5]).unwrap();
        assert_eq!(clamped[0], 48.0);
        // mirrored: 1.125 reads the image backwards from its end, 0.875 is the same texel
        let mirrored = sample_bilinear(&image, [Wrap::Mirror, Wrap::Clamp], [1.125, 0.5]).unwrap();
        let before = sample_bilinear(&image, [Wrap::Mirror, Wrap::Clamp], [0.875, 0.5]).unwrap();
        assert_eq!(mirrored, before);
    }

    #[test]
    fn a_rotated_copy_of_the_texture_is_drawn_texel_for_texel() {
        // the source turned a quarter: output (u, v) takes source (v, 1 - u)
        let texture = gradient(4, 4);
        let corners_to = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
        let corners_from = corners_to.map(|[u, v]| [v, 1. - u]);
        let tris = [
            UvTriangle {
                to: [corners_to[0], corners_to[1], corners_to[2]],
                from: [corners_from[0], corners_from[1], corners_from[2]],
            },
            UvTriangle {
                to: [corners_to[0], corners_to[2], corners_to[3]],
                from: [corners_from[0], corners_from[2], corners_from[3]],
            },
        ];
        let out = rebake(&source(&texture), &tris, &canvas(4, 0, 9), |_, p| p).unwrap();
        assert_eq!((out.claimed, out.overlapped, out.padded), (16, 0, 0));
        for y in 0..4 {
            for x in 0..4 {
                // output texel (x, y) is at (x + .5, y + .5) / 4 and reads source texel (y, 3 - x)
                assert_eq!(
                    pixel(&out.image, x, y),
                    pixel(&texture, y, 3 - x),
                    "texel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn overlapping_triangles_count_the_claims_they_lose_and_padding_extends_the_edge() {
        let texture = gradient(2, 2);
        let tri = UvTriangle {
            to: [[0., 0.], [0.5, 0.], [0., 0.5]],
            from: [[0., 0.], [1., 0.], [0., 1.]],
        };
        let out = rebake(&source(&texture), &[tri, tri], &canvas(8, 0, 7), |_, p| p).unwrap();
        // the texels with x + y < 3, and the four on the hypotenuse, which its triangle owns
        assert_eq!(out.claimed, 10);
        assert_eq!(out.overlapped, 10);
        // an unclaimed texel stays the fill without padding, takes its neighbours' mean with it
        assert_eq!(pixel(&out.image, 7, 7), [7; 4]);
        let padded = rebake(&source(&texture), &[tri], &canvas(8, 1, 7), |_, p| p).unwrap();
        assert!(padded.padded > 0);
        let corner = pixel(&padded.image, 4, 0);
        assert_ne!(corner, [7; 4]);
        assert_eq!(pixel(&padded.image, 7, 7), [7; 4]);
    }

    #[test]
    fn adjust_sees_the_triangle_it_draws() {
        let texture = gradient(2, 2);
        let a = UvTriangle {
            to: [[0., 0.], [1., 0.], [0., 1.]],
            from: [[0., 0.], [1., 0.], [0., 1.]],
        };
        let b = UvTriangle {
            to: [[1., 0.], [1., 1.], [0., 1.]],
            from: [[1., 0.], [1., 1.], [0., 1.]],
        };
        let out = rebake(&source(&texture), &[a, b], &canvas(4, 0, 0), |i, mut p| {
            p[3] = if i == 0 { 10.0 } else { 20.0 };
            p
        })
        .unwrap();
        assert_eq!(pixel(&out.image, 0, 0)[3], 10);
        assert_eq!(pixel(&out.image, 3, 3)[3], 20);
    }
}
