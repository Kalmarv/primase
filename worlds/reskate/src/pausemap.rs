//! The pause-menu map picture (ui/map_image.png, ui/map_image.json, ui/map_clouds.png): a top-down software
//! rasteriser drawing every triangle in its texture's mean colour or sampling its texture ([`Paint`]), highest surface
//! on top, shaded by slope.

use crate::Result;
use crate::records::MapImageJson;
use primase_meshops::vector::Vec3;

#[derive(Debug)]
pub struct PauseMap {
    pub w: usize,
    pub h: usize,
    depth: Vec<f32>,
    rgba: Vec<u32>,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    sx: f32,
    sy: f32,
    pub triangles: i64,
    given: bool,
    pub sea_tint: bool,
}

/// The side the first level of a [`Pyramid`] is at most.
pub const PYRAMID_TOP: u32 = 128;

/// A texture's mip chain as the picture samples it: RGBA8 levels (sRGB bytes), the first the texture's largest level
/// at most [`PYRAMID_TOP`] on a side, each next one box-filtered down to 1 x 1.
#[derive(Debug)]
pub struct Pyramid {
    levels: Vec<(u32, u32, Vec<u8>)>,
}

/// How a sample past the texture's edge reads it (the material's wrap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseWrap {
    Repeat,
    Clamp,
    Mirror,
}

impl PauseWrap {
    /// Texel coordinate `i` of a level `n` texels long.
    fn texel(self, i: i64, n: u32) -> usize {
        let n = i64::from(n.max(1));
        (match self {
            PauseWrap::Repeat => i.rem_euclid(n),
            PauseWrap::Clamp => i.clamp(0, n - 1),
            PauseWrap::Mirror => {
                let m = i.rem_euclid(2 * n);
                if m >= n { 2 * n - 1 - m } else { m }
            }
        }) as usize
    }
}

impl Pyramid {
    /// The chain from a `w` x `h` RGBA8 level (at most [`PYRAMID_TOP`] on a side), down to 1 x 1.
    pub fn new(w: u32, h: u32, rgba: Vec<u8>) -> Pyramid {
        let mut levels = vec![(w, h, rgba)];
        while let Some((w, h, px)) = levels.last()
            && (*w > 1 || *h > 1)
        {
            let next = crate::texture::half(*w, *h, px.clone());
            levels.push(((w >> 1).max(1), (h >> 1).max(1), next));
        }
        Pyramid { levels }
    }

    /// The first level's size.
    pub fn top(&self) -> (u32, u32) {
        (self.levels[0].0, self.levels[0].1)
    }

    /// The colour at `uv`, bilinear in the level whose texel best matches `footprint` first-level texels per pixel
    /// (sRGB bytes as floats).
    pub fn sample(&self, uv: [f32; 2], footprint: f32, wrap: PauseWrap) -> [f32; 3] {
        let k = if footprint > 1.0 {
            (footprint.log2().round() as usize).min(self.levels.len() - 1)
        } else {
            0
        };
        let (w, h, px) = &self.levels[k];
        let (x, y) = (uv[0] * *w as f32 - 0.5, uv[1] * *h as f32 - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (ax, ay) = (x - fx, y - fy);
        let at = |i: i64, j: i64| {
            let o = (wrap.texel(j, *h) * *w as usize + wrap.texel(i, *w)) * 4;
            [px[o], px[o + 1], px[o + 2]].map(f32::from)
        };
        let (i, j) = (fx as i64, fy as i64);
        let (c00, c10, c01, c11) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
        std::array::from_fn(|c| {
            (c00[c] * (1.0 - ax) + c10[c] * ax) * (1.0 - ay) + (c01[c] * (1.0 - ax) + c11[c] * ax) * ay
        })
    }
}

/// What the picture paints a mesh's triangles with.
#[derive(Clone, Copy, Debug)]
pub enum Paint<'a> {
    /// One colour, 0xRRGGBB.
    Flat(u32),
    /// The texture `pyramid` at the vertices' coordinates `uv` (two per vertex), times `factor` (linear RGB), wrapping
    /// as `wrap` says.
    Texture {
        pyramid: &'a Pyramid,
        uv: &'a [f32],
        factor: [f32; 3],
        wrap: PauseWrap,
    },
}

/// One triangle's paint: its colour at barycentric weights.
struct Painter<'a> {
    paint: &'a Paint<'a>,
    /// The vertices' coordinates (textured), and the footprint in first-level texels per pixel.
    uv: [[f32; 2]; 3],
    footprint: f32,
    shade: f32,
}

impl<'a> Painter<'a> {
    /// Vertices `v` at screen points `s` (signed doubled area `area`), shaded by `shade`.
    fn of(paint: &'a Paint<'a>, v: [usize; 3], s: [(f32, f32); 3], area: f32, shade: f32) -> Painter<'a> {
        let (mut uv, mut footprint) = ([[0.0; 2]; 3], 1.0);
        if let Paint::Texture {
            pyramid, uv: coords, ..
        } = paint
        {
            // [`PauseMap::draw`] checked that the coordinates cover every vertex
            uv = v.map(|i| [coords[i * 2], coords[i * 2 + 1]]);
            if area.abs() >= 1e-6 {
                // the barycentric weights' screen gradients, and so the coordinates'
                let [(x0, y0), (x1, y1), (x2, y2)] = s;
                let inv = 1.0 / area;
                let dx = [(y1 - y2) * inv, (y2 - y0) * inv, (y0 - y1) * inv];
                let dy = [(x2 - x1) * inv, (x0 - x2) * inv, (x1 - x0) * inv];
                let (tw, th) = pyramid.top();
                let grad = |d: [f32; 3], c: usize| (0..3).map(|k| d[k] * uv[k][c]).sum::<f32>();
                footprint = [
                    grad(dx, 0) * tw as f32,
                    grad(dx, 1) * th as f32,
                    grad(dy, 0) * tw as f32,
                    grad(dy, 1) * th as f32,
                ]
                .into_iter()
                .fold(0.0f32, |a, g| a.max(g.abs()));
            }
        }
        Painter {
            paint,
            uv,
            footprint,
            shade,
        }
    }

    /// The pixel's colour at barycentric weights `b`, packed as the picture stores it.
    fn colour(&self, b: [f32; 3]) -> u32 {
        let rgb = match self.paint {
            Paint::Flat(rgb) => [(rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255].map(|c| c as f32),
            Paint::Texture {
                pyramid,
                factor,
                wrap,
                ..
            } => {
                let uv = [0, 1].map(|c| (0..3).map(|k| b[k] * self.uv[k][c]).sum::<f32>());
                let texel = pyramid.sample(uv, self.footprint, *wrap);
                std::array::from_fn(|c| {
                    f32::from(primase_image::linear_to_srgb8_f32(
                        primase_image::srgb_to_linear_f32(texel[c] / 255.0) * factor[c],
                    ))
                })
            }
        };
        let [r, g, b] = rgb.map(|c| (c * self.shade) as u32);
        0xFF00_0000u32 | b << 16 | g << 8 | r
    }
}

const PADDING: f32 = 1.1;
const MIN_HALF_HEIGHT: f32 = 25.0;

impl PauseMap {
    /// A front end's own picture (RGBA, row 0 north) covering `bounds` (ground frame, before the offset).
    pub fn given(w: usize, h: usize, rgba: &[u8], bounds: [f64; 4], off: Vec3) -> PauseMap {
        let px: Vec<u32> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        let (min_x, max_x) = (bounds[0] as f32 + off.x, bounds[2] as f32 + off.x);
        let (min_y, max_y) = (bounds[1] as f32 + off.y, bounds[3] as f32 + off.y);
        PauseMap {
            w,
            h,
            depth: vec![0.0; w * h],
            rgba: px,
            min_x,
            max_x,
            min_y,
            max_y,
            sx: w as f32 / (max_x - min_x),
            sy: h as f32 / (max_y - min_y),
            triangles: 0,
            given: true,
            sea_tint: true,
        }
    }

    pub fn new(region: [f64; 4], off: Vec3, height: usize) -> PauseMap {
        let h = height;
        let w = height * 16 / 9;
        let (low_x, high_x) = (region[0] as f32 + off.x, region[2] as f32 + off.x);
        let (low_y, high_y) = (region[1] as f32 + off.y, region[3] as f32 + off.y);
        let aspect = w as f32 / h as f32;
        let (cx, cy) = ((low_x + high_x) * 0.5, (low_y + high_y) * 0.5);
        let hh = ((high_y - low_y) * 0.5)
            .max((high_x - low_x) * 0.5 / aspect)
            .max(MIN_HALF_HEIGHT)
            * PADDING;
        let hw = hh * aspect;
        let (min_x, max_x, min_y, max_y) = (cx - hw, cx + hw, cy - hh, cy + hh);
        PauseMap {
            w,
            h,
            depth: vec![f32::NEG_INFINITY; w * h],
            rgba: vec![0; w * h],
            min_x,
            max_x,
            min_y,
            max_y,
            sx: w as f32 / (max_x - min_x),
            sy: h as f32 / (max_y - min_y),
            triangles: 0,
            given: false,
            sea_tint: true,
        }
    }

    /// Triangles whose vertex i is at basis * pos[i] + origin (map coordinates), painted with `paint`. Refused: an
    /// index past the vertices, or texture coordinates that don't cover every vertex a textured paint samples.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pos: &[f32],
        idx: &[u32],
        origin: Vec3,
        ex: Vec3,
        ey: Vec3,
        ez: Vec3,
        paint: &Paint<'_>,
    ) -> Result<(), String> {
        let vertices = pos.len() / 3;
        if let Some(i) = idx.iter().find(|&&i| i as usize >= vertices) {
            return Err(format!("pause map: index {i} past the {vertices} vertices"));
        }
        if let Paint::Texture { uv, .. } = paint
            && uv.len() / 2 < vertices
        {
            return Err(format!(
                "pause map: {} texture coordinates for {vertices} vertices of a textured material",
                uv.len() / 2
            ));
        }
        let (w, h) = (self.w as i32, self.h as i32);
        for t in (0..idx.len().saturating_sub(2)).step_by(3) {
            let v = [idx[t] as usize, idx[t + 1] as usize, idx[t + 2] as usize];
            let p = v.map(|i| origin + ex * pos[i * 3] + ey * pos[i * 3 + 1] + ez * pos[i * 3 + 2]);
            let s = p.map(|q| ((q.x - self.min_x) * self.sx, (self.max_y - q.y) * self.sy));
            let [(x0, y0), (x1, y1), (x2, y2)] = s;
            let mut bx0 = x0.min(x1.min(x2)).floor() as i32;
            let mut bx1 = x0.max(x1.max(x2)).ceil() as i32;
            let mut by0 = y0.min(y1.min(y2)).floor() as i32;
            let mut by1 = y0.max(y1.max(y2)).ceil() as i32;
            if bx1 < 0 || by1 < 0 || bx0 >= w || by0 >= h {
                continue;
            }
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            let nl = n.length();
            let shade = if nl > 1e-12 {
                0.62 + 0.38 * n.z.abs() / nl
            } else {
                1.0
            };
            let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
            bx0 = bx0.max(0);
            by0 = by0.max(0);
            bx1 = bx1.min(w - 1);
            by1 = by1.min(h - 1);
            self.triangles += 1;
            let painter = Painter::of(paint, v, s, area, shade);
            if area.abs() < 1e-6 || (bx1 - bx0 <= 1 && by1 - by0 <= 1) {
                let px = ((x0 + x1 + x2) / 3.0) as i32;
                let py = ((y0 + y1 + y2) / 3.0) as i32;
                if (px as u32) < w as u32 && (py as u32) < h as u32 {
                    let z = (p[0].z + p[1].z + p[2].z) / 3.0;
                    let third = 1.0 / 3.0;
                    self.plot(py as usize * self.w + px as usize, z, painter.colour([third; 3]));
                }
                continue;
            }
            let inv = 1.0 / area;
            for py in by0..=by1 {
                let fy = py as f32 + 0.5;
                for px in bx0..=bx1 {
                    let fx = px as f32 + 0.5;
                    let w0 = ((x1 - fx) * (y2 - fy) - (x2 - fx) * (y1 - fy)) * inv;
                    let w1 = ((x2 - fx) * (y0 - fy) - (x0 - fx) * (y2 - fy)) * inv;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = p[0].z * w0 + p[1].z * w1 + p[2].z * w2;
                    self.plot(
                        py as usize * self.w + px as usize,
                        z,
                        painter.colour([w0, w1, w2]),
                    );
                }
            }
        }
        Ok(())
    }

    /// Pixel `o` takes `col` at height `z` when it is the highest there (ties: the larger colour, for a stable order).
    fn plot(&mut self, o: usize, z: f32, col: u32) {
        if z > self.depth[o] || (z == self.depth[o] && col > self.rgba[o]) {
            self.depth[o] = z;
            self.rgba[o] = col;
        }
    }

    fn box_blur(a: &[f32], w: usize, h: usize, radius: i32) -> Vec<f32> {
        let mut t = vec![0f32; w * h];
        let mut o = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0f32, 0);
                for k in -radius..=radius {
                    let xx = (x as i32 + k).clamp(0, w as i32 - 1) as usize;
                    s += a[y * w + xx];
                    n += 1;
                }
                t[y * w + x] = s / n as f32;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0f32, 0);
                for k in -radius..=radius {
                    let yy = (y as i32 + k).clamp(0, h as i32 - 1) as usize;
                    s += t[yy * w + x];
                    n += 1;
                }
                o[y * w + x] = s / n as f32;
            }
        }
        o
    }

    /// ui/map_image.png, ui/map_image.json, ui/map_clouds.png.
    pub fn save(&mut self) -> Result<Vec<(String, Vec<u8>)>> {
        if !self.given && self.sea_tint {
            for i in 0..self.rgba.len() {
                let z = self.depth[i];
                if z.is_nan() || z >= -0.3 || (self.rgba[i] >> 24) == 0 {
                    continue;
                }
                let t = (0.55 + -z / 14.0).clamp(0.55, 0.92);
                let c = self.rgba[i];
                let r = ((c & 255) as f32 * (1.0 - t) + 30.0 * t) as u32;
                let g = (((c >> 8) & 255) as f32 * (1.0 - t) + 86.0 * t) as u32;
                let b = (((c >> 16) & 255) as f32 * (1.0 - t) + 122.0 * t) as u32;
                self.rgba[i] = 0xFF00_0000 | b << 16 | g << 8 | r;
            }
        }
        let bytes: Vec<u8> = self.rgba.iter().flat_map(|c| c.to_le_bytes()).collect();
        let image = primase_image::write_png(&primase_image::Image {
            width: self.w as u32,
            height: self.h as u32,
            samples: primase_image::Samples::U8(bytes),
        })
        .expect("valid image");
        let json = crate::json_bytes(
            "ui/map_image.json",
            &MapImageJson {
                min_x: self.min_x,
                max_x: self.max_x,
                min_z: -self.max_y,
                max_z: -self.min_y,
            },
            true,
        )?;

        // The cloud mask: coverage on a 256 grid, cleared by a margin and faded, scaled to 1024. The grid sizes round
        // half to even; that is part of the picture's definition (it decides which cells are covered), not a format.
        const GRID: usize = 256;
        const SIZE: usize = 1024;
        let rows = ((GRID as f32 * self.h as f32 / self.w as f32).round_ties_even() as i32).max(1) as usize;
        let mut square = vec![0f32; GRID * GRID];
        let top = (GRID - rows) / 2;
        for ry in 0..rows {
            for rx in 0..GRID {
                let y0 = ry * self.h / rows;
                let y1 = self.h.min((ry + 1) * self.h / rows);
                let x0 = rx * self.w / GRID;
                let x1 = self.w.min((rx + 1) * self.w / GRID);
                let mut covered = false;
                let mut y = y0;
                while y < y1 && !covered {
                    let mut x = x0;
                    while x < x1 {
                        if (self.rgba[y * self.w + x] >> 24) != 0 {
                            covered = true;
                            break;
                        }
                        x += 2;
                    }
                    y += 2;
                }
                square[(top + ry) * GRID + rx] = if covered { 1.0 } else { 0.0 };
            }
        }
        let margin = ((GRID as f32 * 0.035).round_ties_even() as i32).max(1);
        let mut clear = Self::box_blur(&Self::box_blur(&square, GRID, GRID, margin), GRID, GRID, margin);
        for v in &mut clear {
            *v = if *v > 0.02 { 1.0 } else { 0.0 };
        }
        let fade = ((GRID as f32 * 0.05 / 2.0).round_ties_even() as i32).max(1);
        let clear = Self::box_blur(&Self::box_blur(&clear, GRID, GRID, fade), GRID, GRID, fade);
        let mut mask = vec![0u8; SIZE * SIZE * 4];
        for y in 0..SIZE {
            let fy = y as f32 * (GRID as f32 - 1.0) / (SIZE as f32 - 1.0);
            let ya = fy as usize;
            let yb = (ya + 1).min(GRID - 1);
            let wy = fy - ya as f32;
            for x in 0..SIZE {
                let fx = x as f32 * (GRID as f32 - 1.0) / (SIZE as f32 - 1.0);
                let xa = fx as usize;
                let xb = (xa + 1).min(GRID - 1);
                let wx = fx - xa as f32;
                let v = (clear[ya * GRID + xa] * (1.0 - wx) + clear[ya * GRID + xb] * wx) * (1.0 - wy)
                    + (clear[yb * GRID + xa] * (1.0 - wx) + clear[yb * GRID + xb] * wx) * wy;
                mask[(y * SIZE + x) * 4 + 3] = ((v * 255.0 + 0.5) as i32).clamp(0, 255) as u8;
            }
        }
        let clouds = primase_image::write_png(&primase_image::Image {
            width: SIZE as u32,
            height: SIZE as u32,
            samples: primase_image::Samples::U8(mask),
        })
        .expect("valid image");
        Ok(vec![
            ("ui/map_image.png".into(), image),
            ("ui/map_image.json".into(), json),
            ("ui/map_clouds.png".into(), clouds),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texels of `w` x `h` RGBA8, each (r, g, b) from `f(x, y)`, opaque.
    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [r, g, b] = f(x, y);
                [r, g, b, 255]
            })
            .collect()
    }

    /// A pyramid halves down to 1 x 1; a sample at a texel's centre in the first level is that texel; the footprint
    /// picks the level whose texel matches it, the last past the chain.
    #[test]
    fn a_pyramid_samples_bilinear_at_the_footprints_level() {
        let p = Pyramid::new(4, 2, image(4, 2, |x, y| [(x * 60) as u8, (y * 200) as u8, 7]));
        assert_eq!(
            p.levels.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>(),
            [(4, 2), (2, 1), (1, 1)]
        );
        for (x, y) in [(0u32, 0u32), (3, 1), (2, 0)] {
            let uv = [(x as f32 + 0.5) / 4.0, (y as f32 + 0.5) / 2.0];
            assert_eq!(
                p.sample(uv, 1.0, PauseWrap::Clamp),
                [(x * 60) as f32, (y * 200) as f32, 7.0]
            );
        }
        // halfway between texels 0 and 1 of row 0: the mean
        assert_eq!(p.sample([0.25, 0.25], 1.0, PauseWrap::Clamp)[0], 30.0);
        // a footprint of two texels reads the 2 x 1 level, of a hundred the 1 x 1
        let mid = p.levels[1].2[0];
        assert_eq!(p.sample([0.25, 0.5], 2.0, PauseWrap::Clamp)[0], f32::from(mid));
        let last = p.levels[2].2[0];
        assert_eq!(p.sample([0.9, 0.1], 100.0, PauseWrap::Repeat)[0], f32::from(last));
    }

    /// Repeat wraps around, clamp holds the edge, mirror reflects.
    #[test]
    fn wraps_address_texels_past_the_edge() {
        assert_eq!(PauseWrap::Repeat.texel(-1, 4), 3);
        assert_eq!(PauseWrap::Repeat.texel(5, 4), 1);
        assert_eq!(
            (PauseWrap::Clamp.texel(-1, 4), PauseWrap::Clamp.texel(5, 4)),
            (0, 3)
        );
        assert_eq!(
            (PauseWrap::Mirror.texel(4, 4), PauseWrap::Mirror.texel(-1, 4)),
            (3, 0)
        );
    }

    /// A quad covering the picture, its texture red on the left and blue on the right (u across, clamped): the
    /// picture's left edge is red, its right edge blue; a flat quad above it paints its one colour.
    #[test]
    fn a_textured_quad_is_drawn_by_its_texture_and_a_flat_one_by_its_colour() {
        let mut pm = PauseMap::new([0.0, 0.0, 16.0, 9.0], Vec3::ZERO, 90);
        let pos = [-100., -100., 0., 100., -100., 0., 100., 100., 0., -100., 100., 0.];
        let idx = [0, 1, 2, 0, 2, 3];
        let uv = [0., 0., 1., 0., 1., 1., 0., 1.];
        let pyramid = Pyramid::new(
            2,
            1,
            image(2, 1, |x, _| if x == 0 { [255, 0, 0] } else { [0, 0, 255] }),
        );
        let paint = Paint::Texture {
            pyramid: &pyramid,
            uv: &uv,
            factor: [1.0; 3],
            wrap: PauseWrap::Clamp,
        };
        pm.draw(
            &pos,
            &idx,
            Vec3::ZERO,
            Vec3::UNIT_X,
            Vec3::UNIT_Y,
            Vec3::UNIT_Z,
            &paint,
        )
        .unwrap();
        let at = |pm: &PauseMap, x: usize| pm.rgba[(pm.h / 2) * pm.w + x];
        let (left, right) = (at(&pm, 0), at(&pm, pm.w - 1));
        // the left edge samples u ~0.3, between the texel centres: mostly red; the right edge is past the blue
        // texel's centre, clamped: blue
        assert!(left & 255 > 200 && (left >> 16) & 255 < 60, "{left:08x}");
        assert_eq!((right & 255, (right >> 16) & 255), (0, 255), "{right:08x}");
        // a flat quad a metre above: its colour (0xRRGGBB in, red in the low byte out), unshaded facing up
        let above = [-100., -100., 1., 100., -100., 1., 100., 100., 1., -100., 100., 1.];
        pm.draw(
            &above,
            &idx,
            Vec3::ZERO,
            Vec3::UNIT_X,
            Vec3::UNIT_Y,
            Vec3::UNIT_Z,
            &Paint::Flat(0x102030),
        )
        .unwrap();
        assert_eq!(at(&pm, 0), 0xFF30_2010);
    }
}
