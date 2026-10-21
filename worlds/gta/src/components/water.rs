//! GTA's water as the reader keeps it: a water surface (`gta.water_surface`: a quad as stored, with its outline)
//! and the rest of its file (`gta.water`: the calming and wave quads), as the files store them.
use primase_core::{Component, JsonError};

use super::GTA;

/// A water quad, as stored (GTA ground coordinates, metres).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaterQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub z: f32,
    /// 0: the whole rectangle; 1-4: one of its corner triangles (see the module documentation).
    pub kind: u8,
    pub invisible: bool,
    pub limited_depth: bool,
    /// `a1`-`a4`, by corner.
    pub corners: [f32; 4],
    pub no_stencil: bool,
}

/// A calming quad: where waves are damped.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct CalmingQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub dampening: f32,
}

/// A wave quad: a wave's amplitude and direction over a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaveQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub amplitude: f32,
    pub direction: [f32; 2],
}

/// What a water file holds besides its quads (which are placed): its calming and wave quads, raw.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaWater {
    pub path: String,
    pub calming: Vec<CalmingQuad>,
    pub waves: Vec<WaveQuad>,
}

impl Component for GtaWater {
    const NAME: &'static str = "gta.water";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: the quads are placed (`gta.water_surface`), the tuning's falloff is theirs.
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A quad's outline (GTA ground coordinates), counter-clockwise seen from above.
pub fn outline(q: &WaterQuad) -> Vec<[f32; 2]> {
    let c = [
        [q.min[0], q.min[1]],
        [q.max[0], q.min[1]],
        [q.min[0], q.max[1]],
        [q.max[0], q.max[1]],
    ];
    let mut p: Vec<[f32; 2]> = match q.kind {
        0 => vec![c[0], c[1], c[3], c[2]],
        1 => vec![c[0], c[1], c[2]],
        2 => vec![c[0], c[3], c[2]],
        3 => vec![c[1], c[3], c[2]],
        _ => vec![c[0], c[1], c[3]],
    };
    if area(&p) < 0.0 {
        p.reverse();
    }
    p
}

/// Twice the signed area of a polygon (positive: counter-clockwise).
pub(crate) fn area(p: &[[f32; 2]]) -> f32 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// One water surface to draw: a water quad's outline (GTA ground coordinates, counter-clockwise from above; the whole
/// quad as read, clipped by `gta-select-water`), the quad as stored (its height, corner values and flags) and its
/// tuning's specular falloff; a prototype of its own, placed once by the reader and lowered by `lower-gta-water`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaWaterSurface {
    pub outline: Vec<[f32; 2]>,
    pub quad: WaterQuad,
    pub specular_falloff: Option<f32>,
}

impl Component for GtaWaterSurface {
    const NAME: &'static str = "gta.water_surface";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: the quad as stored (`quad`) instead of its height alone.
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl GtaWaterSurface {
    /// Its box in the core frame.
    pub fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        let mut b: Option<[[f64; 3]; 2]> = None;
        for p in &self.outline {
            let c = GTA.point([p[0], p[1], self.quad.z]).map(f64::from);
            let [lo, hi] = b.get_or_insert([c, c]);
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(kind: u8) -> WaterQuad {
        WaterQuad {
            min: [0.0, 0.0],
            max: [10.0, 20.0],
            z: 5.0,
            kind,
            invisible: false,
            limited_depth: false,
            corners: [26.0; 4],
            no_stencil: false,
        }
    }

    #[test]
    fn outlines_are_the_rectangle_or_its_corner_triangles_counter_clockwise() {
        assert_eq!(
            outline(&quad(0)),
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 20.0], [0.0, 20.0]]
        );
        // type 1: corners 0 1 2, the triangle without (maxX, maxY)
        let t = outline(&quad(1));
        assert_eq!(t.len(), 3);
        assert!(!t.contains(&[10.0, 20.0]));
        // type 3: corners 1 3 2, without (minX, minY)
        assert!(!outline(&quad(3)).contains(&[0.0, 0.0]));
        for k in 0..=4 {
            assert!(area(&outline(&quad(k))) > 0.0, "type {k} counter-clockwise");
        }
    }
}
