//! The pause-menu map picture (ui/map_image.png, ui/map_image.json, ui/map_clouds.png): a top-down software
//! rasteriser drawing every triangle in its texture's mean colour, highest surface on top, shaded by slope.

use crate::Result;
use crate::records::MapImageJson;
use crate::vec::Sdx;

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

const PADDING: f32 = 1.1;
const MIN_HALF_HEIGHT: f32 = 25.0;

impl PauseMap {
    /// A front end's own picture (RGBA, row 0 north) covering `bounds` (ground frame, before the offset).
    pub fn given(w: usize, h: usize, rgba: &[u8], bounds: [f64; 4], off: Sdx) -> PauseMap {
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

    pub fn new(region: [f64; 4], off: Sdx, height: usize) -> PauseMap {
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

    /// Triangles whose vertex i is at basis * pos[i] + origin (map coordinates), in one colour.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, pos: &[f32], idx: &[u32], origin: Sdx, ex: Sdx, ey: Sdx, ez: Sdx, rgb: u32) {
        let (r, g, b) = (
            ((rgb >> 16) & 255) as f32,
            ((rgb >> 8) & 255) as f32,
            (rgb & 255) as f32,
        );
        let (w, h) = (self.w as i32, self.h as i32);
        for t in (0..idx.len().saturating_sub(2)).step_by(3) {
            let (i0, i1, i2) = (
                idx[t] as usize * 3,
                idx[t + 1] as usize * 3,
                idx[t + 2] as usize * 3,
            );
            let p0 = origin + ex * pos[i0] + ey * pos[i0 + 1] + ez * pos[i0 + 2];
            let p1 = origin + ex * pos[i1] + ey * pos[i1 + 1] + ez * pos[i1 + 2];
            let p2 = origin + ex * pos[i2] + ey * pos[i2 + 1] + ez * pos[i2 + 2];
            let (x0, y0) = ((p0.x - self.min_x) * self.sx, (self.max_y - p0.y) * self.sy);
            let (x1, y1) = ((p1.x - self.min_x) * self.sx, (self.max_y - p1.y) * self.sy);
            let (x2, y2) = ((p2.x - self.min_x) * self.sx, (self.max_y - p2.y) * self.sy);
            let mut bx0 = x0.min(x1.min(x2)).floor() as i32;
            let mut bx1 = x0.max(x1.max(x2)).ceil() as i32;
            let mut by0 = y0.min(y1.min(y2)).floor() as i32;
            let mut by1 = y0.max(y1.max(y2)).ceil() as i32;
            if bx1 < 0 || by1 < 0 || bx0 >= w || by0 >= h {
                continue;
            }
            let n = (p1 - p0).cross(p2 - p0);
            let nl = n.length();
            let shade = if nl > 1e-12 {
                0.62 + 0.38 * n.z.abs() / nl
            } else {
                1.0
            };
            let col =
                0xFF00_0000u32 | ((b * shade) as u32) << 16 | ((g * shade) as u32) << 8 | (r * shade) as u32;
            let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
            bx0 = bx0.max(0);
            by0 = by0.max(0);
            bx1 = bx1.min(w - 1);
            by1 = by1.min(h - 1);
            self.triangles += 1;
            if area.abs() < 1e-6 || (bx1 - bx0 <= 1 && by1 - by0 <= 1) {
                let px = ((x0 + x1 + x2) / 3.0) as i32;
                let py = ((y0 + y1 + y2) / 3.0) as i32;
                if (px as u32) < w as u32 && (py as u32) < h as u32 {
                    let z = (p0.z + p1.z + p2.z) / 3.0;
                    let o = py as usize * self.w + px as usize;
                    if z > self.depth[o] || (z == self.depth[o] && col > self.rgba[o]) {
                        self.depth[o] = z;
                        self.rgba[o] = col;
                    }
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
                    let z = p0.z * w0 + p1.z * w1 + p2.z * w2;
                    let o = py as usize * self.w + px as usize;
                    if z > self.depth[o] || (z == self.depth[o] && col > self.rgba[o]) {
                        self.depth[o] = z;
                        self.rgba[o] = col;
                    }
                }
            }
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
