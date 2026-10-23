//! A decoded spline animation sampled at a time: the block and frame a time falls in, each track's components
//! evaluated on its B-spline basis (*The NURBS Book*, algorithms A2.1 and A2.2) in f64 and rounded once to f32, and a
//! skeleton posed by a binding. Decoding is [`crate::spline`]'s (a parse module); this works on its typed values.

use crate::animation::{Binding, QsTransform, Skeleton};
use crate::spline::{Absent, Component, Fallback, Knots, Rotation, SplineAnimation, Vector};
use crate::{Result, invalid};

impl SplineAnimation {
    /// The block and the frame within it that `time` seconds is: frames count from 0 at `frameDuration` apart, a block
    /// covering `maxFramesPerBlock - 1` of them; a time outside the animation is its nearest end.
    pub fn locate(&self, time: f32) -> (usize, f32) {
        let last = (self.num_frames - 1) as f64;
        let frame = (f64::from(time) / f64::from(self.frame_duration)).clamp(0.0, last);
        let per_block = f64::from(self.max_frames_per_block - 1);
        let block = ((frame / per_block).floor() as usize).min(self.blocks.len() - 1);
        (block, (frame - block as f64 * per_block) as f32)
    }

    /// A track at `time` seconds; what the track does not state is `absent`'s fallback: `reference` (the bone's
    /// reference transform) or the neutral value.
    pub fn sample(
        &self,
        track: usize,
        time: f32,
        reference: &QsTransform,
        absent: Absent,
    ) -> Result<QsTransform> {
        let (block, frame) = self.locate(time);
        let t = self
            .blocks
            .get(block)
            .and_then(|b| b.get(track))
            .ok_or_else(|| invalid(0, format!("no track {track}")))?;
        let position = sample_vector(
            &t.position,
            frame,
            match absent.position {
                Fallback::Reference => reference.translation,
                Fallback::Neutral => [0.0; 4],
            },
        )?;
        let scale = sample_vector(
            &t.scale,
            frame,
            match absent.scale {
                Fallback::Reference => reference.scale,
                Fallback::Neutral => [1.0, 1.0, 1.0, 0.0],
            },
        )?;
        let rotation = match &t.rotation {
            Rotation::Absent => match absent.rotation {
                Fallback::Reference => reference.rotation,
                Fallback::Neutral => [0.0, 0.0, 0.0, 1.0],
            },
            Rotation::Static(q) => *q,
            Rotation::Spline { knots, points } => {
                let at = evaluate(knots, frame, points.len())?;
                let mut q = [0.0f64; 4];
                for (p, w) in &at {
                    let c = points
                        .get(*p)
                        .ok_or_else(|| invalid(0, "a control point is missing"))?;
                    for (sum, component) in q.iter_mut().zip(c) {
                        *sum += f64::from(*component) * w;
                    }
                }
                let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
                if norm == 0.0 || !norm.is_finite() {
                    return Err(invalid(0, "a rotation spline blends to no rotation"));
                }
                [
                    (q[0] / norm) as f32,
                    (q[1] / norm) as f32,
                    (q[2] / norm) as f32,
                    (q[3] / norm) as f32,
                ]
            }
        };
        Ok(QsTransform {
            translation: position,
            rotation,
            scale,
        })
    }

    /// Every bone of `skeleton` at `time` seconds: the bones a track of `binding` drives sampled (a track bound to a
    /// negative bone index drives none), the others at their reference transform. `binding` must be the one that binds this animation to `skeleton`.
    pub fn pose_at(
        &self,
        time: f32,
        binding: &Binding,
        skeleton: &Skeleton,
        absent: Absent,
    ) -> Result<Vec<QsTransform>> {
        let tracks = self.blocks.first().map_or(0, Vec::len);
        if binding.transform_track_to_bone.len() != tracks {
            return Err(invalid(
                0,
                "the binding has another number of tracks than the animation",
            ));
        }
        let mut pose = skeleton.reference_pose.clone();
        for (track, &bone) in binding.transform_track_to_bone.iter().enumerate() {
            // a negative bone index is a track that drives no bone
            if bone < 0 {
                continue;
            }
            let bone = usize::try_from(bone)
                .ok()
                .filter(|&b| b < pose.len())
                .ok_or_else(|| {
                    invalid(
                        0,
                        format!("track {track} drives bone {bone}, outside the skeleton"),
                    )
                })?;
            pose[bone] = self.sample(track, time, &skeleton.reference_pose[bone], absent)?;
        }
        Ok(pose)
    }
}

/// The control points and weights a B-spline of `knots` over `points` control points has at `frame`
/// (algorithms A2.1 and A2.2 of The NURBS Book): the `degree + 1` points of the knot span the frame lies in. A frame
/// outside the knots is the nearest end.
#[allow(
    clippy::needless_range_loop,
    reason = "the textbook's index algebra, as written"
)]
fn evaluate(knots: &Knots, frame: f32, points: usize) -> Result<Vec<(usize, f64)>> {
    let p = usize::from(knots.degree);
    let k = |i: usize| -> Result<f64> {
        knots
            .values
            .get(i)
            .map(|&v| f64::from(v))
            .ok_or_else(|| invalid(0, "knot index outside the knots"))
    };
    let u = f64::from(frame).clamp(k(p)?, k(points)?);
    // the span: the last i in p..points with knots[i] <= u (the last span holds the end)
    let mut span = p;
    while span + 1 < points && k(span + 1)? <= u {
        span += 1;
    }
    let mut n = vec![0.0f64; p + 1];
    let mut left = vec![0.0f64; p + 1];
    let mut right = vec![0.0f64; p + 1];
    n[0] = 1.0;
    for j in 1..=p {
        left[j] = u - k(span + 1 - j)?;
        right[j] = k(span + j)? - u;
        let mut saved = 0.0;
        for r in 0..j {
            let denominator = right[r + 1] + left[j - r];
            let temp = if denominator == 0.0 {
                0.0
            } else {
                n[r] / denominator
            };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    Ok((0..=p).map(|i| (span - p + i, n[i])).collect())
}

fn sample_component(c: &Component, knots: Option<&Knots>, frame: f32, default: f32) -> Result<f32> {
    Ok(match c {
        Component::Absent => default,
        Component::Static(v) => *v,
        Component::Spline(points) => {
            let knots = knots.ok_or_else(|| invalid(0, "a spline component without knots"))?;
            let mut value = 0.0;
            for (p, w) in evaluate(knots, frame, points.len())? {
                value += f64::from(
                    *points
                        .get(p)
                        .ok_or_else(|| invalid(0, "a control point is missing"))?,
                ) * w;
            }
            value as f32
        }
    })
}

fn sample_vector(v: &Vector, frame: f32, default: [f32; 4]) -> Result<[f32; 4]> {
    Ok([
        sample_component(&v.x, v.knots.as_ref(), frame, default[0])?,
        sample_component(&v.y, v.knots.as_ref(), frame, default[1])?,
        sample_component(&v.z, v.knots.as_ref(), frame, default[2])?,
        default[3],
    ])
}
