//! The scene's lights as a program reads them: a buffer of one record per light ([`super::BufferSource::Lights`]),
//! each record's fields by what they hold ([`LightValue`]), and the one packing of a world's `core.light`s into it
//! ([`LightTable::pack`]), so every consumer fills the table the same way.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::components::{Light, LightKind, Transform};
use crate::light_unit::LightUnit;
use crate::world::{Entity, World};

/// Which lights a [`LightTable`] holds: one record per `core.light` of this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightTableKind {
    Point,
    Spot,
}

impl LightTableKind {
    /// Whether `light` is of this kind: a point light for points, a spot for spots.
    pub fn holds(self, light: &Light) -> bool {
        matches!(
            (self, light.kind),
            (LightTableKind::Point, LightKind::Point) | (LightTableKind::Spot, LightKind::Spot { .. })
        )
    }
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
    /// One float: `1 / core.light_falloff.distance`, a source-stated normalization of distance, independent of
    /// emitter radius and light range. A light with no stated falloff keeps zero and is named in `PackedLights::unknown`.
    InverseFalloffDistance,
    /// The light's source-stated transmission amplitude (`core.light_translucency.strength`).
    TranslucencyStrength,
    /// Multiplier of the source shader's angular exponent (`core.light_translucency.exponent_scale`).
    TranslucencyExponentScale,
    /// Three floats: the light's colour times its intensity over the world's [`LightUnit::candela_per_unit`], the value
    /// the source stored.
    Colour,
    /// Three floats: the light's specular colour times its specular intensity over the world's
    /// [`LightUnit::candela_per_unit`] ([`Light::specular`]); for a light that states no specular of its own, its colour
    /// as [`LightValue::Colour`] packs it (counted: [`PackedLights::specular_from_colour`]).
    SpecularColour,
    /// Sixteen floats, four vectors of four at 16-byte steps: a spot's cone. The dot product of vector k with a view-space
    /// position `(x, y, z, 1)` is component k of the position in the cone's space: components 0 and 1 are the position's
    /// offsets along the cone's horizontal and vertical axes, each over the tangent of its axis's half-angle
    /// ([`crate::SpotFrustum`]'s, else the outer cone's for both); component 3 is its distance `d` along the beam from
    /// the light; component 2 is `(d - near) * range / (range - near)` for the frustum's near plane (`d` itself for a
    /// light with none), above 0 past it. For a circular cone, components 0 and 1 over component 3 lie on the unit circle
    /// at the cone's edge.
    ViewToCone,
    /// Four floats: a view-space sphere containing the light's volume, its centre then its radius. A point light's is
    /// its range's sphere. A spot's volume is the cone of height `range` along the beam whose base reaches
    /// `range * tan(outer)` from the beam (for a pyramid, [`crate::SpotFrustum`], `outer` is the angle of its base's
    /// corners, `atan(sqrt(tan^2 h + tan^2 v))`): for an outer angle up to 45 degrees the smallest sphere through its apex
    /// and its base's rim (radius `range / (2 cos^2(outer))`, centred that far along the beam), above that the base's own
    /// sphere.
    ViewBoundingSphere,
    /// Four floats: the view-space sphere through a spot's apex and the rim of the circular cone of its vertical
    /// half-angle `v` ([`crate::SpotFrustum::vertical_half_angle`], else its outer cone) at `range` along the beam:
    /// centre `a = range / (2 cos^2 v)` along the beam, radius `a + margin`. It contains the light's volume only where the
    /// horizontal half-angle is not the wider: a source whose light lists cull with such a sphere states it.
    VerticalConeSphere { margin: f32 },
    /// One float: the least view-space depth of the light's volume. A point light's is its position's less its range. A
    /// spot's is over its apex and its base's corners, `range` along the beam and `range * tan` of each axis's half-angle
    /// across its horizontal and vertical axes ([`crate::SpotFrustum`]'s, else the outer cone's for both).
    ViewDepthMin,
    /// One float: the greatest view-space depth of the light's volume, as [`LightValue::ViewDepthMin`].
    ViewDepthMax,
    /// One float: whether the consumer draws the light with a local shadow this frame ([`LocalShadows`]), 1 where it
    /// does and 0 where it does not. A source's programs compare it as a float: a light-organize program lists the
    /// light for the unshadowed tile pass only where it is 0, a shadowed program samples the light's shadow only where it
    /// is above 0.
    Shadowed,
    /// `floats` floats: the depth projection terms of the light's shadow ([`LocalShadow::projection`]), 0 for a light
    /// drawn without one.
    ShadowProjection { floats: u8 },
    /// Four floats: the point the light's shadow is rendered from ([`LocalShadow::centre`]), 0 for a light drawn without
    /// one.
    ShadowCentre,
    /// `faces` groups of four floats: where each face of the light's shadow lies in the atlas ([`LocalShadow::faces`]),
    /// 0 for a light drawn without one.
    ShadowFaces { faces: u8 },
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
            LightValue::ViewPosition | LightValue::Colour | LightValue::SpecularColour => 3,
            LightValue::Range
            | LightValue::InverseRangeSquared
            | LightValue::InverseFalloffDistance
            | LightValue::TranslucencyStrength
            | LightValue::TranslucencyExponentScale
            | LightValue::ViewDepthMin
            | LightValue::ViewDepthMax
            | LightValue::Shadowed
            | LightValue::Constant { .. } => 1,
            LightValue::ViewToCone => 16,
            LightValue::ViewBoundingSphere
            | LightValue::VerticalConeSphere { .. }
            | LightValue::ShadowCentre => 4,
            LightValue::ShadowProjection { floats } | LightValue::Unknown { floats, .. } => {
                u32::from(*floats)
            }
            LightValue::ShadowFaces { faces } => 4 * u32::from(*faces),
        }
    }

    /// Whether the field is part of a shadowed light's place in the atlas ([`LocalShadow`]).
    fn is_shadow_placement(&self) -> bool {
        matches!(
            self,
            LightValue::ShadowProjection { .. } | LightValue::ShadowCentre | LightValue::ShadowFaces { .. }
        )
    }
}

/// A light's shadow as the consumer draws it this frame, in the terms the source's shadowed programs read: its place in
/// the consumer's local-light shadow atlas. The consumer that renders the atlas states every value; the packing derives
/// none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalShadow {
    /// The depth projection terms of the shadow's faces, at least as many as the table's
    /// [`LightValue::ShadowProjection`] takes.
    pub projection: Vec<f32>,
    /// The point the shadow is rendered from, in the frame the source's programs take a view-space position to with the
    /// frame's view-to-world matrix, then 1.
    pub centre: [f32; 4],
    /// Each face's scale (x, y) and offset (z, w) in the atlas, at least as many as the table's
    /// [`LightValue::ShadowFaces`] takes, in its order.
    pub faces: Vec<[f32; 4]>,
}

/// The lights the consumer draws with a local shadow this frame, each with its shadow ([`LightTable::pack`]). Empty
/// ([`LocalShadows::none`]): the consumer shadows no light, and every light packs unshadowed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalShadows(pub BTreeMap<Entity, LocalShadow>);

impl LocalShadows {
    /// No light shadowed.
    pub fn none() -> Self {
        Self::default()
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
    /// Packed lights whose [`LightValue::SpecularColour`] fields took their colour, the light stating no specular of its
    /// own (0 where the table has no such field).
    pub specular_from_colour: usize,
    /// The records packed with their light's shadow ([`LocalShadows`]), by index, in record order: the light lists take
    /// them ([`pack_light_lists`]).
    pub shadowed: Vec<u32>,
    /// Packed lights that cast shadows (`core.light`'s `cast_shadows` is `Some(true)`) packed without one: the consumer
    /// draws them unshadowed and reports them. Every such light where the consumer shadows none.
    pub unshadowed_casters: Vec<Entity>,
}

type Vec3 = [f64; 3];

/// A spot's shape in view space, as the packing takes it: its axes, the tangents of its half-angles, its vertical
/// half-angle, the angle of its widest reach from the beam and its near plane.
struct Cone {
    right: Vec3,
    up: Vec3,
    beam: Vec3,
    tan_horizontal: f64,
    tan_vertical: f64,
    vertical: f64,
    widest: f64,
    near: f64,
}

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
        self.kind.holds(light)
    }

    /// The table agrees with itself and with its buffer: a stride of whole floats that a structured buffer's elements
    /// equal (`element_stride`, `None` for a raw or typed buffer), a capacity, each field whole floats inside the record and
    /// no two overlapping, a spot's fields only in a spot table, a constant finite and every constant and unknown saying
    /// why, and a shadow's place only in a table that says whether the light is shadowed ([`LightValue::Shadowed`]).
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
                LightValue::ViewToCone | LightValue::VerticalConeSphere { .. }
                    if self.kind != LightTableKind::Spot =>
                {
                    return Err("a cone in a table of point lights".into());
                }
                LightValue::VerticalConeSphere { margin } if !(margin.is_finite() && *margin >= 0.0) => {
                    return Err(format!(
                        "a cone sphere's margin {margin} is not finite and non-negative"
                    ));
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
            if f.value.is_shadow_placement() && !self.reads_shadowed() {
                return Err(format!(
                    "light field {:?} at {} places a shadow in a table that does not say which lights are shadowed",
                    f.value, f.offset
                ));
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

    /// Whether the records say which lights are shadowed ([`LightValue::Shadowed`]).
    pub fn reads_shadowed(&self) -> bool {
        self.fields.iter().any(|f| f.value == LightValue::Shadowed)
    }

    /// The world's lights of the table's kind as its records, seen from `view` (the frame's world-to-view matrix, by rows:
    /// view-space component r of a world position `p` is `view[r] . (p, 1)`; its rotation is taken as rigid). Each light
    /// is a `core.light` of the kind with a `core.transform`, a range and, for a spot, an outer cone below 90 degrees; the
    /// colours need the world's `core.light_unit`. A spot's `core.spot_frustum`, where it has one, is its shape: its
    /// two half-angles and near plane take the place of the outer cone. Beyond `capacity` the lights nearest the view's
    /// origin are kept.
    ///
    /// `shadows` are the lights the consumer draws with a local shadow this frame ([`LocalShadows::none`] where it
    /// draws none): in a table that says which lights are shadowed, such a light's record takes [`LightValue::Shadowed`]
    /// 1 and its shadow's place, every other record 0 and zeros. Refused: a shadow for a light whose `core.light` casts
    /// none (`cast_shadows` `Some(false)`), and a shadow with fewer projection terms or faces than the record holds.
    pub fn pack(
        &self,
        w: &World,
        view: &[[f32; 4]; 4],
        shadows: &LocalShadows,
    ) -> Result<PackedLights, String> {
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
            specular_from_colour: 0,
            shadowed: Vec::new(),
            unshadowed_casters: Vec::new(),
        };
        let reads_shadowed = self.reads_shadowed();
        let reads_specular = self.fields.iter().any(|f| f.value == LightValue::SpecularColour);
        for f in &self.fields {
            if let LightValue::Unknown { name, .. } = &f.value {
                out.unknown.push(name.clone());
            }
        }
        let colour = |color: [f32; 3], intensity: f32| -> Result<Vec<f64>, String> {
            let unit = unit_value
                .ok_or("the world's lights have no core.light_unit: a light record's colour has no value")?;
            Ok(color
                .iter()
                .map(|c| f64::from(*c) * f64::from(intensity) / unit)
                .collect())
        };
        for (n, (_, e, light, t, at)) in lights.into_iter().enumerate() {
            if reads_specular && light.specular.is_none() {
                out.specular_from_colour += 1;
            }
            let shadow = shadows.0.get(&e).filter(|_| reads_shadowed);
            if shadow.is_some() && light.cast_shadows == Some(false) {
                return Err(format!("{e:?}: a shadow for a light that casts none"));
            }
            if shadow.is_some() {
                out.shadowed.push(n as u32);
            } else if light.cast_shadows == Some(true) {
                out.unshadowed_casters.push(e);
            }
            let range = light
                .range
                .map(f64::from)
                .filter(|r| r.is_finite() && *r > 0.0)
                .ok_or_else(|| format!("{e:?}: a light with no range, which a light record holds"))?;
            // a spot's axes in view space and its shape: the pyramid its source states, else its outer cone on both axes
            let cone = match light.kind {
                LightKind::Spot { outer_cone, .. } => {
                    let frustum = w.get::<crate::SpotFrustum>(e);
                    if let Some(f) = frustum {
                        <crate::SpotFrustum as crate::Component>::verify(f, e, w)?;
                    }
                    let outer = f64::from(outer_cone);
                    let (horizontal, vertical, near) = frustum.map_or((outer, outer, 0.0), |f| {
                        (
                            f64::from(f.horizontal_half_angle),
                            f64::from(f.vertical_half_angle),
                            f64::from(f.near),
                        )
                    });
                    let (tan_horizontal, tan_vertical) = (horizontal.tan(), vertical.tan());
                    for tan in [tan_horizontal, tan_vertical] {
                        if !(tan.is_finite() && tan > 0.0) {
                            return Err(format!(
                                "{e:?}: a spot's cone {horizontal} by {vertical} has no tangent above 0"
                            ));
                        }
                    }
                    if near >= range {
                        return Err(format!(
                            "{e:?}: a spot's near plane {near} not before its range {range}"
                        ));
                    }
                    let axis = |c: usize| unit(turn(t.cols[c].map(f64::from)));
                    let beam = axis(2)?.map(|x| -x);
                    Some(Cone {
                        right: axis(0)?,
                        up: axis(1)?,
                        beam,
                        tan_horizontal,
                        tan_vertical,
                        vertical,
                        // the angle of the volume's widest reach from the beam: a pyramid's corners
                        widest: match frustum {
                            Some(_) => tan_horizontal.hypot(tan_vertical).atan(),
                            None => outer,
                        },
                        near,
                    })
                }
                _ => None,
            };
            let mut values: Vec<(u32, Vec<f64>)> = Vec::with_capacity(self.fields.len());
            for f in &self.fields {
                let v: Vec<f64> = match &f.value {
                    LightValue::ViewPosition => at.to_vec(),
                    LightValue::Range => vec![range],
                    LightValue::InverseRangeSquared => vec![1.0 / (range * range)],
                    LightValue::InverseFalloffDistance => match w.get::<crate::LightFalloff>(e) {
                        Some(falloff) => vec![f64::from(
                            falloff
                                .inverse_distance()
                                .map_err(|why| format!("{e:?}: {why}"))?,
                        )],
                        None => {
                            let name = w
                                .get::<crate::Name>(e)
                                .map(|n| n.0.as_str())
                                .or_else(|| w.get::<crate::Source>(e).map(|s| s.0.as_str()));
                            out.unknown.push(format!("light {e:?}{}: source falloff distance not stated; inverse falloff distance supplied as zero",
                                name.map(|n| format!(" ({n})")).unwrap_or_default()));
                            vec![0.]
                        }
                    },
                    LightValue::TranslucencyStrength | LightValue::TranslucencyExponentScale => {
                        let source = w
                            .get::<crate::LightTranslucency>(e)
                            .ok_or_else(|| format!("{e:?}: light translucency parameters are not stated"))?;
                        <crate::LightTranslucency as crate::Component>::verify(source, e, w)?;
                        vec![f64::from(if f.value == LightValue::TranslucencyStrength {
                            source.strength
                        } else {
                            source.exponent_scale
                        })]
                    }
                    LightValue::Colour => colour(light.color, light.intensity)?,
                    LightValue::SpecularColour => match &light.specular {
                        Some(s) => colour(s.color, s.intensity)?,
                        None => colour(light.color, light.intensity)?,
                    },
                    LightValue::ViewToCone => {
                        let c = cone.as_ref().ok_or("a cone for a light that is no spot")?;
                        // the near plane's row: (d - near) * range / (range - near), d itself where near is 0
                        let depth = range / (range - c.near);
                        let mut m = Vec::with_capacity(16);
                        for (axis, scale, shift) in [
                            (c.right, 1.0 / c.tan_horizontal, 0.0),
                            (c.up, 1.0 / c.tan_vertical, 0.0),
                            (c.beam, depth, c.near),
                            (c.beam, 1.0, 0.0),
                        ] {
                            m.extend(axis.map(|x| x * scale));
                            m.push((-dot(axis, at) - shift) * scale);
                        }
                        m
                    }
                    LightValue::ViewBoundingSphere => match &cone {
                        None => vec![at[0], at[1], at[2], range],
                        Some(c) => {
                            let outer = c.widest;
                            let radius = if outer <= std::f64::consts::FRAC_PI_4 {
                                range / (2.0 * outer.cos().powi(2))
                            } else {
                                range * outer.tan()
                            };
                            let along = if outer <= std::f64::consts::FRAC_PI_4 {
                                radius
                            } else {
                                range
                            };
                            vec![
                                at[0] + c.beam[0] * along,
                                at[1] + c.beam[1] * along,
                                at[2] + c.beam[2] * along,
                                radius,
                            ]
                        }
                    },
                    LightValue::VerticalConeSphere { margin } => {
                        let c = cone.as_ref().ok_or("a cone sphere for a light that is no spot")?;
                        let along = range / (2.0 * c.vertical.cos().powi(2));
                        vec![
                            at[0] + c.beam[0] * along,
                            at[1] + c.beam[1] * along,
                            at[2] + c.beam[2] * along,
                            along + f64::from(*margin),
                        ]
                    }
                    LightValue::ViewDepthMin | LightValue::ViewDepthMax => {
                        let (least, most) = match &cone {
                            None => (at[2] - range, at[2] + range),
                            Some(c) => {
                                let base = at[2] + c.beam[2] * range;
                                let reach = range
                                    * (c.tan_horizontal * c.right[2].abs() + c.tan_vertical * c.up[2].abs());
                                (at[2].min(base - reach), at[2].max(base + reach))
                            }
                        };
                        vec![if f.value == LightValue::ViewDepthMin {
                            least
                        } else {
                            most
                        }]
                    }
                    LightValue::Shadowed => vec![if shadow.is_some() { 1.0 } else { 0.0 }],
                    LightValue::ShadowProjection { floats } => match shadow {
                        Some(s) => s
                            .projection
                            .get(..usize::from(*floats))
                            .ok_or_else(|| {
                                format!("{e:?}: a shadow with fewer than {floats} projection terms")
                            })?
                            .iter()
                            .map(|x| f64::from(*x))
                            .collect(),
                        None => vec![0.0; usize::from(*floats)],
                    },
                    LightValue::ShadowCentre => match shadow {
                        Some(s) => s.centre.map(f64::from).to_vec(),
                        None => vec![0.0; 4],
                    },
                    LightValue::ShadowFaces { faces } => match shadow {
                        Some(s) => s
                            .faces
                            .get(..usize::from(*faces))
                            .ok_or_else(|| format!("{e:?}: a shadow with fewer than {faces} faces"))?
                            .iter()
                            .flatten()
                            .map(|x| f64::from(*x))
                            .collect(),
                        None => vec![0.0; 4 * usize::from(*faces)],
                    },
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

/// Where a value lies in a light list's record: `bits` bits from bit `shift` (0 the lowest) of the little-endian 32-bit
/// unsigned word at byte `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ListBits {
    pub offset: u32,
    pub shift: u8,
    pub bits: u8,
}

impl ListBits {
    /// The largest value the bits hold.
    pub fn max(&self) -> u32 {
        if self.bits >= 32 {
            u32::MAX
        } else {
            (1u32 << self.bits) - 1
        }
    }

    fn mask(&self) -> u32 {
        self.max() << self.shift
    }

    fn verify(&self, stride: u32, what: &str) -> Result<(), String> {
        let end = u32::from(self.shift) + u32::from(self.bits);
        if self.bits == 0 || end > 32 || !self.offset.is_multiple_of(4) || self.offset + 4 > stride {
            return Err(format!(
                "{what}: bits {}..{end} of the word at {} are not inside a {stride}-byte record",
                self.shift, self.offset
            ));
        }
        Ok(())
    }

    fn write(&self, record: &mut [u8], value: u32) {
        let at = self.offset as usize;
        let mut word = u32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]]);
        word = (word & !self.mask()) | ((value & self.max()) << self.shift);
        record[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// A value of a light list's record the packing writes as stated: what it means to the source's programs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListConstant {
    pub bits: ListBits,
    pub value: u32,
    pub why: String,
}

/// Bits of a light list's record the source's programs read that the packing cannot fill: the source's name, and why.
/// The packing writes 0.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListUnknown {
    pub bits: ListBits,
    pub name: String,
    pub why: String,
}

/// One of the two buffers of the frame's per-tile light lists ([`super::BufferSource::LightLists`]): which lights a
/// program visits at a pixel. The screen is cut into squares of [`super::FrameParameter::LightTileSize`] pixels, tile
/// `(x, y)` (from the top left, `x = pixel x / size`) being record `x + y * LightTilesPerRow` of the lookup; the
/// lookup's record says where the tile's entries start in the index list and how many lights of each kind follow, the
/// kinds one after the other in the lookup's `counts` order; each entry names a light's record in the kind's light table
/// ([`super::BufferSource::Lights`]) and the depth slices of the tile its volume reaches.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LightListPart {
    Lookup(LightLookup),
    Indices(LightIndices),
}

/// The lookup: one record of `stride` bytes per tile, at most `capacity`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightLookup {
    pub stride: u32,
    pub capacity: u32,
    /// The tile's first entry in the index list.
    pub first: ListBits,
    /// How many lights of each kind the tile lists, in the order their entries follow each other.
    pub counts: Vec<(LightTableKind, ListBits)>,
    /// Additional fields counting a listed kind's entries whose [`LightIndices::tile_pass`] bit is set (its lights the
    /// consumer does not shadow), without adding index entries; with no such bit, every entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub count_copies: Vec<(LightTableKind, ListBits)>,
    pub constants: Vec<ListConstant>,
    pub unknown: Vec<ListUnknown>,
}

/// The index list: entries of `stride` bytes, at most `capacity`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightIndices {
    pub stride: u32,
    pub capacity: u32,
    /// The light's record in its kind's table.
    pub index: ListBits,
    /// One bit per depth slice of the tile (bit k of the value is slice k): a program skips the entry at a pixel whose
    /// slice's bit is clear.
    pub slices: ListBits,
    /// Set on the entry of a light the consumer does not shadow, clear on one it does ([`PackedLights::shadowed`]): the
    /// source's unshadowed tile pass visits only entries with it set, a shadowed light being drawn by a shadowed pass of
    /// its own. `None`: the source's entries have no such bit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_pass: Option<ListBits>,
    pub constants: Vec<ListConstant>,
    pub unknown: Vec<ListUnknown>,
}

/// The frame's light lists packed: the lookup's records and the index list's entries, little-endian.
#[derive(Clone, Debug, PartialEq)]
pub struct PackedLightLists {
    pub lookup: Vec<u8>,
    pub indices: Vec<u8>,
    /// The records' bits the packing wrote 0 into ([`ListUnknown`]), by the source's name.
    pub unknown: Vec<String>,
}

/// Fields of a record: whole words inside it, no two overlapping, constants that fit and say why, unknowns named.
fn verify_record(
    stride: u32,
    capacity: u32,
    fields: &[(ListBits, &str)],
    constants: &[ListConstant],
    unknown: &[ListUnknown],
    what: &str,
) -> Result<(), String> {
    if stride == 0 || !stride.is_multiple_of(4) {
        return Err(format!(
            "a light {what} record of {stride} bytes, not whole words"
        ));
    }
    if capacity == 0 {
        return Err(format!("a light {what} that holds no records"));
    }
    let mut all: Vec<(ListBits, String)> = fields.iter().map(|(b, n)| (*b, (*n).to_owned())).collect();
    for c in constants {
        if c.why.trim().is_empty() || c.value > c.bits.max() {
            return Err(format!(
                "light {what} constant {} does not fit its bits or says not why",
                c.value
            ));
        }
        all.push((c.bits, format!("constant {}", c.value)));
    }
    for u in unknown {
        if u.name.trim().is_empty() || u.why.trim().is_empty() {
            return Err(format!(
                "an unknown light {what} field has no name or says not why"
            ));
        }
        all.push((u.bits, u.name.clone()));
    }
    for (i, (b, name)) in all.iter().enumerate() {
        b.verify(stride, &format!("light {what} {name}"))?;
        if let Some((_, other)) = all[..i]
            .iter()
            .find(|(o, _)| o.offset == b.offset && o.mask() & b.mask() != 0)
        {
            return Err(format!("light {what} fields {other} and {name} overlap"));
        }
    }
    Ok(())
}

impl LightListPart {
    /// The part agrees with itself and its buffer (`element_stride` the structured buffer's element size, `None` for
    /// another kind of buffer): whole-word records of a capacity, every field inside one, none overlapping, the lookup
    /// counting each kind once.
    pub fn verify(&self, element_stride: Option<u32>) -> Result<(), String> {
        let stride = match self {
            LightListPart::Lookup(l) => {
                let mut fields = vec![(l.first, "first entry")];
                let mut kinds = Vec::new();
                for (kind, bits) in &l.counts {
                    if kinds.contains(kind) {
                        return Err(format!("a light lookup that counts {kind:?} lights twice"));
                    }
                    kinds.push(*kind);
                    fields.push((*bits, "count"));
                }
                if l.counts.is_empty() {
                    return Err("a light lookup that counts no lights".into());
                }
                for (kind, bits) in &l.count_copies {
                    if !kinds.contains(kind) {
                        return Err(format!("a light lookup copies an unlisted {kind:?} count"));
                    }
                    fields.push((*bits, "count copy"));
                }
                verify_record(l.stride, l.capacity, &fields, &l.constants, &l.unknown, "lookup")?;
                l.stride
            }
            LightListPart::Indices(x) => {
                let mut fields = vec![(x.index, "index"), (x.slices, "slices")];
                if let Some(bits) = x.tile_pass {
                    fields.push((bits, "tile pass"));
                }
                verify_record(
                    x.stride,
                    x.capacity,
                    &fields,
                    &x.constants,
                    &x.unknown,
                    "index list",
                )?;
                x.stride
            }
        };
        match element_stride {
            Some(element) if element != stride => Err(format!(
                "light list records of {stride} bytes in a buffer of {element}-byte elements"
            )),
            _ => Ok(()),
        }
    }
}

/// The frame's light lists listing every light in every tile and every depth slice: each tile's record starts at entry
/// 0 and counts every packed light of each kind (`counts`: the number of records the consumer packed in each kind's
/// table), and the index list holds each kind's records in order, every slice bit set. A light's own record still
/// bounds what it lights (its range), so a program shades as the source's culled lists would, visiting more entries.
/// `shadowed` are the records the consumer packed with their shadow, by kind ([`PackedLights::shadowed`]; none where
/// it shadows none): their entries' [`LightIndices::tile_pass`] bit is clear and the lookup's count copies leave them
/// out, every other entry's set. `tiles` is how many tiles the frame has (tiles per row times rows). Refused: a part
/// that does not verify, more tiles or entries than the buffers hold, a count or index its bits cannot hold, and a
/// shadowed record the counts do not hold or lists with no tile-pass bit to mark it.
pub fn pack_light_lists(
    lookup: &LightLookup,
    indices: &LightIndices,
    counts: &[(LightTableKind, u32)],
    shadowed: &[(LightTableKind, u32)],
    tiles: u32,
) -> Result<PackedLightLists, String> {
    LightListPart::Lookup(lookup.clone()).verify(None)?;
    LightListPart::Indices(indices.clone()).verify(None)?;
    if tiles == 0 || tiles > lookup.capacity {
        return Err(format!(
            "{tiles} tiles for a light lookup of {} records",
            lookup.capacity
        ));
    }
    let count_of =
        |kind: LightTableKind| -> u32 { counts.iter().filter(|(k, _)| *k == kind).map(|(_, n)| *n).sum() };
    let mut marked: Vec<(LightTableKind, u32)> = shadowed.to_vec();
    marked.sort_unstable();
    marked.dedup();
    for (kind, index) in &marked {
        if *index >= count_of(*kind) {
            return Err(format!(
                "shadowed {kind:?} record {index} is not among the {} the lists count",
                count_of(*kind)
            ));
        }
    }
    if !marked.is_empty() && indices.tile_pass.is_none() {
        return Err("the light lists have no tile-pass bit to leave a shadowed light out with".into());
    }
    let shadowed_of =
        |kind: LightTableKind| -> u32 { marked.iter().filter(|(k, _)| *k == kind).count() as u32 };
    let mut entries = Vec::new();
    for (kind, bits) in &lookup.counts {
        let n = count_of(*kind);
        if n > bits.max() || n.saturating_sub(1) > indices.index.max() {
            return Err(format!("{n} {kind:?} lights do not fit the light lists' bits"));
        }
        entries.extend((0..n).map(|i| (*kind, i)));
    }
    for (kind, bits) in &lookup.count_copies {
        let n = count_of(*kind);
        if n > bits.max() {
            return Err(format!(
                "{n} {kind:?} lights do not fit the lookup's copied count bits"
            ));
        }
    }
    if entries.len() as u64 > u64::from(indices.capacity) {
        return Err(format!(
            "{} light list entries for an index list of {}",
            entries.len(),
            indices.capacity
        ));
    }
    let mut out = PackedLightLists {
        lookup: vec![0; tiles as usize * lookup.stride as usize],
        indices: vec![0; entries.len() * indices.stride as usize],
        unknown: lookup
            .unknown
            .iter()
            .chain(&indices.unknown)
            .map(|u| u.name.clone())
            .collect(),
    };
    for record in out.lookup.chunks_exact_mut(lookup.stride as usize) {
        lookup.first.write(record, 0);
        for (kind, bits) in &lookup.counts {
            bits.write(record, count_of(*kind));
        }
        for (kind, bits) in &lookup.count_copies {
            bits.write(record, count_of(*kind) - shadowed_of(*kind));
        }
        for c in &lookup.constants {
            c.bits.write(record, c.value);
        }
    }
    for (record, (kind, index)) in out.indices.chunks_exact_mut(indices.stride as usize).zip(entries) {
        indices.index.write(record, index);
        indices.slices.write(record, indices.slices.max());
        if let Some(bits) = indices.tile_pass {
            bits.write(record, u32::from(!marked.contains(&(kind, index))));
        }
        for c in &indices.constants {
            c.bits.write(record, c.value);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use crate::components::LightSpecular;

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

    /// Independent source parameters survive packing; no component keeps a named zero stand-in, not a guessed size.
    #[test]
    fn source_falloff_and_translucency_pack_without_changing_range_or_radius() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, crate::Name("measured light".into()));
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
            },
        );
        w.insert(
            e,
            Light {
                kind: LightKind::Point,
                color: [1.; 3],
                intensity: 1.,
                range: Some(20.),
                radius: Some(3.),
                cast_shadows: None,
                specular: None,
            },
        );
        w.insert(
            e,
            crate::LightTranslucency {
                strength: 0.25,
                exponent_scale: 0.01,
            },
        );
        let t = table(
            LightTableKind::Point,
            16,
            vec![
                (0, LightValue::InverseFalloffDistance),
                (4, LightValue::TranslucencyStrength),
                (8, LightValue::TranslucencyExponentScale),
                (12, LightValue::Range),
            ],
        );
        let missing = t.pack(&w, &VIEW, &LocalShadows::none()).unwrap();
        assert_eq!(floats(&missing.bytes), vec![0., 0.25, 0.01, 20.]);
        assert_eq!(missing.unknown.len(), 1);
        assert!(missing.unknown[0].contains("measured light"));
        for distance in [8., 1.5, 0.5] {
            w.insert(e, crate::LightFalloff { distance });
            let packed = t.pack(&w, &VIEW, &LocalShadows::none()).unwrap();
            assert!(packed.unknown.is_empty());
            assert_eq!(floats(&packed.bytes), vec![distance.recip(), 0.25, 0.01, 20.]);
            assert_eq!(w.get::<Light>(e).unwrap().radius, Some(3.));
        }
        for distance in [0., -1., f32::NAN, f32::INFINITY, f32::from_bits(1)] {
            w.insert(e, crate::LightFalloff { distance });
            assert!(
                t.pack(&w, &VIEW, &LocalShadows::none())
                    .unwrap_err()
                    .contains("falloff distance")
            );
        }
    }

    /// A table that says which lights are shadowed, with a shadow's place: three point lights at view depth 7, 8 and 9,
    /// casting shadows (the first two) or not saying (the third). Shadowing none, every record holds 0 and zeros and both
    /// casters are reported unshadowed; shadowing the second, its record holds 1 and its shadow's stated terms, centre and
    /// faces, and only the first is reported. Refused: a shadow for a light that casts none, a shadow with fewer faces than
    /// the record holds, a shadow's place in a table that does not say which lights are shadowed.
    #[test]
    fn a_shadowed_light_packs_its_flag_and_its_place() {
        let mut w = World::new();
        w.insert_resource(LightUnit {
            candela_per_unit: 1.0,
        });
        let place = |w: &mut World, z: f32, cast_shadows: Option<bool>| {
            let e = w.spawn();
            w.insert(
                e,
                Transform {
                    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., z]],
                },
            );
            w.insert(
                e,
                Light {
                    kind: LightKind::Point,
                    color: [1.0; 3],
                    intensity: 1.0,
                    range: Some(4.0),
                    cast_shadows,
                    radius: None,
                    specular: None,
                },
            );
            e
        };
        let first = place(&mut w, 3., Some(true));
        let second = place(&mut w, 2., Some(true));
        let quiet = place(&mut w, 1., None);
        let t = table(
            LightTableKind::Point,
            72,
            vec![
                (0, LightValue::ViewPosition),
                (12, LightValue::Shadowed),
                (16, LightValue::ShadowProjection { floats: 2 }),
                (24, LightValue::ShadowCentre),
                (40, LightValue::ShadowFaces { faces: 2 }),
            ],
        );
        assert_eq!(t.verify(None), Ok(()));
        let record = |packed: &PackedLights, e: Entity| -> Vec<f32> {
            let i = packed.lights.iter().position(|l| *l == e).unwrap();
            floats(&packed.bytes[72 * i..72 * (i + 1)])
        };
        let none = t.pack(&w, &VIEW, &LocalShadows::none()).unwrap();
        assert_eq!(none.lights, vec![first, second, quiet]);
        assert!(none.shadowed.is_empty());
        assert_eq!(none.unshadowed_casters, vec![first, second]);
        for e in [first, second, quiet] {
            assert!(record(&none, e)[3..].iter().all(|x| *x == 0.0));
        }
        let shadow = LocalShadow {
            projection: vec![1.001, -0.008, 7.0],
            centre: [-34.25, 8.0, -0.5, 1.0],
            faces: vec![[0.25, -0.25, 0.5, 0.5], [0.25, -0.25, 1.5, 0.5]],
        };
        let shadows = LocalShadows([(second, shadow.clone())].into());
        let one = t.pack(&w, &VIEW, &shadows).unwrap();
        assert_eq!(one.shadowed, vec![1]);
        assert_eq!(one.unshadowed_casters, vec![first]);
        assert_eq!(
            record(&one, second)[3..],
            [
                1.0, 1.001, -0.008, -34.25, 8.0, -0.5, 1.0, 0.25, -0.25, 0.5, 0.5, 0.25, -0.25, 1.5, 0.5
            ]
        );
        assert!(record(&one, first)[3..].iter().all(|x| *x == 0.0));

        w.get_mut::<Light>(second).unwrap().cast_shadows = Some(false);
        assert!(t.pack(&w, &VIEW, &shadows).unwrap_err().contains("casts none"));
        w.get_mut::<Light>(second).unwrap().cast_shadows = Some(true);
        let mut short = shadow;
        short.faces.pop();
        let refused = t.pack(&w, &VIEW, &LocalShadows([(second, short)].into()));
        assert!(refused.unwrap_err().contains("fewer than 2 faces"));
        let unsaid = table(LightTableKind::Point, 16, vec![(0, LightValue::ShadowCentre)]);
        assert!(unsaid.verify(None).unwrap_err().contains("does not say"));
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
                    specular: None,
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
        assert!(t.pack(&w, &VIEW, &LocalShadows::none()).is_err());
        w.insert_resource(LightUnit {
            candela_per_unit: 2.0,
        });
        t.capacity = 1;
        let packed = t.pack(&w, &VIEW, &LocalShadows::none()).unwrap();
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

    /// Two point lights of colour (0.5, 0.25, 1) and intensity 8 in a world whose unit is 2 candela: one with its own
    /// specular, colour (1, 0.5, 0) at intensity 4, packs specular (2, 1, 0); the other, which states none, packs its
    /// colour (2, 1, 4) there, and is the one counted. Both pack colour (2, 1, 4).
    #[test]
    fn a_light_packs_its_own_specular_and_one_without_packs_its_colour() {
        let mut w = World::new();
        w.insert_resource(LightUnit {
            candela_per_unit: 2.0,
        });
        let place = |w: &mut World, z: f32, specular: Option<LightSpecular>| {
            let e = w.spawn();
            w.insert(
                e,
                Transform {
                    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., z]],
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
                    specular,
                },
            );
            e
        };
        let own = place(
            &mut w,
            0.0,
            Some(LightSpecular {
                color: [1.0, 0.5, 0.0],
                intensity: 4.0,
            }),
        );
        let plain = place(&mut w, -5.0, None);
        let t = table(
            LightTableKind::Point,
            24,
            vec![(0, LightValue::Colour), (12, LightValue::SpecularColour)],
        );
        let packed = t.pack(&w, &VIEW, &LocalShadows::none()).unwrap();
        assert_eq!(packed.lights, vec![own, plain]);
        assert_eq!(packed.specular_from_colour, 1);
        let f = floats(&packed.bytes);
        let want = [2.0, 1.0, 4.0, 2.0, 1.0, 0.0, 2.0, 1.0, 4.0, 2.0, 1.0, 4.0];
        for (i, v) in want.into_iter().enumerate() {
            close(f[i], v, &format!("float {i}"));
        }
        // a table with no specular field counts none
        let colour_only = table(LightTableKind::Point, 12, vec![(0, LightValue::Colour)]);
        assert_eq!(
            colour_only
                .pack(&w, &VIEW, &LocalShadows::none())
                .unwrap()
                .specular_from_colour,
            0
        );
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
                specular: None,
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
        let f = floats(&t.pack(&w, &VIEW, &LocalShadows::none()).unwrap().bytes);
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

    /// The same spot shaped by a `core.spot_frustum`: half-angles 45 degrees across (tan 1) and 30 up (tan 1 / sqrt 3),
    /// near plane 2, range 6. Each axis's row is over its own tangent; the near plane's row is
    /// `(d - 2) * 6 / (6 - 2)`; the bounding sphere is the base's own (the corners' angle, `atan(sqrt(1 + 1 / 3))`, is
    /// above 45 degrees: radius 6 sqrt(4 / 3), centred at the base); the vertical cone's sphere is the 30-degree cone's,
    /// radius 6 / (2 cos^2 30) = 4 and the margin, centred 4 along the beam; the depths are the apex's and the base's, as
    /// both axes lie across the view.
    #[test]
    fn a_spot_frustum_shapes_its_cone_near_plane_and_spheres() {
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
        let spot = Light {
            kind: LightKind::Spot {
                inner_cone: 0.3,
                outer_cone: 0.6,
            },
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            range: Some(6.0),
            cast_shadows: None,
            radius: None,
            specular: None,
        };
        w.insert(e, spot.clone());
        let frustum = crate::SpotFrustum {
            horizontal_half_angle: std::f32::consts::FRAC_PI_4,
            vertical_half_angle: std::f32::consts::FRAC_PI_6,
            near: 2.0,
        };
        w.insert(e, frustum);
        let t = table(
            LightTableKind::Spot,
            128,
            vec![
                (0, LightValue::ViewToCone),
                (64, LightValue::ViewBoundingSphere),
                (80, LightValue::VerticalConeSphere { margin: 0.001 }),
                (96, LightValue::ViewDepthMin),
                (100, LightValue::ViewDepthMax),
            ],
        );
        let f = floats(&t.pack(&w, &VIEW, &LocalShadows::none()).unwrap().bytes);
        let corners = (4.0f64 / 3.0).sqrt();
        let want = [
            1.0,
            0.0,
            0.0,
            0.0, // right / tan 45
            0.0,
            3f64.sqrt(),
            0.0,
            0.0, // up / tan 30
            0.0,
            0.0,
            1.5,
            -18.0, // the near plane: (beam . p - beam . apex - 2) * 6 / 4
            0.0,
            0.0,
            1.0,
            -10.0, // beam, less beam . apex
            0.0,
            0.0,
            16.0,
            6.0 * corners, // the base's sphere
            0.0,
            0.0,
            14.0,
            4.001, // the vertical cone's sphere and its margin
            10.0,
            16.0, // depths
        ];
        for (i, v) in want.into_iter().enumerate() {
            close(f[i], v, &format!("float {i}"));
        }
        // the near plane: a position 1 along the beam is before it, 3 along is past it
        let row = |r: usize, d: f64| {
            (0..4)
                .map(|k| f64::from(f[4 * r + k]) * [0.0, 0.0, 10.0 + d, 1.0][k])
                .sum::<f64>()
        };
        assert!(row(2, 1.0) < 0.0 && row(2, 3.0) > 0.0);

        // refused: a frustum whose near plane is not before the range, a half-angle of 90 degrees, a frustum on a light
        // that is no spot, and a cone sphere in a table of point lights
        for bad in [
            crate::SpotFrustum { near: 6.0, ..frustum },
            crate::SpotFrustum {
                vertical_half_angle: std::f32::consts::FRAC_PI_2,
                ..frustum
            },
        ] {
            w.insert(e, bad);
            assert!(t.pack(&w, &VIEW, &LocalShadows::none()).is_err(), "{bad:?}");
        }
        w.insert(e, frustum);
        w.insert(
            e,
            Light {
                kind: LightKind::Point,
                ..spot
            },
        );
        assert!(<crate::SpotFrustum as crate::Component>::verify(&frustum, e, &w).is_err());
        let points = table(
            LightTableKind::Point,
            16,
            vec![(0, LightValue::VerticalConeSphere { margin: 0.0 })],
        );
        assert!(points.verify(None).is_err());
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

    fn bits(offset: u32, shift: u8, bits: u8) -> ListBits {
        ListBits { offset, shift, bits }
    }

    /// A lookup of 12-byte records (the first entry in word 0, the point and spot counts in the low and high halves of
    /// word 1, word 2 unknown) and an index list of 4-byte entries (the index in the low 16 bits, 15 slice bits from bit
    /// 16, bit 31 a flag the source's programs require set).
    fn lists() -> (LightLookup, LightIndices) {
        (
            LightLookup {
                stride: 12,
                capacity: 4,
                first: bits(0, 0, 32),
                counts: vec![
                    (LightTableKind::Point, bits(4, 0, 16)),
                    (LightTableKind::Spot, bits(4, 16, 16)),
                ],
                count_copies: Vec::new(),
                constants: Vec::new(),
                unknown: vec![ListUnknown {
                    bits: bits(8, 0, 32),
                    name: "word 2".into(),
                    why: "read only by a debug view".into(),
                }],
            },
            LightIndices {
                stride: 4,
                capacity: 8,
                index: bits(0, 0, 16),
                slices: bits(0, 16, 15),
                tile_pass: Some(bits(0, 31, 1)),
                constants: Vec::new(),
                unknown: Vec::new(),
            },
        )
    }

    fn words(bytes: &[u8]) -> Vec<u32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect()
    }

    /// Two point lights and a spot in three tiles: every tile starts at entry 0 and counts 2 points and 1 spot; the
    /// entries are the points' records 0 and 1, then the spot's record 0, each with every slice bit and the flag set.
    #[test]
    fn every_light_is_listed_in_every_tile_and_slice() {
        let (lookup, indices) = lists();
        let packed = pack_light_lists(
            &lookup,
            &indices,
            &[(LightTableKind::Spot, 1), (LightTableKind::Point, 2)],
            &[],
            3,
        )
        .unwrap();
        assert_eq!(
            words(&packed.lookup),
            [0, 0x0001_0002, 0, 0, 0x0001_0002, 0, 0, 0x0001_0002, 0]
        );
        assert_eq!(words(&packed.indices), [0xffff_0000, 0xffff_0001, 0xffff_0000]);
        assert_eq!(packed.unknown, ["word 2"]);
    }

    /// A second eligible-light count lets a shader reject an empty tile before walking the main counts. Listing all
    /// lights makes both counts equal; copying them must not duplicate the three entries.
    #[test]
    fn copied_counts_keep_nonempty_tiles_and_do_not_duplicate_entries() {
        let (mut lookup, indices) = lists();
        lookup.unknown.clear();
        lookup.count_copies = vec![
            (LightTableKind::Point, bits(8, 0, 16)),
            (LightTableKind::Spot, bits(8, 16, 16)),
        ];
        let packed = pack_light_lists(
            &lookup,
            &indices,
            &[(LightTableKind::Point, 2), (LightTableKind::Spot, 1)],
            &[],
            2,
        )
        .unwrap();
        assert_eq!(
            words(&packed.lookup),
            [0, 0x0001_0002, 0x0001_0002, 0, 0x0001_0002, 0x0001_0002]
        );
        assert_eq!(words(&packed.indices), [0xffff_0000, 0xffff_0001, 0xffff_0000]);
        assert!(packed.unknown.is_empty());
        let empty = pack_light_lists(&lookup, &indices, &[], &[], 1).unwrap();
        assert_eq!(words(&empty.lookup), [0, 0, 0]);
        assert!(empty.indices.is_empty());

        let mut invalid = lookup.clone();
        invalid.counts.pop();
        assert!(
            LightListPart::Lookup(invalid)
                .verify(None)
                .unwrap_err()
                .contains("unlisted")
        );
        let mut narrow = lookup.clone();
        narrow.count_copies[0].1 = bits(8, 0, 1);
        assert!(pack_light_lists(&narrow, &indices, &[(LightTableKind::Point, 2)], &[], 1).is_err());
        lookup.count_copies[0].1 = bits(4, 0, 16);
        assert!(
            LightListPart::Lookup(lookup)
                .verify(None)
                .unwrap_err()
                .contains("overlap")
        );
    }

    /// The consumer shadows point record 1 of 2: its entry's tile-pass bit is clear, the other entries' set, and the
    /// lookup's copied counts leave it out while its counts still list it. Refused: a shadowed record the counts do not
    /// hold, and a shadowed record in lists with no tile-pass bit.
    #[test]
    fn a_shadowed_light_leaves_the_tile_pass_but_stays_listed() {
        let (mut lookup, mut indices) = lists();
        lookup.unknown.clear();
        lookup.count_copies = vec![
            (LightTableKind::Point, bits(8, 0, 16)),
            (LightTableKind::Spot, bits(8, 16, 16)),
        ];
        let counts = [(LightTableKind::Point, 2), (LightTableKind::Spot, 1)];
        let packed = pack_light_lists(&lookup, &indices, &counts, &[(LightTableKind::Point, 1)], 1).unwrap();
        assert_eq!(words(&packed.lookup), [0, 0x0001_0002, 0x0001_0001]);
        assert_eq!(words(&packed.indices), [0xffff_0000, 0x7fff_0001, 0xffff_0000]);
        assert!(
            pack_light_lists(&lookup, &indices, &counts, &[(LightTableKind::Spot, 1)], 1)
                .unwrap_err()
                .contains("not among")
        );
        indices.tile_pass = None;
        assert!(
            pack_light_lists(&lookup, &indices, &counts, &[(LightTableKind::Point, 0)], 1)
                .unwrap_err()
                .contains("no tile-pass bit")
        );
    }

    /// Refused: more tiles or entries than the buffers hold, a count its bits cannot hold, overlapping fields, a part
    /// whose records are not its buffer's elements.
    #[test]
    fn light_lists_refuse_what_they_cannot_hold() {
        let (lookup, indices) = lists();
        assert!(pack_light_lists(&lookup, &indices, &[(LightTableKind::Point, 1)], &[], 5).is_err());
        assert!(pack_light_lists(&lookup, &indices, &[(LightTableKind::Point, 9)], &[], 1).is_err());
        let mut narrow = lookup.clone();
        narrow.counts[0].1 = bits(4, 0, 2);
        assert!(pack_light_lists(&narrow, &indices, &[(LightTableKind::Point, 4)], &[], 1).is_err());
        let mut overlap = indices.clone();
        overlap.slices = bits(0, 15, 15);
        assert!(LightListPart::Indices(overlap).verify(Some(4)).is_err());
        assert!(LightListPart::Lookup(lookup).verify(Some(16)).is_err());
        assert_eq!(LightListPart::Indices(indices).verify(Some(4)), Ok(()));
    }
}
