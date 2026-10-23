//! Unity AnimationClips: the curves of a Mecanim clip (`m_MuscleClip`) decoded to keys with tangents, and the
//! transform curves of a clip lowered to a `core.clip` for the joints of a skeleton.
//!
//! A clip stores its curves in three blocks, numbered in this order: the **streamed** curves (`m_StreamedClip`), the
//! **dense** curves (`m_DenseClip`, one sample per frame at a fixed rate) and the **constant** curves
//! (`m_ConstantClip`, one value each). `m_ClipBindingConstant.genericBindings` lists what the curves drive, in curve
//! order: a binding takes one curve (a float property), or for a Transform the curves of its attribute, 1 position (3),
//! 2 rotation, a quaternion (4), 3 scale (3), 4 Euler angles (3). A binding names its target by `path`, the CRC-32 of the
//! path of GameObject names below the Animator's own (the Animator's GameObject itself is the empty path, hash 0).
//!
//! The streamed block is a sequence of frames, each `time: f32`, `count: i32` and `count` keys of `index: i32` and four
//! `f32` coefficients; a key opens a cubic segment of its curve that lasts until the curve's next key:
//! `value(dt) = ((c0 dt + c1) dt + c2) dt + c3` for `dt` seconds after the key, so `c3` is the key's value and `c2` its
//! outgoing slope; the incoming slope of the next key is the derivative at the segment's end. The first frame is at
//! `-f32::MAX` and gives every curve's initial value; the last is at `+inf` with no keys. A segment whose coefficients are
//! not finite is a step.

use std::collections::BTreeSet;

use primase_core::{
    Channel, Clip, ClipTarget, Entity, Extrapolation, Interpolation, JointId, Property, Rest,
};

use crate::components::UNITY;
use crate::read::player::{ObjRef, Player};
use crate::read::value::{Array, Value};
use crate::{Error, Result, invalid};

/// One curve binding of a clip (`genericBindings`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericBinding {
    /// The CRC-32 of the target's path below the Animator's GameObject.
    pub path: u32,
    /// The attribute: for a Transform (`type_id` 4) 1 position, 2 rotation, 3 scale, 4 Euler angles; else the
    /// property's hash.
    pub attribute: u32,
    /// The target's class id (4 Transform, 95 Animator, 137 SkinnedMeshRenderer, ...).
    pub type_id: i64,
    pub is_pptr: bool,
}

impl GenericBinding {
    /// How many curves this binding takes.
    fn width(&self) -> usize {
        match (self.type_id, self.attribute) {
            (4, 1 | 3 | 4) => 3,
            (4, 2) => 4,
            _ => 1,
        }
    }
}

/// Where a curve was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Streamed,
    Dense,
    Constant,
}

/// A key of a curve: its time (seconds), value and the slopes (per second) into and out of it. An infinite outgoing
/// slope is a step to the next key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
}

/// One scalar curve: keys in increasing time, evaluated as a cubic Hermite spline between them (a dense curve's slopes
/// are its secants, so it is piecewise linear).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub storage: Storage,
    pub keys: Vec<Key>,
}

impl Curve {
    /// The value at `t` and the slopes into and out of it: a key's own at its time, a segment's derivative inside it,
    /// the end values held with no slope outside. A step segment holds its key's value.
    pub fn sample(&self, t: f32) -> (f32, f32, f32) {
        let k = &self.keys;
        let (Some(first), Some(last)) = (k.first(), k.last()) else {
            return (0.0, 0.0, 0.0);
        };
        if t <= first.time {
            return (
                first.value,
                0.0,
                if t == first.time { first.out_slope } else { 0.0 },
            );
        }
        if t >= last.time {
            return (last.value, if t == last.time { last.in_slope } else { 0.0 }, 0.0);
        }
        let i = k.partition_point(|key| key.time <= t) - 1;
        let (a, b) = (&k[i], &k[i + 1]);
        if t == a.time {
            return (a.value, a.in_slope, a.out_slope);
        }
        if !a.out_slope.is_finite() {
            return (a.value, 0.0, 0.0);
        }
        let (dt, s) = (
            f64::from(b.time - a.time),
            f64::from(t - a.time) / f64::from(b.time - a.time),
        );
        let (p0, p1) = (f64::from(a.value), f64::from(b.value));
        let (m0, m1) = (f64::from(a.out_slope) * dt, f64::from(b.in_slope) * dt);
        let (s2, s3) = (s * s, s * s * s);
        let value = (2.0 * s3 - 3.0 * s2 + 1.0) * p0
            + (s3 - 2.0 * s2 + s) * m0
            + (-2.0 * s3 + 3.0 * s2) * p1
            + (s3 - s2) * m1;
        let slope = ((6.0 * s2 - 6.0 * s) * p0
            + (3.0 * s2 - 4.0 * s + 1.0) * m0
            + (-6.0 * s2 + 6.0 * s) * p1
            + (3.0 * s2 - 2.0 * s) * m1)
            / dt;
        (value as f32, slope as f32, slope as f32)
    }

    /// Whether any slope is a step.
    fn has_steps(&self) -> bool {
        self.keys.iter().any(|k| !k.out_slope.is_finite())
    }

    fn is_constant(&self) -> bool {
        self.keys.len() == 1
    }
}

/// A binding with the curves it takes, in the order of its attribute's components.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub binding: GenericBinding,
    pub curves: Vec<Curve>,
}

/// An AnimationClip's curves as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipData {
    pub name: String,
    /// Seconds, `m_StopTime - m_StartTime`.
    pub duration: f32,
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// Animation events (`m_Events`): not read.
    pub events: usize,
    /// How far a streamed segment's end was from the next key's value, the worst over the clip: the decode's own
    /// consistency (the segment is the cubic the next key continues).
    pub worst_continuity_error: f32,
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("clip: no integer {k}")))
}

fn float(v: &Value, k: &str) -> Result<f32> {
    v.get(k)
        .and_then(Value::as_f32)
        .ok_or_else(|| invalid(format!("clip: no float {k}")))
}

fn list<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match v.get(k) {
        Some(Value::Array(Array::Values(items))) => Ok(items),
        Some(Value::Array(a)) if a.is_empty() => Ok(&[]),
        _ => Err(invalid(format!("clip: no list {k}"))),
    }
}

fn floats(v: &Value, k: &str) -> Result<Vec<f32>> {
    match v.get(k) {
        Some(Value::Array(Array::F32(f))) => Ok(f.clone()),
        Some(Value::Array(a)) if a.is_empty() => Ok(Vec::new()),
        _ => Err(invalid(format!("clip: no float list {k}"))),
    }
}

/// The streamed block's curves: `curve_count` curves from the frames in `words`.
fn read_streamed(name: &str, words: &[u32], curve_count: usize) -> Result<(Vec<Curve>, f32)> {
    let bad = |why: String| invalid(format!("{name}: streamed clip: {why}"));
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    // each curve's entries: the time and the four coefficients of the cubic that starts there
    let mut entries: Vec<Vec<(f32, [f32; 4])>> = vec![Vec::new(); curve_count];
    let mut at = 0;
    let take = |at: &mut usize, n: usize| -> Result<&[u8]> {
        let part = bytes
            .get(*at..*at + n)
            .ok_or_else(|| bad(format!("frames past the data at byte {at}", at = *at)))?;
        *at += n;
        Ok(part)
    };
    let mut last_time = f32::NEG_INFINITY;
    let mut ended = false;
    while at < bytes.len() {
        let time = f32::from_le_bytes(take(&mut at, 4)?.try_into().expect("4 bytes"));
        let count = i32::from_le_bytes(take(&mut at, 4)?.try_into().expect("4 bytes"));
        if time.is_nan() || time < last_time {
            return Err(bad(format!("frame time {time} after {last_time}")));
        }
        last_time = time;
        let count = usize::try_from(count).map_err(|_| bad(format!("{count} keys in a frame")))?;
        for _ in 0..count {
            let index = i32::from_le_bytes(take(&mut at, 4)?.try_into().expect("4 bytes"));
            let mut c = [0.0f32; 4];
            for x in &mut c {
                *x = f32::from_le_bytes(take(&mut at, 4)?.try_into().expect("4 bytes"));
            }
            let index = usize::try_from(index)
                .ok()
                .filter(|i| *i < curve_count)
                .ok_or_else(|| bad(format!("a key of curve {index} of {curve_count}")))?;
            entries[index].push((time, c));
        }
        if time == f32::INFINITY {
            ended = true;
            break;
        }
    }
    if !ended || at != bytes.len() {
        return Err(bad(
            "the frames do not end with the +inf frame at the end of the data".into(),
        ));
    }
    let mut worst = 0.0f32;
    let mut out = Vec::with_capacity(curve_count);
    for (i, list) in entries.iter().enumerate() {
        // the initial entry (at -f32::MAX) is the value before the first key, which the first key repeats
        let real: Vec<&(f32, [f32; 4])> = list
            .iter()
            .filter(|(t, _)| *t > f32::MIN / 2.0 && t.is_finite())
            .collect();
        let mut keys: Vec<Key> = Vec::new();
        if real.is_empty() {
            let (_, c) = list.first().ok_or_else(|| bad(format!("curve {i} has no key")))?;
            keys.push(Key {
                time: 0.0,
                value: c[3],
                in_slope: 0.0,
                out_slope: 0.0,
            });
        }
        let mut previous: Option<(f32, [f32; 4])> = None;
        for &&(time, c) in &real {
            let in_slope = match previous {
                Some((t0, p)) => {
                    let dt = time - t0;
                    // the cubic of the previous key at this key
                    let end = ((p[0] * dt + p[1]) * dt + p[2]) * dt + p[3];
                    if end.is_finite() && c[3].is_finite() {
                        worst = worst.max((end - c[3]).abs() / (1.0 + c[3].abs()));
                    }
                    (3.0 * p[0] * dt + 2.0 * p[1]) * dt + p[2]
                }
                None => 0.0,
            };
            keys.push(Key {
                time,
                value: c[3],
                in_slope,
                out_slope: c[2],
            });
            previous = Some((time, c));
        }
        // a key whose segment is a step has non-finite coefficients: its slopes are a step
        for k in &mut keys {
            if !k.in_slope.is_finite() {
                k.in_slope = f32::INFINITY;
            }
        }
        out.push(Curve {
            storage: Storage::Streamed,
            keys,
        });
    }
    Ok((out, worst))
}

impl ClipData {
    /// The clip object `r`. Legacy clips (`m_Legacy`, curves stored per kind) are not read.
    pub fn read(player: &Player, r: &ObjRef) -> Result<ClipData> {
        let c = player.decode(r)?;
        let name = c.get("m_Name").and_then(Value::as_str).unwrap_or("").to_owned();
        if c.get("m_Legacy")
            .is_some_and(|l| matches!(l, Value::Bool(true)) || l.as_i64().is_some_and(|v| v != 0))
        {
            return Err(Error::Unsupported(format!("{name}: legacy animation clip")));
        }
        let muscle = c
            .get("m_MuscleClip")
            .ok_or_else(|| invalid(format!("{name}: no m_MuscleClip")))?;
        let data = muscle
            .at(&["m_Clip", "data"])
            .ok_or_else(|| invalid(format!("{name}: no m_MuscleClip.m_Clip.data")))?;
        let streamed = data
            .get("m_StreamedClip")
            .ok_or_else(|| invalid(format!("{name}: no m_StreamedClip")))?;
        let dense = data
            .get("m_DenseClip")
            .ok_or_else(|| invalid(format!("{name}: no m_DenseClip")))?;
        let constant = data
            .get("m_ConstantClip")
            .ok_or_else(|| invalid(format!("{name}: no m_ConstantClip")))?;
        let start = float(muscle, "m_StartTime")?;
        let stop = float(muscle, "m_StopTime")?;
        if start != 0.0 {
            return Err(Error::Unsupported(format!(
                "{name}: a clip starting at {start} s"
            )));
        }
        let streamed_words = match streamed.get("data") {
            Some(Value::Array(Array::U32(w))) => w.as_slice(),
            Some(Value::Array(a)) if a.is_empty() => &[],
            _ => return Err(invalid(format!("{name}: streamed clip data is not 32-bit words"))),
        };
        let streamed_count = usize::try_from(int(streamed, "curveCount")?)
            .map_err(|_| invalid(format!("{name}: negative curve count")))?;
        let (mut curves, worst) = if streamed_count == 0 && streamed_words.is_empty() {
            (Vec::new(), 0.0)
        } else {
            read_streamed(&name, streamed_words, streamed_count)?
        };
        // dense: frame f of curve c is sample[f * curves + c], at begin + f / rate
        let dense_curves = usize::try_from(int(dense, "m_CurveCount")?)
            .map_err(|_| invalid(format!("{name}: negative dense curve count")))?;
        let frames = usize::try_from(int(dense, "m_FrameCount")?)
            .map_err(|_| invalid(format!("{name}: negative frame count")))?;
        let samples = floats(dense, "m_SampleArray")?;
        if samples.len() != frames * dense_curves {
            return Err(invalid(format!(
                "{name}: dense clip: {} samples for {frames} frames of {dense_curves} curves",
                samples.len()
            )));
        }
        let (rate, begin) = (float(dense, "m_SampleRate")?, float(dense, "m_BeginTime")?);
        for c in 0..dense_curves {
            let values: Vec<f32> = (0..frames).map(|f| samples[f * dense_curves + c]).collect();
            let times: Vec<f32> = (0..frames).map(|f| begin + f as f32 / rate).collect();
            let slope = |a: usize, b: usize| (values[b] - values[a]) / (times[b] - times[a]);
            curves.push(Curve {
                storage: Storage::Dense,
                keys: (0..frames)
                    .map(|f| Key {
                        time: times[f],
                        value: values[f],
                        in_slope: if f > 0 { slope(f - 1, f) } else { 0.0 },
                        out_slope: if f + 1 < frames { slope(f, f + 1) } else { 0.0 },
                    })
                    .collect(),
            });
        }
        for v in floats(constant, "data")? {
            curves.push(Curve {
                storage: Storage::Constant,
                keys: vec![Key {
                    time: 0.0,
                    value: v,
                    in_slope: 0.0,
                    out_slope: 0.0,
                }],
            });
        }
        // the bindings take the curves in order
        let mut bindings = Vec::new();
        let constant_bindings = c
            .get("m_ClipBindingConstant")
            .ok_or_else(|| invalid(format!("{name}: no m_ClipBindingConstant")))?;
        for b in list(constant_bindings, "genericBindings")? {
            bindings.push(GenericBinding {
                path: u32::try_from(int(b, "path")?)
                    .map_err(|_| invalid(format!("{name}: a path hash beyond 32 bits")))?,
                attribute: u32::try_from(int(b, "attribute")?)
                    .map_err(|_| invalid(format!("{name}: an attribute beyond 32 bits")))?,
                type_id: int(b, "typeID")?,
                is_pptr: int(b, "isPPtrCurve")? != 0,
            });
        }
        if let Some(b) = bindings.iter().find(|b| b.is_pptr) {
            return Err(Error::Unsupported(format!(
                "{name}: a PPtr curve (attribute {})",
                b.attribute
            )));
        }
        let wanted: usize = bindings.iter().map(GenericBinding::width).sum();
        if wanted != curves.len() {
            return Err(invalid(format!(
                "{name}: the bindings take {wanted} curves, the clip stores {}",
                curves.len()
            )));
        }
        let mut rest = curves.into_iter();
        let tracks = bindings
            .into_iter()
            .map(|binding| {
                let curves = rest.by_ref().take(binding.width()).collect();
                Track { binding, curves }
            })
            .collect();
        let looping = muscle
            .get("m_LoopTime")
            .is_some_and(|l| matches!(l, Value::Bool(true)) || l.as_i64().is_some_and(|v| v != 0));
        Ok(ClipData {
            name,
            duration: stop - start,
            looping,
            tracks,
            events: list(&c, "m_Events")?.len(),
            worst_continuity_error: worst,
        })
    }
}

/// The hash a clip's binding names a path by: the CRC-32 (the zlib one) of the path of GameObject names below the
/// Animator's GameObject, empty for the GameObject itself.
pub fn path_hash(path: &str) -> u32 {
    let mut crc = !0u32;
    for byte in path.bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// The clips an AnimatorController lists (`m_AnimationClips`), in order, null entries left out.
pub fn controller_clips(player: &Player, controller: &ObjRef) -> Result<Vec<ObjRef>> {
    let c = player.decode(controller)?;
    let mut out = Vec::new();
    for p in list(&c, "m_AnimationClips")? {
        if let Some(clip) = player.resolve(&controller.file, p)? {
            out.push(clip);
        }
    }
    Ok(out)
}

/// What lowering a clip did: channels made, bindings left out by reason.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClipLowering {
    pub channels: usize,
    pub keys: usize,
    /// Rotation keys renormalised to unit length (they were not within 1e-4 of it).
    pub renormalised_keys: usize,
    /// Transform curves whose path is no joint of the rig, by attribute (`position`, `rotation`, `scale`).
    pub unresolved: std::collections::BTreeMap<&'static str, usize>,
    /// Bindings with no joint channel in core, by what they are.
    pub not_lowered: std::collections::BTreeMap<&'static str, usize>,
}

/// A joint-channel value or tangent of `property` in the world's frame.
fn convert(property: Property, v: &[f32]) -> Vec<f32> {
    let rest = match property {
        Property::Translation => Rest::Trs {
            translation: [v[0], v[1], v[2]],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Property::Rotation => Rest::Trs {
            translation: [0.0; 3],
            rotation: [v[0], v[1], v[2], v[3]],
            scale: [1.0; 3],
        },
        _ => Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [v[0], v[1], v[2]],
        },
    };
    match rest.converted(&UNITY) {
        Rest::Trs {
            translation,
            rotation,
            scale,
        } => match property {
            Property::Translation => translation.to_vec(),
            Property::Rotation => rotation.to_vec(),
            _ => scale.to_vec(),
        },
        Rest::Affine(_) => unreachable!("a TRS converts to a TRS"),
    }
}

impl Track {
    /// This transform track as a joint channel on `target`, in the world's frame: its curves sampled at the union of
    /// their key times, as a Hermite spline (exact: each curve is a cubic between its keys), as a Linear channel when
    /// every curve is dense or constant, and as a one-key channel when every curve is constant. `None` for a
    /// binding that is not position, rotation or scale of a Transform. An error for steps mixed with smooth segments.
    pub fn channel(
        &self,
        target: ClipTarget,
        duration: f32,
        renormalised: &mut usize,
    ) -> Result<Option<Channel>> {
        let property = match (self.binding.type_id, self.binding.attribute) {
            (4, 1) => Property::Translation,
            (4, 2) => Property::Rotation,
            (4, 3) => Property::Scale,
            _ => return Ok(None),
        };
        let width = self.curves.len();
        let steps = self.curves.iter().filter(|c| c.has_steps()).count();
        if steps > 0 {
            return Err(Error::Unsupported(format!("a {property:?} curve with steps")));
        }
        let mut times: Vec<f32> = self
            .curves
            .iter()
            .flat_map(|c| c.keys.iter().map(|k| k.time))
            .collect();
        // a key a rounding beyond the clip's end is at its end
        times.retain(|t| *t <= duration + 1e-4);
        times.iter_mut().for_each(|t| *t = t.min(duration));
        times.sort_by(f32::total_cmp);
        times.dedup();
        let all_constant = self.curves.iter().all(Curve::is_constant);
        let all_linear = self.curves.iter().all(|c| c.storage != Storage::Streamed);
        let interpolation = if all_constant || all_linear {
            Interpolation::Linear
        } else {
            Interpolation::Hermite
        };
        if all_constant {
            times.truncate(1);
        }
        let mut values = Vec::new();
        for (n, &t) in times.iter().enumerate() {
            let samples: Vec<(f32, f32, f32)> = self.curves.iter().map(|c| c.sample(t)).collect();
            let mut value: Vec<f32> = samples.iter().map(|s| s.0).collect();
            if property == Property::Rotation {
                let length = value.iter().map(|x| x * x).sum::<f32>().sqrt();
                if (length - 1.0).abs() > 1e-4 && length > 0.0 {
                    value.iter_mut().for_each(|x| *x /= length);
                    *renormalised += 1;
                }
            }
            if interpolation == Interpolation::Hermite {
                let before = if n > 0 { t - times[n - 1] } else { 0.0 };
                let after = if n + 1 < times.len() {
                    times[n + 1] - t
                } else {
                    0.0
                };
                let in_tangent: Vec<f32> = samples.iter().map(|s| s.1 * before).collect();
                let out_tangent: Vec<f32> = samples.iter().map(|s| s.2 * after).collect();
                values.extend(convert(property, &in_tangent));
                values.extend(convert(property, &value));
                values.extend(convert(property, &out_tangent));
            } else {
                values.extend(convert(property, &value));
            }
        }
        debug_assert_eq!(width, if property == Property::Rotation { 4 } else { 3 });
        Ok(Some(Channel {
            target,
            property,
            interpolation,
            times,
            values,
        }))
    }
}

impl ClipData {
    /// The clip as a `core.clip` for a rig: the position, rotation and scale curves of the Transforms `resolve` finds
    /// (a path hash to the skeleton and joint it is), in the world's frame; the rest counted in the returned
    /// [`ClipLowering`] by reason.
    pub fn lower(&self, resolve: &dyn Fn(u32) -> Option<(Entity, JointId)>) -> Result<(Clip, ClipLowering)> {
        let mut report = ClipLowering::default();
        let mut channels = Vec::new();
        let mut seen = BTreeSet::new();
        for track in &self.tracks {
            let b = &track.binding;
            let kind = match (b.type_id, b.attribute) {
                (4, 1) => "position",
                (4, 2) => "rotation",
                (4, 3) => "scale",
                (4, 4) => {
                    *report
                        .not_lowered
                        .entry("Euler angle curves of a Transform")
                        .or_default() += 1;
                    continue;
                }
                (95, _) => {
                    *report
                        .not_lowered
                        .entry("Animator (humanoid muscle) curves")
                        .or_default() += 1;
                    continue;
                }
                (137, _) => {
                    *report
                        .not_lowered
                        .entry("SkinnedMeshRenderer blend shape curves")
                        .or_default() += 1;
                    continue;
                }
                _ => {
                    *report
                        .not_lowered
                        .entry("curves of other components")
                        .or_default() += 1;
                    continue;
                }
            };
            let Some((skeleton, joint)) = resolve(b.path) else {
                *report.unresolved.entry(kind).or_default() += 1;
                continue;
            };
            let target = ClipTarget::Joint { skeleton, joint };
            let channel = track
                .channel(target, self.duration, &mut report.renormalised_keys)?
                .expect("a position, rotation or scale curve is a channel");
            if !seen.insert((joint, channel.property)) {
                return Err(invalid(format!(
                    "{}: two {:?} channels for one joint ({:?})",
                    self.name, channel.property, joint
                )));
            }
            report.keys += channel.times.len();
            channels.push(channel);
        }
        report.channels = channels.len();
        Ok((
            Clip {
                name: self.name.clone(),
                duration: self.duration,
                extrapolation: if self.looping {
                    Extrapolation::Loop
                } else {
                    Extrapolation::Hold
                },
                channels,
                root_motion: None,
                additive: None,
            },
            report,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: f32, value: f32, in_slope: f32, out_slope: f32) -> Key {
        Key {
            time,
            value,
            in_slope,
            out_slope,
        }
    }

    fn frame(time: f32, keys: &[(i32, [f32; 4])]) -> Vec<u32> {
        let mut words = vec![time.to_bits(), keys.len() as u32];
        for (index, c) in keys {
            words.push(*index as u32);
            words.extend(c.iter().map(|x| x.to_bits()));
        }
        words
    }

    /// The initial frame, two keys of one curve and the end frame: the keys' values, outgoing slopes, the incoming
    /// slope of the second as the first cubic's derivative at its end, and the continuity of the decode.
    #[test]
    fn a_streamed_curve_decodes_to_keys_with_slopes() {
        // a cubic from 1 at t=0 over 2 s: v(dt) = 0.5 dt^3 - dt^2 + 3 dt + 1; its end value 1+6-4+4=7... at dt = 2
        let (c0, c1, c2, c3) = (0.5f32, -1.0, 3.0, 1.0);
        let end = ((c0 * 2.0 + c1) * 2.0 + c2) * 2.0 + c3;
        let mut words = frame(-f32::MAX, &[(0, [0.0, 0.0, 0.0, 1.0])]);
        words.extend(frame(0.0, &[(0, [c0, c1, c2, c3])]));
        words.extend(frame(2.0, &[(0, [0.0, 0.0, 0.5, end])]));
        words.extend(frame(f32::INFINITY, &[]));
        let (curves, worst) = read_streamed("t", &words, 1).unwrap();
        assert_eq!(worst, 0.0);
        let k = &curves[0].keys;
        assert_eq!(k.len(), 2);
        assert_eq!((k[0].time, k[0].value, k[0].out_slope), (0.0, 1.0, 3.0));
        // the derivative at the end of the first segment: 3 c0 dt^2 + 2 c1 dt + c2 = 6 - 4 + 3
        assert_eq!(
            (k[1].time, k[1].value, k[1].in_slope, k[1].out_slope),
            (2.0, end, 5.0, 0.5)
        );
        // the Hermite spline through these keys is the stored cubic
        let (v, s, _) = curves[0].sample(1.0);
        assert!((v - (0.5 - 1.0 + 3.0 + 1.0)).abs() < 1e-5, "{v}");
        assert!((s - (1.5 - 2.0 + 3.0)).abs() < 1e-5, "{s}");
        assert_eq!(curves[0].sample(0.0), (1.0, 0.0, 3.0));
        assert_eq!(curves[0].sample(2.0), (end, 5.0, 0.0));
    }

    /// A curve with only its initial entry is constant; frames that do not end at +inf, a key of a curve beyond the
    /// count and times going back are refused.
    #[test]
    fn streamed_clips_refuse_what_is_not_the_format() {
        let mut words = frame(-f32::MAX, &[(0, [0.0, 0.0, 0.0, 4.0])]);
        words.extend(frame(f32::INFINITY, &[]));
        let (curves, _) = read_streamed("t", &words, 1).unwrap();
        assert_eq!(curves[0].keys, vec![key(0.0, 4.0, 0.0, 0.0)]);
        let refused = |words: &[u32], n: usize| read_streamed("t", words, n).unwrap_err().to_string();
        assert!(refused(&words[..words.len() - 2], 1).contains("do not end"));
        assert!(refused(&words, 0).contains("curve 0 of 0"));
        let mut back = frame(1.0, &[]);
        back.extend(frame(0.5, &[]));
        back.extend(frame(f32::INFINITY, &[]));
        assert!(refused(&back, 1).contains("after"));
    }

    /// A Hermite curve samples to its keys, and a transform track of three curves with different key times becomes
    /// one Hermite channel at the union of the times whose pieces are the same cubics; constants give one key, dense
    /// curves a Linear channel.
    #[test]
    fn transform_tracks_become_joint_channels() {
        let binding = |attribute| GenericBinding {
            path: 1,
            attribute,
            type_id: 4,
            is_pptr: false,
        };
        let line = |a: f32, b: f32| Curve {
            storage: Storage::Streamed,
            keys: vec![key(0.0, a, 0.0, b - a), key(1.0, b, b - a, 0.0)],
        };
        let mut world = primase_core::World::new();
        let target = ClipTarget::Joint {
            skeleton: world.spawn(),
            joint: JointId(0),
        };
        let track = Track {
            binding: binding(1),
            curves: vec![
                line(0.0, 1.0),
                Curve {
                    storage: Storage::Constant,
                    keys: vec![key(0.0, 5.0, 0.0, 0.0)],
                },
                Curve {
                    storage: Storage::Streamed,
                    keys: vec![
                        key(0.0, 0.0, 0.0, 2.0),
                        key(0.5, 1.0, 2.0, 2.0),
                        key(1.0, 2.0, 2.0, 0.0),
                    ],
                },
            ],
        };
        let mut renormalised = 0;
        let c = track
            .channel(target.clone(), 1.0, &mut renormalised)
            .unwrap()
            .unwrap();
        assert_eq!(c.property, Property::Translation);
        assert_eq!(c.interpolation, Interpolation::Hermite);
        assert_eq!(c.times, vec![0.0, 0.5, 1.0]);
        // 9 values a key: in tangents, value, out tangents; x runs 0 to 1 and z 0 to 2 along straight lines
        assert_eq!(c.values.len(), 27);
        let key1 = &c.values[9..18];
        assert!(
            (key1[3] - 0.5).abs() < 1e-6 && (key1[4] - 5.0).abs() < 1e-6 && (key1[5] + 1.0).abs() < 1e-6,
            "{key1:?}"
        );
        let constant = Track {
            binding: binding(3),
            curves: vec![
                Curve {
                    storage: Storage::Constant,
                    keys: vec![key(0.0, 1.0, 0.0, 0.0)]
                };
                3
            ],
        };
        let c = constant
            .channel(target.clone(), 1.0, &mut renormalised)
            .unwrap()
            .unwrap();
        assert_eq!(
            (c.interpolation, c.times.len(), c.values.len()),
            (Interpolation::Linear, 1, 3)
        );
        assert!(
            Track {
                binding: binding(4),
                curves: vec![],
            }
            .channel(target, 1.0, &mut renormalised)
            .unwrap()
            .is_none()
        );
    }

    /// The path hash is the zlib CRC-32: values from the SkateBIRD avatar's name table.
    #[test]
    fn path_hashes_are_the_crc32_of_the_path() {
        assert_eq!(path_hash(""), 0);
        assert_eq!(path_hash("scarf"), 2_702_068_705);
        assert_eq!(path_hash("root/Lchain1/Lchain2/Lchain3"), 3_634_936_137);
        assert_eq!(path_hash("root/Rchain1/Rchain2"), 356_844_446);
    }
}
