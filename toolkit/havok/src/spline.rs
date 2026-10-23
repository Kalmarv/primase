//! `hkaSplineCompressedAnimation` decoded: the blocks of compressed tracks its `data` holds as typed values
//! ([`SplineAnimation::parse`]) and each track sampled at a time ([`SplineAnimation::pose_at`]).
//!
//! The format facts (the block, mask and track layouts, the quantizations, the knot vectors, the spans of time a
//! block covers) were taken from reading Smithbox's `SplineCompressedAnimation.cs` and
//! `NewHavokAnimation_SplineCompressed.cs` (an adaptation of HavokLib's `hkaSplineDecompressor`, GPL-3.0) and from
//! the files themselves; no code is taken from either. The spline arithmetic is the standard B-spline basis
//! (Piegl and Tiller, *The NURBS Book*, algorithms A2.1 and A2.2), written here in f64 and rounded once to f32.
//!
//! # The data
//!
//! `data` is `numBlocks` blocks at `blockOffsets`, each 16-byte aligned, a block covering `maxFramesPerBlock - 1`
//! frames (adjacent blocks share their boundary frame). A block is, for each transform track in order:
//!
//! - a 4-byte mask: the quantizations (bits 0-1 the position scalars', 2-5 the rotation's, 6-7 the scale scalars'),
//!   then the position, rotation and scale flags, each byte holding in its low nibble the components that are
//!   static (x y z w) and in its high nibble those that are splines;
//! - after all the masks, padding to 4; then for each track a position, a rotation and a scale, each followed by
//!   padding to 4: a vector with a spline component is a spline track (a count, a degree, the knot bytes, padding to 4,
//!   each spline component's bounds and each static component's value as f32, then the control points a point at a
//!   time with the spline components in x y z order, each a quantized scalar between its bounds), a vector with only
//!   static components is those f32s; a rotation with a spline flag is a spline track (a count, a degree, the knots,
//!   padding to the quantization's alignment, the quantized quaternions), one with a static flag is one quantized
//!   quaternion at that alignment;
//! - padding to 16.
//!
//! A component with neither flag is absent, and the caller states what stands in for it ([`Absent`]). In Sekiro's
//! character animations (1,122,267 tracks of 8,888 animations) the neutral value is what the data says: an absent
//! rotation is on a bone whose reference rotation is not the identity in 34% of the cases against 77% of all tracks,
//! and an absent position component on a bone whose reference component is not zero in 41%, 4% and 9% of the cases
//! (x, y, z) against 93%, 36% and 44% of all, so an encoder that left them out was leaving out zero and the identity,
//! not the reference. The game agrees: its live pose of the idle animation (read from the running game's memory) equals
//! the neutral decoding on the bones where the two fallbacks differ (12 of 12 closer to neutral, 0 to the reference,
//! 1e-6 to 1e-5 against 5e-5 to 3e-4; knowledge base note 447729a8). Elden Ring's files (611,903 tracks of 5,843 animations) show
//! the same direction, more weakly, and cannot be compared in the game (it is not launched here): of its absent components, the share
//! on a bone whose reference is not neutral is 51.6% (rotation), 59.9%, 6.5% and 19.7% (position x, y, z) against 85.6%, 93.1%, 33.6% and
//! 47.2% of all tracks, so the encoder drops what is neutral, not what equals the reference (which would leave the two shares alike).
//! Float tracks are not read: an animation with some is refused. A track bound to a negative bone
//! index drives no bone.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Animation, Tracks};
use crate::{Error, Result, invalid};

/// The knots and degree a spline track's control points are evaluated with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Knots {
    pub degree: u8,
    /// Frames within the block, `points + degree + 1` of them, not decreasing.
    pub values: Vec<u8>,
}

/// One component of a vector: stated by the mask or not.
#[derive(Clone, Debug, PartialEq)]
pub enum Component {
    /// Neither static nor spline: the caller's default stands.
    Absent,
    Static(f32),
    /// The control points (decoded to f32), evaluated with the track's knots.
    Spline(Vec<f32>),
}

/// A position or a scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Vector {
    /// Present when any component is a spline.
    pub knots: Option<Knots>,
    pub x: Component,
    pub y: Component,
    pub z: Component,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rotation {
    /// The identity.
    Absent,
    /// A quaternion `x y z w`, as decoded from its quantization.
    Static([f32; 4]),
    Spline {
        knots: Knots,
        points: Vec<[f32; 4]>,
    },
}

/// What stands in for a component a track does not state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Zero for a position, the identity for a rotation, one for a scale.
    Neutral,
    /// The bone's reference transform.
    Reference,
}

/// The fallback for each kind of component; the caller states it (see the module documentation for what Sekiro's
/// files say).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Absent {
    pub position: Fallback,
    pub rotation: Fallback,
    pub scale: Fallback,
}

impl Absent {
    /// Zero, identity and one.
    pub const NEUTRAL: Absent = Absent {
        position: Fallback::Neutral,
        rotation: Fallback::Neutral,
        scale: Fallback::Neutral,
    };
    /// The reference transform for all three.
    pub const REFERENCE: Absent = Absent {
        position: Fallback::Reference,
        rotation: Fallback::Reference,
        scale: Fallback::Reference,
    };
}

/// One transform track of one block.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub position: Vector,
    pub rotation: Rotation,
    pub scale: Vector,
}

/// How a rotation is stored (`RotationQuantization`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationQuantization {
    Polar32,
    ThreeComp40,
    ThreeComp48,
    Uncompressed,
}

impl RotationQuantization {
    fn of(code: u8, at: usize) -> Result<RotationQuantization> {
        Ok(match code {
            0 => RotationQuantization::Polar32,
            1 => RotationQuantization::ThreeComp40,
            2 => RotationQuantization::ThreeComp48,
            5 => RotationQuantization::Uncompressed,
            3 => return Err(invalid(at, "rotation quantization THREECOMP24 is not read")),
            4 => return Err(invalid(at, "rotation quantization STRAIGHT16 is not read")),
            other => return Err(invalid(at, format!("rotation quantization {other} is unknown"))),
        })
    }

    /// The alignment of a quantized quaternion within the data.
    fn align(self) -> usize {
        match self {
            RotationQuantization::Polar32 | RotationQuantization::Uncompressed => 4,
            RotationQuantization::ThreeComp40 => 1,
            RotationQuantization::ThreeComp48 => 2,
        }
    }
}

/// The quantized scalar a vector's spline control points are stored as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scalar {
    Bits8,
    Bits16,
}

impl Scalar {
    fn of(code: u8, at: usize) -> Result<Scalar> {
        match code {
            0 => Ok(Scalar::Bits8),
            1 => Ok(Scalar::Bits16),
            other => Err(invalid(at, format!("scalar quantization {other} is unknown"))),
        }
    }
}

/// A track's mask.
#[derive(Clone, Copy, Debug)]
struct Mask {
    position: Scalar,
    rotation: u8,
    scale: Scalar,
    position_flags: u8,
    rotation_flags: u8,
    scale_flags: u8,
}

const STATIC: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
const SPLINE: [u8; 4] = [0x10, 0x20, 0x40, 0x80];

/// A bounds-checked reader of the block data.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| invalid(self.at, "the block runs past the animation data"))?;
        let out = self.bytes.get(self.at..end).unwrap_or_default();
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?.first().copied().unwrap_or_default())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let at = self.at;
        self.take(N)?.try_into().map_err(|_| invalid(at, "short read"))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32> {
        let at = self.at;
        let v = self.raw_f32()?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(invalid(at, "a stored float is not finite"))
        }
    }

    /// A float as stored, finite or not (a vector component's value or range, where a non-finite one is that
    /// component's own: see [`Unreadable`]).
    fn raw_f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    /// Skip to the next multiple of `align` (a position in the whole data).
    fn pad(&mut self, align: usize) -> Result<()> {
        let to = self.at.next_multiple_of(align);
        if to > self.bytes.len() {
            return Err(invalid(self.at, "padding runs past the animation data"));
        }
        self.at = to;
        Ok(())
    }
}

fn mask(r: &mut Reader) -> Result<Mask> {
    let at = r.at;
    let [quantization, position, rotation, scale] = r.array::<4>()?;
    Ok(Mask {
        position: Scalar::of(quantization & 3, at)?,
        rotation: (quantization >> 2) & 0xF,
        scale: Scalar::of(quantization >> 6, at)?,
        position_flags: position,
        rotation_flags: rotation,
        scale_flags: scale,
    })
}

/// A spline track's head: the point count, the degree and the knots.
fn knots(r: &mut Reader) -> Result<(usize, Knots)> {
    let at = r.at;
    let items = r.i16()?;
    let degree = r.u8()?;
    let points = usize::try_from(items)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| invalid(at, "a spline track with a negative count"))?;
    if !(1..=4).contains(&degree) || points <= usize::from(degree) {
        return Err(invalid(
            at,
            format!("a spline track of degree {degree} with {points} control points"),
        ));
    }
    // points + degree + 1 knots
    let count = points
        .checked_add(usize::from(degree))
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| invalid(at, "the knot count overflows"))?;
    let values = r.take(count)?.to_vec();
    if values.windows(2).any(|w| matches!(*w, [a, b] if a > b)) {
        return Err(invalid(at, "spline knots decrease"));
    }
    Ok((points, Knots { degree, values }))
}

fn quantized(r: &mut Reader, scalar: Scalar, min: f32, max: f32) -> Result<f32> {
    let ratio = match scalar {
        Scalar::Bits8 => f64::from(r.u8()?) / 255.0,
        Scalar::Bits16 => f64::from(r.u16()?) / 65535.0,
    };
    Ok(round_f32(
        f64::from(min) + (f64::from(max) - f64::from(min)) * ratio,
    ))
}

/// `v` rounded to the nearest f32: a value computed in f64 rounded once.
#[allow(
    clippy::cast_possible_truncation,
    reason = "rounding to f32 is the stated conversion"
)]
fn round_f32(v: f64) -> f32 {
    v as f32
}

fn vector(r: &mut Reader, flags: u8, scalar: Scalar, unreadable: &mut Vec<usize>) -> Result<Vector> {
    // x, y and z: the first three of the flags (w is a rotation's)
    let spline = SPLINE.iter().take(3).any(|f| flags & f != 0);
    let state = |i: usize| {
        if SPLINE.get(i).is_some_and(|f| flags & f != 0) {
            1
        } else if STATIC.get(i).is_some_and(|f| flags & f != 0) {
            2
        } else {
            0
        }
    };
    if !spline {
        let mut read = |i: usize| -> Result<Component> {
            if state(i) == 2 {
                let v = r.raw_f32()?;
                if v.is_finite() {
                    Ok(Component::Static(v))
                } else {
                    unreadable.push(i);
                    Ok(Component::Absent)
                }
            } else {
                Ok(Component::Absent)
            }
        };
        return Ok(Vector {
            knots: None,
            x: read(0)?,
            y: read(1)?,
            z: read(2)?,
        });
    }
    let (points, knots) = knots(r)?;
    r.pad(4)?;
    let mut bounds = [(0.0, 0.0); 3];
    let mut component = [Component::Absent, Component::Absent, Component::Absent];
    for ((bound, comp), i) in bounds.iter_mut().zip(component.iter_mut()).zip(0..) {
        match state(i) {
            1 => {
                *bound = (r.raw_f32()?, r.raw_f32()?);
                if !(bound.0.is_finite() && bound.1.is_finite()) {
                    unreadable.push(i);
                }
            }
            2 => {
                let v = r.raw_f32()?;
                if v.is_finite() {
                    *comp = Component::Static(v);
                } else {
                    unreadable.push(i);
                }
            }
            _ => {}
        }
    }
    let mut curves: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for _ in 0..points {
        for ((curve, bound), i) in curves.iter_mut().zip(&bounds).zip(0..) {
            if state(i) == 1 {
                curve.push(quantized(r, scalar, bound.0, bound.1)?);
            }
        }
    }
    for ((comp, curve), i) in component.iter_mut().zip(curves).zip(0..) {
        if state(i) == 1 && !unreadable.contains(&i) {
            *comp = Component::Spline(curve);
        }
    }
    let [x, y, z] = component;
    Ok(Vector {
        knots: Some(knots),
        x,
        y,
        z,
    })
}

/// A quaternion stored as three components and the index and sign of the fourth, which is what makes it a unit.
fn three_components(c: [f32; 3], shift: usize, negative: bool) -> [f32; 4] {
    let rest = 1.0 - (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]);
    let mut largest = if rest <= 0.0 { 0.0 } else { rest.sqrt() };
    if negative {
        largest = -largest;
    }
    let mut out = [0.0; 4];
    let mut next = c.into_iter();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = if i == shift {
            largest
        } else {
            next.next().unwrap_or_default()
        };
    }
    out
}

fn quaternion(r: &mut Reader, how: RotationQuantization) -> Result<[f32; 4]> {
    Ok(match how {
        RotationQuantization::Uncompressed => [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        RotationQuantization::ThreeComp48 => {
            // 15 bits each, the top bit of each a part of the index and the sign
            let (x, y, z) = (r.i16()?, r.i16()?, r.i16()?);
            // the index: y's top bit, then x's
            let shift = match (y < 0, x < 0) {
                (false, false) => 0,
                (false, true) => 1,
                (true, false) => 2,
                (true, true) => 3,
            };
            let negative = (z as u16) >> 15 != 0;
            // a 15-bit value less 16383 is exact in f32
            let value = |v: i16| (f32::from(v & 0x7FFF) - 16383.0) * 0.000_043_161;
            three_components([value(x), value(y), value(z)], shift, negative)
        }
        RotationQuantization::ThreeComp40 => {
            let [b0, b1, b2, b3, b4] = r.array::<5>()?;
            let c = u64::from_le_bytes([b0, b1, b2, b3, b4, 0, 0, 0]);
            // 12 bits each, then the index in bits 36-37 and the sign in bit 38; a 12-bit value less 2047 is exact in f32
            let v = |shift: u32| (((c >> shift) & 0xFFF) as f32 - 2047.0) * 0.000_345_436;
            three_components([v(0), v(12), v(24)], ((c >> 36) & 3) as usize, (c >> 38) & 1 != 0)
        }
        RotationQuantization::Polar32 => {
            let c = r.u32()?;
            let radius = f64::from((c >> 18) & 0x3FF) / 1023.0;
            let w = 1.0 - radius * radius;
            let phi_theta = f64::from(c & 0x3FFFF);
            let ring = phi_theta.sqrt().floor();
            let (mut phi, mut theta) = (0.0, 0.0);
            if ring > 0.0 {
                theta = std::f64::consts::FRAC_PI_4 * (phi_theta - ring * ring) / ring;
                phi = ring * (std::f64::consts::FRAC_PI_2 / 511.0);
            }
            let magnitude = (1.0 - w * w).sqrt();
            let mut q = [
                phi.sin() * theta.cos() * magnitude,
                phi.sin() * theta.sin() * magnitude,
                phi.cos() * magnitude,
                w,
            ];
            for (v, bit) in q.iter_mut().zip([28, 29, 30, 31]) {
                if (c >> bit) & 1 != 0 {
                    *v = -*v;
                }
            }
            q.map(round_f32)
        }
    })
}

fn rotation(r: &mut Reader, flags: u8, code: u8) -> Result<Rotation> {
    let at = r.at;
    let how = RotationQuantization::of(code, at)?;
    if SPLINE.iter().any(|f| flags & f != 0) {
        let (points, knots) = knots(r)?;
        r.pad(how.align())?;
        let points = (0..points)
            .map(|_| quaternion(r, how))
            .collect::<Result<Vec<_>>>()?;
        Ok(Rotation::Spline { knots, points })
    } else if STATIC.iter().any(|f| flags & f != 0) {
        r.pad(how.align())?;
        Ok(Rotation::Static(quaternion(r, how)?))
    } else {
        Ok(Rotation::Absent)
    }
}

/// A vector component (a position or scale axis) of a track whose stored value, or the range its quantised control
/// points are scaled between, is NaN or infinite: no value can be decoded from it (Elden Ring's `c2030/a001_009000` has
/// one, the scale of the leaf bone `R_Egg`, all six stored bounds NaN). Its control points are still skipped, and the
/// component is [`Component::Absent`], so the caller's `absent` stands for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    pub block: usize,
    pub track: usize,
    /// `position x`, `scale z`, ...
    pub part: &'static str,
}

/// A decoded `hkaSplineCompressedAnimation`.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineAnimation {
    /// One list of transform tracks for each block.
    pub blocks: Vec<Vec<Track>>,
    /// The vector components whose stored value or quantisation range is not finite, which [`Self::sample`] takes as
    /// absent.
    pub unreadable: Vec<Unreadable>,
    pub duration: f32,
    pub num_frames: u32,
    pub max_frames_per_block: u32,
    pub frame_duration: f32,
}

impl SplineAnimation {
    /// Decode the blocks of `animation` (which must be a spline compressed one without float tracks, little endian).
    /// Each block must end where the data says the next thing begins: the float data at `floatBlockOffsets`, then, after
    /// padding to 16, the next block or the end of the data.
    pub fn parse(animation: &Animation) -> Result<SplineAnimation> {
        let Tracks::Spline(s) = &animation.tracks else {
            return Err(invalid(
                0,
                format!("{} is not a spline compressed animation", animation.class),
            ));
        };
        if animation.float_tracks != 0 {
            return Err(invalid(0, "an animation with float tracks is not read"));
        }
        if s.endian != 0 {
            return Err(invalid(0, "a big endian animation is not read"));
        }
        if s.max_frames_per_block < 2
            || s.num_frames == 0
            || s.frame_duration.is_nan()
            || s.frame_duration <= 0.0
        {
            return Err(invalid(
                0,
                "the animation's frame counts or duration are not usable",
            ));
        }
        let blocks = s.num_blocks as usize;
        if s.block_offsets.len() != blocks || s.float_block_offsets.len() != blocks || blocks == 0 {
            return Err(invalid(0, "the block offsets do not match the block count"));
        }
        let mut out = Vec::with_capacity(blocks);
        let mut unreadable = Vec::new();
        // each block's start and float data offset, and where the next begins (the end of the data after the last)
        let nexts = s
            .block_offsets
            .iter()
            .skip(1)
            .map(|&o| o as usize)
            .chain([s.data.len()]);
        let blocks = s.block_offsets.iter().zip(&s.float_block_offsets).zip(nexts);
        for (b, ((&start, &float), next)) in blocks.enumerate() {
            let start = start as usize;
            if !start.is_multiple_of(16) {
                return Err(invalid(start, "a block is not 16-byte aligned"));
            }
            // `floatBlockOffsets` count from the block's own start
            let float_start = start
                .checked_add(float as usize)
                .ok_or_else(|| invalid(start, format!("block {b}'s float data offset overflows")))?;
            let mut r = Reader {
                bytes: &s.data,
                at: start,
            };
            let ends = Ends { float_start, next };
            out.push(block(
                &mut r,
                animation.transform_tracks,
                b,
                ends,
                &mut unreadable,
            )?);
        }
        Ok(SplineAnimation {
            blocks: out,
            unreadable,
            duration: animation.duration,
            num_frames: s.num_frames,
            max_frames_per_block: s.max_frames_per_block,
            frame_duration: s.frame_duration,
        })
    }
}

/// Where a block's tracks end: where its float data begins; and where, after padding to 16, the next block begins
/// (the end of the data after the last).
#[derive(Clone, Copy)]
struct Ends {
    float_start: usize,
    next: usize,
}

/// Part `axis` (0, 1 or 2: x, y, z) of a vector, from its three `names`.
fn axis_part(names: [&'static str; 3], axis: usize) -> Result<&'static str> {
    names
        .get(axis)
        .copied()
        .ok_or_else(|| invalid(0, format!("axis {axis} of a vector")))
}

/// One block, number `index`: its tracks, ending where `ends` says.
fn block(
    r: &mut Reader,
    tracks: usize,
    index: usize,
    ends: Ends,
    unreadable: &mut Vec<Unreadable>,
) -> Result<Vec<Track>> {
    let masks = (0..tracks).map(|_| mask(r)).collect::<Result<Vec<_>>>()?;
    r.pad(4)?;
    let mut out = Vec::with_capacity(tracks);
    for (track, m) in masks.into_iter().enumerate() {
        // a failure names where it is: the block, the track and which of its three parts
        let at = |part: &'static str| {
            move |e: Error| {
                invalid(
                    e.offset,
                    format!("block {index} track {track} {part}: {}", e.detail),
                )
            }
        };
        let mut bad = Vec::new();
        let position = vector(r, m.position_flags, m.position, &mut bad).map_err(at("position"))?;
        for axis in bad.drain(..) {
            unreadable.push(Unreadable {
                block: index,
                track,
                part: axis_part(["position x", "position y", "position z"], axis)?,
            });
        }
        r.pad(4)?;
        let rotation = rotation(r, m.rotation_flags, m.rotation).map_err(at("rotation"))?;
        r.pad(4)?;
        let scale = vector(r, m.scale_flags, m.scale, &mut bad).map_err(at("scale"))?;
        for axis in bad.drain(..) {
            unreadable.push(Unreadable {
                block: index,
                track,
                part: axis_part(["scale x", "scale y", "scale z"], axis)?,
            });
        }
        r.pad(4)?;
        out.push(Track {
            position,
            rotation,
            scale,
        });
    }
    let Ends { float_start, next } = ends;
    if r.at != float_start {
        return Err(invalid(
            r.at,
            format!(
                "block {index} ends at {}, the float data begins at {float_start}",
                r.at
            ),
        ));
    }
    let on_boundary = r.at.is_multiple_of(16);
    r.pad(16)?;
    if r.at != next {
        // The 2018.1 files the install holds pad a block whose float data begins on a 16-byte boundary by 16 more
        // zero bytes in some cases (711 of 1,568 such blocks; the other 857 and every block of the 2016.2 files have
        // none, with the same trailing bytes: the encoder's rule is its own, not the data's). Those 16 zero bytes are
        // accepted, nothing else.
        let extra = on_boundary
            && r.at.checked_add(16) == Some(next)
            && r.bytes
                .get(r.at..next)
                .is_some_and(|pad| pad.iter().all(|&b| b == 0));
        if !extra {
            return Err(invalid(
                r.at,
                format!("block {index} ends at {}, the next begins at {next}", r.at),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;
    use crate::animation::{AnimationType, QsTransform, SplineData};

    /// Positions and scales fall back to the reference, rotations to the identity.
    const MIXED: Absent = Absent {
        position: Fallback::Reference,
        rotation: Fallback::Neutral,
        scale: Fallback::Reference,
    };

    const IDENTITY: QsTransform = QsTransform {
        translation: [1.0, 2.0, 3.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0, 0.0],
    };

    fn animation(
        data: Vec<u8>,
        tracks: usize,
        blocks: &[(u32, u32)],
        per_block: u32,
        frames: u32,
    ) -> Animation {
        Animation {
            class: "hkaSplineCompressedAnimation".into(),
            kind: AnimationType::SplineCompressed,
            duration: (frames - 1) as f32,
            transform_tracks: tracks,
            float_tracks: 0,
            extracted_motion: None,
            annotation_tracks: vec![String::new(); tracks],
            tracks: Tracks::Spline(SplineData {
                num_frames: frames,
                num_blocks: blocks.len() as u32,
                max_frames_per_block: per_block,
                mask_and_quantization_size: 0,
                block_duration: (per_block - 1) as f32,
                block_inverse_duration: 1.0 / (per_block - 1) as f32,
                frame_duration: 1.0,
                block_offsets: blocks.iter().map(|b| b.0).collect(),
                float_block_offsets: blocks.iter().map(|b| b.1).collect(),
                transform_offsets: vec![],
                float_offsets: vec![],
                data,
                endian: 0,
            }),
        }
    }

    fn f(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    fn pad(data: &mut Vec<u8>, align: usize) {
        while !data.len().is_multiple_of(align) {
            data.push(0);
        }
    }

    /// One block of one track with a static position (x and z), an uncompressed static rotation and no scale.
    fn static_block(x: f32, z: f32) -> Vec<u8> {
        // quantization: rotation 5 (uncompressed); position x z static; rotation static; scale absent
        let mut d = vec![5 << 2, 0x01 | 0x04, 0x01, 0x00];
        d.extend(f(x));
        d.extend(f(z));
        pad(&mut d, 4);
        d.extend([f(0.0), f(1.0), f(0.0), f(0.0)].concat());
        d
    }

    #[test]
    fn a_static_track_states_what_the_mask_names_and_the_reference_stands_for_the_rest() {
        let mut d = static_block(10.0, 30.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 2);
        let s = SplineAnimation::parse(&a).unwrap();
        let t = s.sample(0, 0.0, &IDENTITY, MIXED).unwrap();
        // x and z from the file, y from the reference; the rotation as stored; the scale the reference's
        assert_eq!(t.translation, [10.0, 2.0, 30.0, 0.0]);
        assert_eq!(t.rotation, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(t.scale, [1.0, 1.0, 1.0, 0.0]);
    }

    /// A stored value that is NaN is that component's own: it reads as absent (the caller's `absent` stands for it) and
    /// is named in `unreadable`, where it used to refuse the whole animation (Elden Ring's `c2030/a001_009000`).
    #[test]
    fn a_nan_component_is_read_as_absent_and_named() {
        let mut d = static_block(f32::NAN, 5.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 2);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(
            s.unreadable,
            [Unreadable {
                block: 0,
                track: 0,
                part: "position x"
            }]
        );
        let t = s.sample(0, 0.0, &IDENTITY, Absent::NEUTRAL).unwrap();
        // x absent (neutral zero), z as stored
        assert_eq!(t.translation, [0.0, 0.0, 5.0, 0.0]);
        // an infinite one too, and a finite animation has none
        let mut d = static_block(f32::INFINITY, 0.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        assert_eq!(
            SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2))
                .unwrap()
                .unreadable
                .len(),
            1
        );
        let mut d = static_block(1.0, 2.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        assert!(
            SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2))
                .unwrap()
                .unreadable
                .is_empty()
        );
    }

    /// A block that ends elsewhere than the data says is refused naming both places.
    #[test]
    fn a_block_must_end_where_the_float_data_begins() {
        let mut d = static_block(0.0, 0.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d.clone(), 1, &[(0, end + 4)], 256, 2);
        let e = SplineAnimation::parse(&a).unwrap_err().to_string();
        assert!(e.contains("float data begins"), "{e}");
        let a = animation(d, 1, &[(0, end)], 256, 2);
        assert!(SplineAnimation::parse(&a).is_ok());
    }

    /// A block of 32 bytes (a static position of three components and an uncompressed static rotation), so that its
    /// float data begins on a 16-byte boundary.
    fn aligned_block(x: f32) -> Vec<u8> {
        let mut d = vec![5 << 2, 0x01 | 0x02 | 0x04, 0x01, 0x00];
        d.extend([f(x), f(0.0), f(0.0)].concat());
        d.extend([f(0.0), f(1.0), f(0.0), f(0.0)].concat());
        assert_eq!(d.len(), 32);
        d
    }

    /// Two aligned blocks with `between` between them: the float data of each begins at 32 of its own.
    fn two_aligned_blocks(between: &[u8]) -> SplineAnimation {
        let mut d = aligned_block(1.0);
        d.extend(between);
        let second = d.len() as u32;
        d.extend(aligned_block(2.0));
        let a = animation(d, 1, &[(0, 32), (second, 32)], 3, 5);
        SplineAnimation::parse(&a).unwrap()
    }

    /// The padding between two blocks whose float data begins on a 16-byte boundary: none (the 2016.2 files, and some
    /// 2018.1 ones) or 16 zero bytes (other 2018.1 ones) are the data's; anything else is not.
    #[test]
    fn a_block_ending_on_a_boundary_may_be_followed_by_sixteen_zero_bytes() {
        let none = two_aligned_blocks(&[]);
        let sixteen = two_aligned_blocks(&[0; 16]);
        assert_eq!(
            none.sample(0, 3.0, &IDENTITY, MIXED).unwrap().translation,
            sixteen.sample(0, 3.0, &IDENTITY, MIXED).unwrap().translation
        );
        for between in [vec![0u8; 8], vec![0u8; 32], {
            let mut v = vec![0u8; 16];
            v[5] = 1;
            v
        }] {
            let mut d = aligned_block(1.0);
            d.extend(&between);
            let second = d.len() as u32;
            d.extend(aligned_block(2.0));
            let a = animation(d, 1, &[(0, 32), (second, 32)], 3, 5);
            assert!(SplineAnimation::parse(&a).is_err(), "{between:?}");
        }
    }

    /// A position spline of degree 1 over three control points, x 8-bit between 0 and 10, y static 7.5, z absent: the
    /// control points 0, 10, 0 at knots [0 0 5 10 10] are a ramp up over frames 0 to 5 and down over 5 to 10.
    #[test]
    fn a_spline_position_is_evaluated_between_its_knots() {
        // the position flags: x spline, y static, z nothing
        let mut d = vec![0x00, 0x10 | 0x02, 0x00, 0x00];
        pad(&mut d, 4);
        d.extend(2i16.to_le_bytes());
        d.push(1);
        d.extend([0, 0, 5, 10, 10]);
        pad(&mut d, 4);
        d.extend(f(0.0));
        d.extend(f(10.0)); // x bounds
        d.extend(f(7.5)); // y static (the y flag is static)
        d.extend([0, 255, 0]); // x control points
        pad(&mut d, 4);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 11);
        let s = SplineAnimation::parse(&a).unwrap();
        let at = |t: f32| s.sample(0, t, &IDENTITY, MIXED).unwrap().translation;
        assert_eq!(at(0.0), [0.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(2.5), [5.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(5.0), [10.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(7.5), [5.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(10.0), [0.0, 7.5, 3.0, 0.0]);
        // past the end is the end
        assert_eq!(at(99.0), [0.0, 7.5, 3.0, 0.0]);
    }

    /// The identity in three 12-bit components and the index 3 (w), and the 180 degrees about z the polar form's
    /// radius 1023 with no angle is, as the formulas state them.
    #[test]
    fn the_quantized_quaternions_decode_by_their_bit_layouts() {
        let c: u64 = 0x7FF | (0x7FF << 12) | (0x7FF << 24) | (3 << 36);
        let bytes = c.to_le_bytes();
        let mut r = Reader {
            bytes: &bytes[..5],
            at: 0,
        };
        assert_eq!(
            quaternion(&mut r, RotationQuantization::ThreeComp40).unwrap(),
            [0.0, 0.0, 0.0, 1.0]
        );
        // the sign bit makes w -1
        let c = c | (1 << 38);
        let bytes = c.to_le_bytes();
        let mut r = Reader {
            bytes: &bytes[..5],
            at: 0,
        };
        assert_eq!(
            quaternion(&mut r, RotationQuantization::ThreeComp40).unwrap(),
            [0.0, 0.0, 0.0, -1.0]
        );
        // 15-bit components at 0x3FFF are zero: the index is bit 15 of x and of y (x low, y high); index 0 is x
        let words = [0x3FFFu16, 0x3FFF, 0x3FFF];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut r = Reader { bytes: &bytes, at: 0 };
        assert_eq!(
            quaternion(&mut r, RotationQuantization::ThreeComp48).unwrap(),
            [1.0, 0.0, 0.0, 0.0]
        );
        // both bits set is index 3 (w); the sign bit of z negates it
        let words = [0xBFFFu16, 0xBFFF, 0xBFFF];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut r = Reader { bytes: &bytes, at: 0 };
        assert_eq!(
            quaternion(&mut r, RotationQuantization::ThreeComp48).unwrap(),
            [0.0, 0.0, 0.0, -1.0]
        );
        let bytes = (1023u32 << 18).to_le_bytes();
        let mut r = Reader { bytes: &bytes, at: 0 };
        let q = quaternion(&mut r, RotationQuantization::Polar32).unwrap();
        assert_eq!(q, [0.0, 0.0, 1.0, 0.0]);
    }

    /// Two blocks of two frames each (maxFramesPerBlock 3 gives 2 frames a block): a time picks the block that covers
    /// it, and the last frame is the last block's end.
    #[test]
    fn a_time_is_located_in_its_block() {
        let mut d = static_block(1.0, 0.0);
        let first_end = d.len() as u32;
        pad(&mut d, 16);
        let second = d.len() as u32;
        d.extend(static_block(2.0, 0.0));
        pad(&mut d, 16);
        // each block's float offset counts from the block's own start
        let a = animation(d, 1, &[(0, first_end), (second, first_end)], 3, 5);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(s.locate(0.5), (0, 0.5));
        assert_eq!(s.locate(1.9), (0, 1.9));
        assert_eq!(s.locate(2.0), (1, 0.0));
        assert_eq!(s.locate(4.0), (1, 2.0));
        assert_eq!(s.locate(99.0), (1, 2.0));
        assert_eq!(s.sample(0, 1.5, &IDENTITY, MIXED).unwrap().translation[0], 1.0);
        assert_eq!(s.sample(0, 3.0, &IDENTITY, MIXED).unwrap().translation[0], 2.0);
    }

    #[test]
    fn what_is_not_read_is_refused_by_name() {
        // rotation quantization 3 (THREECOMP24)
        let mut d = vec![3 << 2, 0, 0x01, 0];
        d.extend([0, 0, 0]);
        pad(&mut d, 4);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 2);
        assert!(
            SplineAnimation::parse(&a)
                .unwrap_err()
                .to_string()
                .contains("THREECOMP24")
        );
        let mut a = animation(vec![0; 16], 1, &[(0, 4)], 256, 2);
        a.float_tracks = 1;
        assert!(
            SplineAnimation::parse(&a)
                .unwrap_err()
                .to_string()
                .contains("float tracks")
        );
    }
}
