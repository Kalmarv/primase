//! A source's local-light shadow atlas, as a mechanism: the layout the source states ([`LocalShadowAtlas`]) and the
//! placement of the lights a consumer chose to shadow in it ([`LocalShadowAtlas::place`]). Which lights get a shadow
//! is the consumer's choice, never the atlas's: it places exactly the lights it is given, in the order given, and
//! refuses, with the reason, what does not fit.
use serde::Serialize;

use super::{LocalShadow, LocalShadows};
use crate::Entity;
use crate::render_graph::{Region, TargetId};

/// One face of a light's shadow: a view from the light's centre along `forward`, `right` and `up` its view's x and y,
/// in the frame the source's shadowed programs compare positions in ([`LocalShadow::centre`]'s). Unit vectors, at
/// right angles, with `right x up = forward`: a point `p` of the face's view lies at
/// `(right . (p - c), up . (p - c), forward . (p - c))` in it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ShadowFace {
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

/// One depth projection term of a light's shadow record ([`LocalShadow::projection`]), of its faces' near plane `n` and
/// far plane `f` (the light's range).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowDepthTerm {
    /// `f / (f - n)`.
    FarOverDepthRange,
    /// `-n f / (f - n)`.
    NegatedNearFarOverDepthRange,
    /// A value the source's record holds whatever the light.
    Constant(f32),
}

/// The device depth a face's projection writes over the depth `z` along its forward axis, between its near plane `n`
/// and far plane `f`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowDepth {
    /// 1 at the near plane, 0 at the far: `n (f - z) / (z (f - n))`. Casters keep the greater depth.
    Reversed,
    /// 0 at the near plane, 1 at the far: `f (z - n) / (z (f - n))`. Casters keep the lesser depth.
    Standard,
}

/// The layout of a source's local-light shadow atlas, every value the source's: one depth target, square regions in
/// the order the source fills them, the faces each shadowed light takes in its record's order, and how a face
/// projects. A light takes `faces.len()` consecutive regions of `regions`, its first face the first of them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalShadowAtlas {
    /// The depth target the faces are drawn into ([`crate::render_graph::RenderGraph::targets`]).
    pub target: TargetId,
    /// The target's width and height in texels.
    pub size: [u32; 2],
    /// The side of a region in texels: a face's viewport.
    pub region: u32,
    /// How many texels across a region span the face's 90 degrees, centred: the rest is a guard band on each side
    /// that the face's projection covers past 90 degrees and the shadowed programs never sample.
    pub covered: u32,
    /// Each region's top-left texel, in the order the source fills them.
    pub regions: Vec<[u32; 2]>,
    /// A light's faces, in the order its record lists them.
    pub faces: Vec<ShadowFace>,
    /// A face's near plane over the light's range (its far plane).
    pub near_over_range: f32,
    pub depth: ShadowDepth,
    /// The terms of a light's record ([`LocalShadow::projection`]), in its order.
    pub projection: Vec<ShadowDepthTerm>,
    /// The fixed-function state the source draws the casters into the atlas with (its depth test, bias and culling),
    /// what a consumer that draws them with programs of its own takes.
    pub caster_state: iridia_interface::RenderState,
    /// What the source's layout rests on: which of its values were read from the frames and which inferred.
    pub why: String,
}

/// A light the consumer chose to shadow: its centre, in the frame the source's shadowed programs compare positions in,
/// and its range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowedLight {
    pub light: Entity,
    pub centre: [f32; 3],
    pub range: f32,
}

/// One face of a placed light: its region of the atlas and its camera, the view a consumer draws the light's shadow
/// casters from into that region.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShadowFaceView {
    pub light: Entity,
    /// The face's index in [`LocalShadowAtlas::faces`].
    pub face: u8,
    pub region: Region,
    /// World (the frame of [`ShadowedLight::centre`]) to the face's view, rows: `view * [p, 1]`.
    pub view: [[f32; 4]; 4],
    /// The face's view to clip, rows: `projection * [v, 1]`, its w the depth along the face's forward axis.
    pub projection: [[f32; 4]; 4],
    pub near: f32,
    pub far: f32,
}

/// The lights a consumer chose, placed: each one's shadow as its record states it ([`LocalShadows`], for
/// [`super::LightTable::pack`]), and every face's region and camera, light by light in the order given, each light's
/// faces in the atlas's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShadowPlacement {
    pub shadows: LocalShadows,
    pub faces: Vec<ShadowFaceView>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl LocalShadowAtlas {
    /// How many lights the atlas holds: whole sets of faces in its regions.
    pub fn capacity(&self) -> usize {
        self.regions.len().checked_div(self.faces.len()).unwrap_or(0)
    }

    /// The layout is whole: at least one face and one region; every region inside the target and none overlapping
    /// another; a guard band no wider than the region and as wide on both sides; each face a right-handed set of unit
    /// vectors at right angles; a near plane inside the range; at least one finite projection term; and a reason.
    pub fn verify(&self) -> Result<(), String> {
        let [width, height] = self.size;
        if width == 0 || height == 0 || self.region == 0 {
            return Err("a local shadow atlas with an empty target or region".into());
        }
        if self.covered == 0 || self.covered > self.region || !(self.region - self.covered).is_multiple_of(2)
        {
            return Err(format!(
                "a local shadow atlas covering {} of a region's {} texels: not a centred span inside it",
                self.covered, self.region
            ));
        }
        if self.faces.is_empty() || self.faces.len() > usize::from(u8::MAX) {
            return Err(format!(
                "a local shadow atlas of {} faces a light",
                self.faces.len()
            ));
        }
        if self.regions.is_empty() {
            return Err("a local shadow atlas with no regions".into());
        }
        for (i, [x, y]) in self.regions.iter().enumerate() {
            if x.checked_add(self.region).is_none_or(|r| r > width)
                || y.checked_add(self.region).is_none_or(|b| b > height)
            {
                return Err(format!(
                    "local shadow region {i} at ({x}, {y}) lies outside the {width} x {height} atlas"
                ));
            }
            for (j, [u, v]) in self.regions[..i].iter().enumerate() {
                if x.abs_diff(*u) < self.region && y.abs_diff(*v) < self.region {
                    return Err(format!("local shadow regions {j} and {i} overlap"));
                }
            }
        }
        for (i, f) in self.faces.iter().enumerate() {
            let unit = [f.forward, f.right, f.up]
                .iter()
                .all(|v| v.iter().all(|x| x.is_finite()) && (dot(*v, *v) - 1.0).abs() <= 1e-5);
            let square = dot(f.forward, f.right).abs() <= 1e-5
                && dot(f.forward, f.up).abs() <= 1e-5
                && dot(f.right, f.up).abs() <= 1e-5;
            let handed = cross(f.right, f.up)
                .iter()
                .zip(f.forward)
                .all(|(a, b)| (a - b).abs() <= 1e-5);
            if !(unit && square && handed) {
                return Err(format!(
                    "local shadow face {i} is not a right-handed set of unit vectors at right angles"
                ));
            }
        }
        if !(self.near_over_range > 0.0 && self.near_over_range < 1.0) {
            return Err(format!(
                "a local shadow near plane of {} times the range",
                self.near_over_range
            ));
        }
        if self.projection.is_empty()
            || self
                .projection
                .iter()
                .any(|t| matches!(t, ShadowDepthTerm::Constant(v) if !v.is_finite()))
        {
            return Err("a local shadow record with no projection terms, or one not finite".into());
        }
        self.caster_state
            .verify()
            .map_err(|e| format!("the local shadow casters' state: {e}"))?;
        if self.why.trim().is_empty() {
            return Err("a local shadow atlas layout with no reason".into());
        }
        Ok(())
    }

    /// Place `chosen`, in that order: light `i` takes regions `i * F ..` of [`LocalShadowAtlas::regions`] (`F` faces
    /// each). Refused, with the reason, where they do not fit (more lights than [`LocalShadowAtlas::capacity`]), where a
    /// light is given twice, or where its centre is not finite or its range not above 0. A face's view looks from the
    /// centre along the face's axis; its projection maps the `covered` texels to 90 degrees (the region's
    /// `region / covered` of the tangent to its edge) with the atlas's [`ShadowDepth`], near plane
    /// `range * near_over_range` and far plane `range`.
    pub fn place(&self, chosen: &[ShadowedLight]) -> Result<ShadowPlacement, String> {
        self.verify()?;
        if chosen.len() > self.capacity() {
            return Err(format!(
                "{} lights chosen for a local shadow atlas that holds {} ({} regions, {} faces a light)",
                chosen.len(),
                self.capacity(),
                self.regions.len(),
                self.faces.len()
            ));
        }
        let [width, height] = self.size.map(|s| s as f32);
        let half = self.covered as f32 / 2.0;
        let scale = self.covered as f32 / self.region as f32;
        let mut placed = ShadowPlacement::default();
        for (i, l) in chosen.iter().enumerate() {
            if chosen[..i].iter().any(|o| o.light == l.light) {
                return Err(format!(
                    "light {:?} chosen twice for the local shadow atlas",
                    l.light
                ));
            }
            if !l.centre.iter().all(|x| x.is_finite()) || !(l.range > 0.0 && l.range.is_finite()) {
                return Err(format!(
                    "light {:?}: a shadow centre {:?} and range {}",
                    l.light, l.centre, l.range
                ));
            }
            let far = l.range;
            let near = far * self.near_over_range;
            let depth = far - near;
            let mut faces = Vec::with_capacity(self.faces.len());
            for (k, f) in self.faces.iter().enumerate() {
                let [x, y] = self.regions[i * self.faces.len() + k];
                let row = |axis: [f32; 3]| [axis[0], axis[1], axis[2], -dot(axis, l.centre)];
                let (a, b) = match self.depth {
                    ShadowDepth::Reversed => (-near / depth, near * far / depth),
                    ShadowDepth::Standard => (far / depth, -near * far / depth),
                };
                placed.faces.push(ShadowFaceView {
                    light: l.light,
                    face: k as u8,
                    region: Region {
                        x,
                        y,
                        width: self.region,
                        height: self.region,
                    },
                    view: [row(f.right), row(f.up), row(f.forward), [0.0, 0.0, 0.0, 1.0]],
                    projection: [
                        [scale, 0.0, 0.0, 0.0],
                        [0.0, scale, 0.0, 0.0],
                        [0.0, 0.0, a, b],
                        [0.0, 0.0, 1.0, 0.0],
                    ],
                    near,
                    far,
                });
                // the face's 90 degrees in atlas coordinates: x right, y down from the region's centre
                let centre = self.region as f32 / 2.0;
                faces.push([
                    half / width,
                    -half / height,
                    (x as f32 + centre) / width,
                    (y as f32 + centre) / height,
                ]);
            }
            let projection = self
                .projection
                .iter()
                .map(|t| match t {
                    ShadowDepthTerm::FarOverDepthRange => far / depth,
                    ShadowDepthTerm::NegatedNearFarOverDepthRange => -(near * far) / depth,
                    ShadowDepthTerm::Constant(v) => *v,
                })
                .collect();
            placed.shadows.0.insert(
                l.light,
                LocalShadow {
                    projection,
                    centre: [l.centre[0], l.centre[1], l.centre[2], 1.0],
                    faces,
                },
            );
        }
        Ok(placed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six faces along the axes, each `right x up = forward`.
    fn cube() -> Vec<ShadowFace> {
        let face = |forward, right, up| ShadowFace { forward, right, up };
        vec![
            face([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
            face([-1., 0., 0.], [0., 0., 1.], [0., 1., 0.]),
            face([0., 1., 0.], [1., 0., 0.], [0., 0., -1.]),
            face([0., -1., 0.], [1., 0., 0.], [0., 0., 1.]),
            face([0., 0., 1.], [1., 0., 0.], [0., 1., 0.]),
            face([0., 0., -1.], [-1., 0., 0.], [0., 1., 0.]),
        ]
    }

    fn atlas(regions: Vec<[u32; 2]>) -> LocalShadowAtlas {
        LocalShadowAtlas {
            target: 0,
            size: [512, 512],
            region: 64,
            covered: 58,
            regions,
            faces: cube(),
            near_over_range: 0.001,
            depth: ShadowDepth::Reversed,
            projection: vec![
                ShadowDepthTerm::FarOverDepthRange,
                ShadowDepthTerm::NegatedNearFarOverDepthRange,
                ShadowDepthTerm::Constant(0.0),
                ShadowDepthTerm::Constant(1.0),
            ],
            caster_state: iridia_interface::RenderState::default(),
            why: "test".into(),
        }
    }

    fn grid() -> Vec<[u32; 2]> {
        (0..16).map(|i| [(i % 4) * 128, (i / 4) * 128]).collect()
    }

    fn mul(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
        m.map(|r| r.iter().zip(v).map(|(a, b)| a * b).sum())
    }

    /// A point on each face's axis projects to its region's centre at the depth the record's terms give it, and a
    /// point 45 degrees off the axis to the edge of the covered span: the record's place of a face and its camera
    /// agree texel for texel.
    #[test]
    fn a_face_camera_and_the_record_place_a_point_alike() {
        let a = atlas(grid());
        let light = crate::World::new().spawn();
        let c = [1.0, 2.0, -3.0];
        let p = a
            .place(&[ShadowedLight {
                light,
                centre: c,
                range: 8.0,
            }])
            .unwrap();
        let shadow = &p.shadows.0[&light];
        assert_eq!(shadow.centre, [1.0, 2.0, -3.0, 1.0]);
        assert_eq!(p.faces.len(), 6);
        for (f, face) in p.faces.iter().zip(&a.faces) {
            for (off, want) in [
                ([0.0, 0.0], [0.0, 0.0]),
                ([1.0, 0.0], [1.0, 0.0]),
                ([0.0, 1.0], [0.0, 1.0]),
            ] {
                let z = 4.0;
                let w: Vec<f32> = (0..3)
                    .map(|k| c[k] + z * (face.forward[k] + off[0] * face.right[k] + off[1] * face.up[k]))
                    .collect();
                let clip = mul(&f.projection, mul(&f.view, [w[0], w[1], w[2], 1.0]));
                let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
                // the viewport: x right, y down
                let px = f.region.x as f32 + (ndc[0] + 1.0) / 2.0 * 64.0;
                let py = f.region.y as f32 + (1.0 - ndc[1]) / 2.0 * 64.0;
                // what the shadowed program samples: the record's scale and offset of (u, v) / z
                let [sx, sy, ox, oy] = shadow.faces[f.face as usize];
                let ux = (ox + sx * want[0]) * 512.0;
                let uy = (oy + sy * want[1]) * 512.0;
                assert!(
                    (px - ux).abs() < 1e-3 && (py - uy).abs() < 1e-3,
                    "{f:?} {px} {py} {ux} {uy}"
                );
                // reversed depth from the record's terms: 1 - A - B / z
                let record = 1.0 - shadow.projection[0] - shadow.projection[1] / z;
                assert!((ndc[2] - record).abs() < 1e-6, "{} {record}", ndc[2]);
            }
        }
        assert_eq!(shadow.projection[2..], [0.0, 1.0]);
    }

    /// Lights take whole sets of faces in the order given; more than fit, a light given twice and a range of 0 are
    /// refused with the reason.
    #[test]
    fn the_atlas_places_what_it_is_given_and_refuses_what_does_not_fit() {
        let a = atlas(grid());
        assert_eq!(a.capacity(), 2);
        let mut w = crate::World::new();
        let e: Vec<Entity> = (0..4).map(|_| w.spawn()).collect();
        let l = |i: usize, range| ShadowedLight {
            light: e[i],
            centre: [0.0; 3],
            range,
        };
        let p = a.place(&[l(2, 4.0), l(1, 8.0)]).unwrap();
        assert_eq!(p.faces[6].light, e[1]);
        assert_eq!([p.faces[6].region.x, p.faces[6].region.y], [256, 128]);
        assert_eq!(p.faces[0].near, 4.0 * 0.001);
        let err = a.place(&[l(1, 8.0), l(2, 8.0), l(3, 8.0)]).unwrap_err();
        assert!(err.contains("holds 2"), "{err}");
        assert!(a.place(&[l(1, 8.0), l(1, 8.0)]).unwrap_err().contains("twice"));
        assert!(a.place(&[l(1, 0.0)]).is_err());
        assert!(a.place(&[]).unwrap().faces.is_empty());
    }

    /// Regions that overlap or leave the target, a guard band off centre and a face that is not right-handed are
    /// refused.
    #[test]
    fn a_broken_layout_is_refused() {
        assert!(
            atlas(vec![[0, 0], [32, 0]])
                .verify()
                .unwrap_err()
                .contains("overlap")
        );
        assert!(atlas(vec![[480, 0]]).verify().unwrap_err().contains("outside"));
        let mut odd = atlas(grid());
        odd.covered = 57;
        assert!(odd.verify().is_err());
        let mut left = atlas(grid());
        left.faces[0].right = [0.0, 0.0, 1.0];
        assert!(left.verify().unwrap_err().contains("right-handed"));
    }
}
