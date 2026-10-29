//! The scene's lights as a program reads them: a buffer of one record per light ([`super::BufferSource::Lights`]),
//! each record's fields by what they hold ([`LightValue`]), and the one packing of a world's `core.light`s into it
//! ([`LightTable::pack`]), so every consumer fills the table the same way.

use serde::Serialize;

use crate::components::{Light, LightKind, Transform};
use crate::light_unit::LightUnit;
use crate::world::{Entity, World};

/// Which lights a [`LightTable`] holds: one record per `core.light` of this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightTableKind {
    Point,
    Spot,
}

/// The scene's lights of one kind as a program reads them from a buffer: one record of `stride` bytes per `core.light` of
/// `kind`, in no order the programs rely on (they read how many there are from a
/// [`super::FrameParameter::LightCount`] constant and visit each), each holding `fields` as 32-bit floats. A byte no field
/// covers is read by no program of the source; a consumer writes zero there. The source's buffer holds at most
/// `capacity` records.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightTable {
    pub kind: LightTableKind,
    /// Bytes per record: the buffer's element size.
    pub stride: u32,
    /// The most records the source's buffer holds.
    pub capacity: u32,
    pub fields: Vec<LightField>,
}

/// One field of a light record: what it holds, starting at byte `offset` of the record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightField {
    pub offset: u32,
    pub value: LightValue,
}

/// What a field of a light record holds, from the light's `core.light`, its placement (`core.transform`: the light at
/// its origin, a spot shining along its local -Z with its cone's horizontal and vertical axes its local X and Y) and the
/// frame's view: "view space" is the space the frame's world-to-view matrix ([`super::ViewParameter::View`]) takes a
/// world position to, and a view-space depth is a position's z there.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LightValue {
    /// Three floats: the light's position in view space.
    ViewPosition,
    /// One float: the light's range, in world units (where its light stops).
    Range,
    /// One float: `1 / range^2`.
    InverseRangeSquared,
    /// Three floats: the light's colour times its intensity over the world's [`LightUnit::candela_per_unit`], the value
    /// the source stored. A program that reads a separate specular colour reads this too: a `core.light` lights both.
    Colour,
    /// Sixteen floats, four vectors of four at 16-byte steps: a spot's cone. The dot product of vector k with a view-space
    /// position `(x, y, z, 1)` is component k of the position in the cone's space: components 0 and 1 are the position's
    /// offsets along the cone's horizontal and vertical axes, over the tangent of the outer cone angle; component 3 is its
    /// distance along the beam from the light; component 2 equals component 3 (the cone has no near plane: a position is
    /// past it when its distance along the beam is above 0). Components 0 and 1 over component 3 lie on the unit circle at
    /// the cone's edge.
    ViewToCone,
    /// Four floats: a view-space sphere containing the light's volume, its centre then its radius. A point light's is
    /// its range's sphere. A spot's volume is the cone of height `range` along the beam whose base has radius
    /// `range * tan(outer)`: for an outer angle up to 45 degrees the smallest sphere through its apex and its base's rim
    /// (radius `range / (2 cos^2(outer))`, centred that far along the beam), above that the base's own sphere.
    ViewBoundingSphere,
    /// One float: the least view-space depth of the light's volume. A point light's is its position's less its range. A
    /// spot's is over its apex and the square of half-side `range * tan(outer)` across its horizontal and vertical axes at
    /// `range` along the beam (the base's corners).
    ViewDepthMin,
    /// One float: the greatest view-space depth of the light's volume, as [`LightValue::ViewDepthMin`].
    ViewDepthMax,
    /// One float the source's programs take as fixed, and what it means to them.
    Constant { value: f32, why: String },
    /// `floats` floats the programs read that `core.light` does not give: the source's name for them, and why. A consumer
    /// states the stand-in it writes ([`PackedLights::unknown`]: the packing writes 0).
    Unknown { floats: u8, name: String, why: String },
}

impl LightValue {
    /// The 32-bit floats the field takes.
    pub fn floats(&self) -> u32 {
        match self {
            LightValue::ViewPosition | LightValue::Colour => 3,
            LightValue::Range
            | LightValue::InverseRangeSquared
            | LightValue::ViewDepthMin
            | LightValue::ViewDepthMax
            | LightValue::Constant { .. } => 1,
            LightValue::ViewToCone => 16,
            LightValue::ViewBoundingSphere => 4,
            LightValue::Unknown { floats, .. } => u32::from(*floats),
        }
    }
}

/// A world's lights packed into a [`LightTable`]'s records.
#[derive(Clone, Debug, PartialEq)]
pub struct PackedLights {
    /// The records, `stride` bytes each, little-endian.
    pub bytes: Vec<u8>,
    /// The light of each record, in record order.
    pub lights: Vec<Entity>,
    /// Lights of the table's kind beyond its capacity, farthest from the view first, not packed.
    pub left_out: usize,
    /// The fields the packing wrote 0 into because `core.light` does not give them ([`LightValue::Unknown`]), by the
    /// source's name.
    pub unknown: Vec<String>,
}

type Vec3 = [f64; 3];

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: Vec3) -> Result<Vec3, String> {
    let n = dot(v, v).sqrt();
    if n > 0.0 && n.is_finite() {
        Ok(v.map(|x| x / n))
    } else {
        Err("a degenerate light axis".into())
    }
}

impl LightTable {
    /// Whether `light` is of the table's kind: a point light for a point table, a spot for a spot table.
    pub fn holds(&self, light: &Light) -> bool {
        matches!(
            (self.kind, light.kind),
            (LightTableKind::Point, LightKind::Point) | (LightTableKind::Spot, LightKind::Spot { .. })
        )
    }

    /// The table agrees with itself and with its buffer: a stride of whole floats that a structured buffer's elements
    /// equal (`element_stride`, `None` for a raw or typed buffer), a capacity, each field whole floats inside the record and
    /// no two overlapping, a spot's fields only in a spot table, a constant finite and every constant and unknown saying
    /// why.
    pub fn verify(&self, element_stride: Option<u32>) -> Result<(), String> {
        if self.stride == 0 || !self.stride.is_multiple_of(4) {
            return Err(format!(
                "a light record of {} bytes, not whole floats",
                self.stride
            ));
        }
        if let Some(element) = element_stride
            && element != self.stride
        {
            return Err(format!(
                "light records of {} bytes in a buffer of {element}-byte elements",
                self.stride
            ));
        }
        if self.capacity == 0 {
            return Err("a light table that holds no records".into());
        }
        let mut spans = Vec::with_capacity(self.fields.len());
        for f in &self.fields {
            let floats = f.value.floats();
            let end = f.offset.checked_add(floats * 4);
            if !f.offset.is_multiple_of(4) || floats == 0 || end.is_none_or(|end| end > self.stride) {
                return Err(format!(
                    "light field {:?} at {} ({floats} floats) is not inside the {}-byte record",
                    f.value, f.offset, self.stride
                ));
            }
            match &f.value {
                LightValue::ViewToCone if self.kind != LightTableKind::Spot => {
                    return Err("a cone in a table of point lights".into());
                }
                LightValue::Constant { value, why } if !value.is_finite() || why.trim().is_empty() => {
                    return Err(format!(
                        "light constant {value} at {} is not finite or says not why",
                        f.offset
                    ));
                }
                LightValue::Unknown { name, why, .. } if name.trim().is_empty() || why.trim().is_empty() => {
                    return Err(format!(
                        "unknown light field at {} has no name or says not why",
                        f.offset
                    ));
                }
                _ => {}
            }
            spans.push((f.offset, f.offset + floats * 4));
        }
        spans.sort_unstable();
        if let Some(pair) = spans.windows(2).find(|p| p[1].0 < p[0].1) {
            return Err(format!(
                "light fields overlap: bytes {}..{} and {}..{}",
                pair[0].0, pair[0].1, pair[1].0, pair[1].1
            ));
        }
        Ok(())
    }

    /// The world's lights of the table's kind as its records, seen from `view` (the frame's world-to-view matrix, by rows:
    /// view-space component r of a world position `p` is `view[r] . (p, 1)`; its rotation is taken as rigid). Each light
    /// is a `core.light` of the kind with a `core.transform`, a range and, for a spot, an outer cone below 90 degrees; the
    /// colours need the world's `core.light_unit`. Beyond `capacity` the lights nearest the view's origin are kept.
    pub fn pack(&self, w: &World, view: &[[f32; 4]; 4]) -> Result<PackedLights, String> {
        self.verify(None)?;
        let unit_value = w.resource::<LightUnit>().map(|u| f64::from(u.candela_per_unit));
        let row = |r: usize| view[r].map(f64::from);
        let to_view = |p: Vec3| -> Vec3 {
            [0, 1, 2].map(|r| {
                let v = row(r);
                v[0] * p[0] + v[1] * p[1] + v[2] * p[2] + v[3]
            })
        };
        let turn = |d: Vec3| -> Vec3 {
            [0, 1, 2].map(|r| {
                let v = row(r);
                v[0] * d[0] + v[1] * d[1] + v[2] * d[2]
            })
        };
        // the lights of the kind, nearest the view first
        let mut lights = Vec::new();
        for (e, light) in w.iter::<Light>() {
            if !self.holds(light) {
                continue;
            }
            let t = w
                .get::<Transform>(e)
                .ok_or_else(|| format!("{e:?}: a light without a core.transform"))?;
            let at = to_view(t.cols[3].map(f64::from));
            lights.push((dot(at, at), e, light, t, at));
        }
        lights.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let left_out = lights.len().saturating_sub(self.capacity as usize);
        lights.truncate(self.capacity as usize);
        let mut out = PackedLights {
            bytes: vec![0; lights.len() * self.stride as usize],
            lights: Vec::with_capacity(lights.len()),
            left_out,
            unknown: Vec::new(),
        };
        for f in &self.fields {
            if let LightValue::Unknown { name, .. } = &f.value {
                out.unknown.push(name.clone());
            }
        }
        for (n, (_, e, light, t, at)) in lights.into_iter().enumerate() {
            let range = light
                .range
                .map(f64::from)
                .filter(|r| r.is_finite() && *r > 0.0)
                .ok_or_else(|| format!("{e:?}: a light with no range, which a light record holds"))?;
            // a spot's axes in view space and the tangent of its outer cone
            let cone = match light.kind {
                LightKind::Spot { outer_cone, .. } => {
                    let tan = f64::from(outer_cone).tan();
                    if !(tan.is_finite() && tan > 0.0) {
                        return Err(format!(
                            "{e:?}: a spot's outer cone {outer_cone} has no tangent above 0"
                        ));
                    }
                    let axis = |c: usize| unit(turn(t.cols[c].map(f64::from)));
                    let beam = axis(2)?.map(|x| -x);
                    Some((axis(0)?, axis(1)?, beam, tan, f64::from(outer_cone)))
                }
                _ => None,
            };
            let mut values: Vec<(u32, Vec<f64>)> = Vec::with_capacity(self.fields.len());
            for f in &self.fields {
                let v: Vec<f64> = match &f.value {
                    LightValue::ViewPosition => at.to_vec(),
                    LightValue::Range => vec![range],
                    LightValue::InverseRangeSquared => vec![1.0 / (range * range)],
                    LightValue::Colour => {
                        let unit = unit_value.ok_or(
                            "the world's lights have no core.light_unit: a light record's colour has no value",
                        )?;
                        light
                            .color
                            .iter()
                            .map(|c| f64::from(*c) * f64::from(light.intensity) / unit)
                            .collect()
                    }
                    LightValue::ViewToCone => {
                        let (right, up, beam, tan, _) = cone.ok_or("a cone for a light that is no spot")?;
                        let mut m = Vec::with_capacity(16);
                        for (axis, scale) in [(right, 1.0 / tan), (up, 1.0 / tan), (beam, 1.0), (beam, 1.0)] {
                            m.extend(axis.map(|x| x * scale));
                            m.push(-dot(axis, at) * scale);
                        }
                        m
                    }
                    LightValue::ViewBoundingSphere => match cone {
                        None => vec![at[0], at[1], at[2], range],
                        Some((_, _, beam, tan, outer)) => {
                            let radius = if outer <= std::f64::consts::FRAC_PI_4 {
                                range / (2.0 * outer.cos().powi(2))
                            } else {
                                range * tan
                            };
                            let along = if outer <= std::f64::consts::FRAC_PI_4 {
                                radius
                            } else {
                                range
                            };
                            vec![
                                at[0] + beam[0] * along,
                                at[1] + beam[1] * along,
                                at[2] + beam[2] * along,
                                radius,
                            ]
                        }
                    },
                    LightValue::ViewDepthMin | LightValue::ViewDepthMax => {
                        let (least, most) = match cone {
                            None => (at[2] - range, at[2] + range),
                            Some((right, up, beam, tan, _)) => {
                                let base = at[2] + beam[2] * range;
                                let reach = range * tan * (right[2].abs() + up[2].abs());
                                (at[2].min(base - reach), at[2].max(base + reach))
                            }
                        };
                        vec![if f.value == LightValue::ViewDepthMin {
                            least
                        } else {
                            most
                        }]
                    }
                    LightValue::Constant { value, .. } => vec![f64::from(*value)],
                    LightValue::Unknown { floats, .. } => vec![0.0; usize::from(*floats)],
                };
                values.push((f.offset, v));
            }
            let record = &mut out.bytes[n * self.stride as usize..(n + 1) * self.stride as usize];
            for (offset, v) in values {
                for (k, x) in v.into_iter().enumerate() {
                    let x = x as f32;
                    if !x.is_finite() {
                        return Err(format!("{e:?}: a light record value at {offset} is not finite"));
                    }
                    let byte = offset as usize + 4 * k;
                    record[byte..byte + 4].copy_from_slice(&x.to_le_bytes());
                }
            }
            out.lights.push(e);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    fn floats(bytes: &[u8]) -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }

    fn close(got: f32, want: f64, what: &str) {
        assert!(
            (f64::from(got) - want).abs() <= 1e-5 * want.abs().max(1.0),
            "{what}: {got}, worked by hand {want}"
        );
    }

    /// The view: world x stays x, world y stays y, world z becomes -z, then 10 is added to z (a camera at world z 10
    /// looking down -z).
    const VIEW: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 10.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    fn table(kind: LightTableKind, stride: u32, fields: Vec<(u32, LightValue)>) -> LightTable {
        LightTable {
            kind,
            stride,
            capacity: 4,
            fields: fields
                .into_iter()
                .map(|(offset, value)| LightField { offset, value })
                .collect(),
        }
    }

    /// A point light at world (1, 2, 3), colour (0.5, 0.25, 1), intensity 8, range 4, in a world whose unit is 2 candela:
    /// view position (1, 2, 7), colour (2, 1, 4), 1 / 16, range 4, a constant 0 and an unknown pair written 0; a second
    /// point light farther away is left out by a capacity of 1.
    #[test]
    fn a_point_light_packs_into_its_record() {
        let mut w = World::new();
        let place = |w: &mut World, at: [f32; 3]| {
            let e = w.spawn();
            w.insert(
                e,
                Transform {
                    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], at],
                },
            );
            w.insert(
                e,
                Light {
                    kind: LightKind::Point,
                    color: [0.5, 0.25, 1.0],
                    intensity: 8.0,
                    range: Some(4.0),
                    cast_shadows: None,
                    radius: None,
                },
            );
            e
        };
        let near = place(&mut w, [1., 2., 3.]);
        place(&mut w, [1., 2., -30.]);
        let mut t = table(
            LightTableKind::Point,
            48,
            vec![
                (0, LightValue::ViewPosition),
                (12, LightValue::InverseRangeSquared),
                (16, LightValue::Colour),
                (
                    28,
                    LightValue::Unknown {
                        floats: 2,
                        name: "Wrap".into(),
                        why: "test".into(),
                    },
                ),
                (36, LightValue::Range),
                (
                    40,
                    LightValue::Constant {
                        value: 0.0,
                        why: "test".into(),
                    },
                ),
            ],
        );
        // no unit: the colour has no value
        assert!(t.pack(&w, &VIEW).is_err());
        w.insert_resource(LightUnit {
            candela_per_unit: 2.0,
        });
        t.capacity = 1;
        let packed = t.pack(&w, &VIEW).unwrap();
        assert_eq!((packed.lights.clone(), packed.left_out), (vec![near], 1));
        assert_eq!(packed.unknown, vec!["Wrap".to_owned()]);
        let f = floats(&packed.bytes);
        let want = [1.0, 2.0, 7.0, 1.0 / 16.0, 2.0, 1.0, 4.0, 0.0, 0.0, 4.0, 0.0];
        for (i, v) in want.into_iter().enumerate() {
            close(f[i], v, &format!("float {i}"));
        }
        assert_eq!(f.len(), 12);
        assert_eq!(f[11], 0.0, "a byte no field covers is zero");
    }

    /// A spot at world (0, 0, 0) with an identity placement, so its beam is world -Z (view +Z, from the camera at
    /// view depth 10), outer cone 30 degrees, range 6: the cone's vectors are worked below, axis by axis; its
    /// sphere for an outer angle of 30 degrees has radius 6 / (2 cos^2 30) = 4 and centre 4 along the beam (view depth
    /// 14); its depth range runs from the apex (10) to the base (16) plus 6 tan 30 (|x.z| + |y.z| = 0, so 16).
    #[test]
    fn a_spot_packs_its_cone_sphere_and_depths() {
        let mut w = World::new();
        w.insert_resource(LightUnit {
            candela_per_unit: 1.0,
        });
        let e = w.spawn();
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]],
            },
        );
        w.insert(
            e,
            Light {
                kind: LightKind::Spot {
                    inner_cone: 0.4,
                    outer_cone: std::f32::consts::FRAC_PI_6,
                },
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                range: Some(6.0),
                cast_shadows: None,
                radius: None,
            },
        );
        let t = table(
            LightTableKind::Spot,
            112,
            vec![
                (0, LightValue::ViewToCone),
                (64, LightValue::ViewBoundingSphere),
                (80, LightValue::ViewDepthMin),
                (84, LightValue::ViewDepthMax),
            ],
        );
        let f = floats(&t.pack(&w, &VIEW).unwrap().bytes);
        let cot = 1.0 / (std::f64::consts::FRAC_PI_6).tan();
        // view axes: right (1, 0, 0), up (0, 1, 0), beam: world -Z turned by the view is (0, 0, 1); apex at (0, 0, 10)
        let want = [
            cot, 0.0, 0.0, 0.0, // right / tan, less right . apex (0)
            0.0, cot, 0.0, 0.0, // up / tan
            0.0, 0.0, 1.0, -10.0, // beam, less beam . apex
            0.0, 0.0, 1.0, -10.0, // beam again: no near plane
            0.0, 0.0, 14.0, 4.0, // sphere
            10.0, 16.0, // depths
        ];
        for (i, v) in want.into_iter().enumerate() {
            close(f[i], v, &format!("float {i}"));
        }
        // a position on the cone's edge, 3 along the beam and 3 tan 30 across: (x / w)^2 + (y / w)^2 = 1
        let p = [3.0 * (std::f64::consts::FRAC_PI_6).tan(), 0.0, 13.0, 1.0];
        let row = |r: usize| (0..4).map(|k| f64::from(f[4 * r + k]) * p[k]).sum::<f64>();
        close((row(0) / row(3)) as f32, 1.0, "the cone's edge");
    }

    /// A table refuses a record of no whole floats, a buffer of another stride, fields outside the record or over one
    /// another, a cone for point lights and a constant or unknown that says not why.
    #[test]
    fn a_light_table_agrees_with_itself() {
        let ok = table(LightTableKind::Point, 16, vec![(0, LightValue::ViewPosition)]);
        assert_eq!(ok.verify(Some(16)), Ok(()));
        assert!(ok.verify(Some(32)).is_err());
        assert!(table(LightTableKind::Point, 14, vec![]).verify(None).is_err());
        assert!(
            table(LightTableKind::Point, 16, vec![(8, LightValue::ViewPosition)])
                .verify(None)
                .is_err()
        );
        assert!(
            table(
                LightTableKind::Point,
                16,
                vec![(0, LightValue::ViewPosition), (8, LightValue::Range)]
            )
            .verify(None)
            .is_err()
        );
        assert!(
            table(LightTableKind::Point, 64, vec![(0, LightValue::ViewToCone)])
                .verify(None)
                .is_err()
        );
        assert!(
            table(
                LightTableKind::Point,
                16,
                vec![(
                    0,
                    LightValue::Constant {
                        value: 0.0,
                        why: " ".into()
                    }
                )]
            )
            .verify(None)
            .is_err()
        );
    }
}
