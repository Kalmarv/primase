//! The passes that reshape skin weights: `limit-skin-influences` caps the influences a vertex has, and
//! `quantize-skin-weights` stores float weights as normalised bytes or shorts. `normalize-skin-weights` (in
//! [`crate::passes`]) shares their reading, writing and largest-remainder arithmetic.

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::skin::SUM_TOLERANCE;
use primase_core::{
    Component, Entity, Influences, Mesh, Primitive, Semantic, Skin, Stream, WeightRule, World,
};

/// Weights summing to `unit` from shares `w` (any non-negative values with a positive sum), by largest remainder: each
/// share rounded down, the remainder given to the largest fractional parts, ties to the earlier share.
pub(crate) fn quantize(w: &[f64], unit: u64) -> Vec<u64> {
    let sum: f64 = w.iter().sum();
    let exact: Vec<f64> = w.iter().map(|x| x / sum * unit as f64).collect();
    let mut q: Vec<u64> = exact.iter().map(|x| x.floor() as u64).collect();
    let mut left = unit - q.iter().sum::<u64>();
    let mut order: Vec<usize> = (0..w.len()).collect();
    order.sort_by(|&a, &b| {
        let (fa, fb) = (exact[a] - exact[a].floor(), exact[b] - exact[b].floor());
        fb.partial_cmp(&fa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    for &k in &order {
        if left == 0 {
            break;
        }
        q[k] += 1;
        left -= 1;
    }
    q
}

/// Refuses a vertex's weights holding a NaN, an infinity or a negative value (a sum check passes some of them: NaN
/// compares false, and +w with -w can sum to 1).
pub(crate) fn check_weights(ws: &[f64]) -> Result<(), String> {
    match ws.iter().find(|x| !x.is_finite() || **x < 0.0) {
        Some(x) => Err(format!("a weight is {x}: not finite or negative")),
        None => Ok(()),
    }
}

/// The weights of a primitive with `Influences::Streams { sets }`: each vertex's weights over all its sets (four a
/// set, in set order) as floats (normalised integers divided by their unit), and the unit: 0 for `F32x4`, 255 for
/// `U8x4`, 65535 for `U16x4`.
pub(crate) fn read_weights(p: &Primitive, sets: u8) -> Result<(Vec<Vec<f64>>, u64), String> {
    let mut all: Vec<Vec<f64>> = vec![Vec::new(); p.vertex_count()];
    let mut unit = None;
    for k in 0..sets {
        let (values, u): (Vec<[f64; 4]>, u64) = match p.streams.get(&Semantic::Weights(k)) {
            Some(Stream::F32x4(v)) => (v.iter().map(|x| x.map(f64::from)).collect(), 0),
            Some(Stream::U8x4(v)) => (v.iter().map(|x| x.map(|y| f64::from(y) / 255.0)).collect(), 255),
            Some(Stream::U16x4(v)) => (
                v.iter().map(|x| x.map(|y| f64::from(y) / 65535.0)).collect(),
                65535,
            ),
            _ => return Err(format!("Weights({k}) is not a four-value stream")),
        };
        if *unit.get_or_insert(u) != u {
            return Err("weight sets of different types".to_owned());
        }
        for (v, x) in all.iter_mut().zip(values) {
            v.extend(x);
        }
    }
    Ok((all, unit.ok_or("no weight sets")?))
}

/// The inverse of [`read_weights`]: `all` (four values a set, `sets` sets) stored in `unit`'s type under
/// `Weights(0..sets)`.
pub(crate) fn write_weights(
    streams: &mut std::collections::BTreeMap<Semantic, Stream>,
    all: &[Vec<f64>],
    sets: u8,
    unit: u64,
) {
    for k in 0..sets {
        let at = |v: &Vec<f64>, c: usize| v[usize::from(k) * 4 + c];
        let stream = match unit {
            0 => Stream::F32x4(
                all.iter()
                    .map(|v| std::array::from_fn(|c| at(v, c) as f32))
                    .collect(),
            ),
            255 => Stream::U8x4(
                all.iter()
                    .map(|v| std::array::from_fn(|c| (at(v, c) * 255.0).round() as u8))
                    .collect(),
            ),
            _ => Stream::U16x4(
                all.iter()
                    .map(|v| std::array::from_fn(|c| (at(v, c) * 65535.0).round() as u16))
                    .collect(),
            ),
        };
        streams.insert(Semantic::Weights(k), stream);
    }
}

/// The skinned entities that have a mesh, with their skin, for a pass to edit: `(entity, skin, mesh)`.
fn skinned(w: &World) -> Vec<(Entity, Skin, Mesh)> {
    w.iter::<Skin>()
        .filter_map(|(e, s)| w.get::<Mesh>(e).map(|m| (e, s.clone(), m.clone())))
        .collect()
}

/// A primitive's sets, when its influences are per-vertex streams.
fn stream_sets(skin: &Skin, i: usize) -> Option<u8> {
    match skin.influences.get(i).copied() {
        Some(Some(Influences::Streams { sets })) => Some(sets),
        _ => None,
    }
}

/// A primitive's joint slots over all its sets, four a set, in set order.
fn read_joints(p: &Primitive, sets: u8) -> Result<Vec<Vec<u32>>, String> {
    let mut all: Vec<Vec<u32>> = vec![Vec::new(); p.vertex_count()];
    for k in 0..sets {
        let values: Vec<[u32; 4]> = match p.streams.get(&Semantic::Joints(k)) {
            Some(Stream::U8x4(v)) => v.iter().map(|x| x.map(u32::from)).collect(),
            Some(Stream::U16x4(v)) => v.iter().map(|x| x.map(u32::from)).collect(),
            _ => return Err(format!("Joints({k}) is not an integer stream")),
        };
        for (v, x) in all.iter_mut().zip(values) {
            v.extend(x);
        }
    }
    Ok(all)
}

/// `limit-skin-influences{max=N}` (`N` in 1..=255, required): every skinned primitive with per-vertex influences
/// ([`Influences::Streams`]) keeps at most `N` influences a vertex: its `N` largest weights (equal weights: the lower
/// joint slot first, then the earlier influence), the rest dropped and the kept ones rescaled to the vertex's original
/// sum, so a primitive that summed to 1 still does. The joint and weight sets shrink to `ceil(N / 4)` (never grow);
/// influences past the last kept one are zero-weight padding that names the vertex's first kept slot, so indices stay
/// in range. A vertex with `N` or fewer non-zero influences keeps their values exactly. Float weights stay float and
/// normalised byte or short weights stay so, requantised by largest remainder to the vertex's original integer sum.
/// Primitives bound to one slot (`Whole`, `SingleIndex`) are left as they are. Counted: vertices that lost weight, the
/// largest weight dropped from one vertex, and the total dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitSkinInfluences {
    max: u8,
}

impl LimitSkinInfluences {
    /// At most `max` influences a vertex (at least 1).
    pub fn new(max: u8) -> Result<LimitSkinInfluences, String> {
        if max == 0 {
            return Err("max=0: a vertex keeps at least one influence".to_owned());
        }
        Ok(LimitSkinInfluences { max })
    }

    fn from_given(g: &Given) -> Result<LimitSkinInfluences, String> {
        let v = g.required("max");
        let max = v
            .parse::<u8>()
            .map_err(|_| format!("max={v} is not a whole number from 1 to 255"))?;
        LimitSkinInfluences::new(max)
    }
}

/// `limit-skin-influences`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LIMIT_SKIN_INFLUENCES: PassSpec = PassSpec {
    name: "limit-skin-influences",
    options: &[OptionSpec::required(
        "max",
        "4",
        "1 to 255: the most influences a vertex keeps",
    )],
    make: |g| Ok(Box::new(LimitSkinInfluences::from_given(g)?)),
};

impl Pass for LimitSkinInfluences {
    fn name(&self) -> &'static str {
        "limit-skin-influences"
    }
    fn options(&self) -> Options {
        Options::from([("max".to_owned(), self.max.to_string())])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME]
    }
    fn approximates(&self) -> &'static str {
        "skin influences beyond the largest few dropped, the rest rescaled"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let max = usize::from(self.max);
        let mut lost = 0u64;
        let (mut largest, mut total) = (0.0f64, 0.0f64);
        for (e, mut skin, mut mesh) in skinned(w) {
            for (i, mut p) in mesh.primitives_mut().enumerate() {
                let Some(sets) = stream_sets(&skin, i) else {
                    continue;
                };
                let new_sets = sets.min(self.max.div_ceil(4));
                let at = format!("{e:?} primitive {i}");
                let (mut weights, unit) = read_weights(&p, sets).map_err(|m| format!("{at}: {m}"))?;
                let mut joints = read_joints(&p, sets).map_err(|m| format!("{at}: {m}"))?;
                let width = usize::from(new_sets) * 4;
                let limit = max.min(usize::from(sets) * 4);
                for (v, (ws, js)) in weights.iter_mut().zip(&mut joints).enumerate() {
                    if ws.iter().any(|x| !x.is_finite() || *x < 0.0) {
                        return Err(format!("{at}: vertex {v} has a weight not finite or negative"));
                    }
                    let nonzero = ws.iter().filter(|&&x| x > 0.0).count();
                    // the influences kept, in their stored order
                    let kept: Vec<usize> = if nonzero <= max && (limit..ws.len()).all(|k| ws[k] == 0.0) {
                        // every non-zero influence is within the first `max`: stays as stored
                        (0..limit).collect()
                    } else if nonzero <= max {
                        (0..ws.len()).filter(|&k| ws[k] > 0.0).collect()
                    } else {
                        let mut order: Vec<usize> = (0..ws.len()).filter(|&k| ws[k] > 0.0).collect();
                        order.sort_by(|&a, &b| {
                            ws[b].total_cmp(&ws[a]).then(js[a].cmp(&js[b])).then(a.cmp(&b))
                        });
                        order.truncate(max);
                        order.sort_unstable();
                        order
                    };
                    let first = kept.first().map_or(js[0], |&k| js[k]);
                    let mut new_w: Vec<f64> = kept.iter().map(|&k| ws[k]).collect();
                    let mut new_j: Vec<u32> = kept.iter().map(|&k| js[k]).collect();
                    if nonzero > max {
                        let (before, after): (f64, f64) = (ws.iter().sum(), new_w.iter().sum());
                        let dropped = before - after;
                        lost += 1;
                        largest = largest.max(dropped);
                        total += dropped;
                        new_w = if unit == 0 {
                            new_w.iter().map(|x| x / after * before).collect()
                        } else {
                            let whole = (before * unit as f64).round() as u64;
                            quantize(&new_w, whole)
                                .iter()
                                .map(|&q| q as f64 / unit as f64)
                                .collect()
                        };
                    }
                    new_w.resize(width, 0.0);
                    new_j.resize(width, first);
                    *ws = new_w;
                    *js = new_j;
                }
                let streams = p.streams_mut();
                for k in new_sets..sets {
                    streams.remove(&Semantic::Joints(k));
                    streams.remove(&Semantic::Weights(k));
                }
                write_weights(streams, &weights, new_sets, unit);
                for k in 0..new_sets {
                    let at = |v: &Vec<u32>, c: usize| v[usize::from(k) * 4 + c];
                    let wide = matches!(streams.get(&Semantic::Joints(k)), Some(Stream::U16x4(_)));
                    let stream = if wide {
                        Stream::U16x4(
                            joints
                                .iter()
                                .map(|v| std::array::from_fn(|c| at(v, c) as u16))
                                .collect(),
                        )
                    } else {
                        Stream::U8x4(
                            joints
                                .iter()
                                .map(|v| std::array::from_fn(|c| at(v, c) as u8))
                                .collect(),
                        )
                    };
                    streams.insert(Semantic::Joints(k), stream);
                }
                skin.influences[i] = Some(Influences::Streams { sets: new_sets });
            }
            w.insert(e, mesh);
            w.insert(e, skin);
        }
        log.approximate("skinned vertices that lost influences", lost);
        log.count(
            "largest weight dropped from one vertex, in millionths",
            (largest * 1e6).round() as u64,
        );
        log.count(
            "total weight dropped, in millionths",
            (total * 1e6).round() as u64,
        );
        Ok(())
    }
}

/// `quantize-skin-weights{bits=8|16}` (required): every skinned primitive's float weights ([`Stream::F32x4`]) stored
/// as normalised bytes (`bits=8`) or shorts (`bits=16`): each vertex's weights over all its sets quantised to sum to
/// exactly 255 (or 65535) by largest remainder. A primitive whose vertex weights do not sum to 1 (within
/// [`SUM_TOLERANCE`]) is refused: normalise first (`normalize-skin-weights`); so is a NaN, infinite or negative weight.
/// Weights already stored in the asked
/// width stay as they are; weights stored in the other width are refused (converting between widths is not done
/// silently). The skin's rule becomes `SumsToOne`. Counted: vertices quantised, and the largest error of one weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuantizeSkinWeights {
    bits: u8,
}

impl QuantizeSkinWeights {
    /// To `bits` bits a weight: 8 or 16.
    pub fn new(bits: u8) -> Result<QuantizeSkinWeights, String> {
        match bits {
            8 | 16 => Ok(QuantizeSkinWeights { bits }),
            _ => Err(format!("bits={bits} is not 8 or 16")),
        }
    }

    fn from_given(g: &Given) -> Result<QuantizeSkinWeights, String> {
        let v = g.required("bits");
        QuantizeSkinWeights::new(v.parse::<u8>().map_err(|_| format!("bits={v} is not 8 or 16"))?)
    }

    fn unit(&self) -> u64 {
        if self.bits == 8 { 255 } else { 65535 }
    }
}

/// `quantize-skin-weights`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const QUANTIZE_SKIN_WEIGHTS: PassSpec = PassSpec {
    name: "quantize-skin-weights",
    options: &[OptionSpec::required(
        "bits",
        "8",
        "8 or 16: the bits a weight is stored in",
    )],
    make: |g| Ok(Box::new(QuantizeSkinWeights::from_given(g)?)),
};

impl Pass for QuantizeSkinWeights {
    fn name(&self) -> &'static str {
        "quantize-skin-weights"
    }
    fn options(&self) -> Options {
        Options::from([("bits".to_owned(), self.bits.to_string())])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME]
    }
    fn approximates(&self) -> &'static str {
        "float skin weights rounded to bytes or shorts"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let unit = self.unit();
        let mut done = 0u64;
        let mut largest = 0.0f64;
        for (e, skin, mut mesh) in skinned(w) {
            for (i, mut p) in mesh.primitives_mut().enumerate() {
                let Some(sets) = stream_sets(&skin, i) else {
                    continue;
                };
                let at = format!("{e:?} primitive {i}");
                let (mut all, stored) = read_weights(&p, sets).map_err(|m| format!("{at}: {m}"))?;
                if stored != 0 && stored != unit {
                    return Err(format!(
                        "{at}: weights are stored in {} bits, not {}; converting between widths is not done",
                        if stored == 255 { 8 } else { 16 },
                        self.bits
                    ));
                }
                for (v, ws) in all.iter().enumerate() {
                    check_weights(ws).map_err(|m| format!("{at}: vertex {v}: {m}"))?;
                }
                if let Some(v) = all
                    .iter()
                    .position(|ws| (ws.iter().sum::<f64>() - 1.0).abs() > f64::from(SUM_TOLERANCE))
                {
                    return Err(format!(
                        "{at}: vertex {v}'s weights sum to {}, not 1; run normalize-skin-weights first",
                        all[v].iter().sum::<f64>()
                    ));
                }
                if stored == unit {
                    continue;
                }
                for ws in &mut all {
                    let new: Vec<f64> = quantize(ws, unit)
                        .iter()
                        .map(|&q| q as f64 / unit as f64)
                        .collect();
                    largest = largest.max(
                        ws.iter()
                            .zip(&new)
                            .map(|(a, b)| (a - b).abs())
                            .fold(0.0, f64::max),
                    );
                    *ws = new;
                    done += 1;
                }
                write_weights(p.streams_mut(), &all, sets, unit);
            }
            w.insert(e, mesh);
            if let Some(s) = w.get_mut::<Skin>(e) {
                s.weights = WeightRule::SumsToOne;
            }
        }
        log.approximate("skinned vertices whose float weights were quantised", done);
        log.count(
            "largest error of one weight, in millionths",
            (largest * 1e6).round() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::register;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{
        Binding, BindingId, Deformation, Joint, JointId, Rest, Skeleton, Slot, Topology, Transform,
    };
    use std::collections::BTreeMap;

    fn manager() -> PassManager {
        let mut r = PassRegistry::new();
        register(&mut r);
        PassManager::new(r).with_pass_checks(true)
    }

    /// One skinned entity of one primitive: `joints[k]` is `Joints(k)` (one row a vertex), `weights` is `Weights(0)`
    /// and `more` the weight sets after it.
    fn world(joints: &[Vec<[u8; 4]>], weights: Stream, more: Vec<Stream>) -> (World, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![Joint {
                    id: JointId(1),
                    name: "root".into(),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                }],
            },
        );
        let e = w.spawn();
        let n = joints[0].len();
        let mut streams = BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]))]);
        for (k, j) in joints.iter().enumerate() {
            streams.insert(Semantic::Joints(k as u8), Stream::U8x4(j.clone()));
        }
        streams.insert(Semantic::Weights(0), weights);
        for (k, s) in more.into_iter().enumerate() {
            streams.insert(Semantic::Weights(k as u8 + 1), s);
        }
        w.insert(
            e,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams,
                indices: None,
                material: None,
            }]),
        );
        w.insert(
            e,
            Skin {
                influences: vec![Some(Influences::Streams {
                    sets: joints.len() as u8,
                })],
                weights: WeightRule::AsStored,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton,
                    palettes: vec![Some(vec![
                        Slot {
                            joint: JointId(1),
                            inverse_bind: Transform::IDENTITY,
                        };
                        10
                    ])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        (w, e)
    }

    fn run(w: &mut World, pipeline: &str) -> Result<RunReport, String> {
        let m = manager();
        let mut report = RunReport::default();
        m.run(
            w,
            &m.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn streams(w: &World, e: Entity) -> BTreeMap<Semantic, Stream> {
        w.get::<Mesh>(e).unwrap().primitives()[0].streams.clone()
    }

    fn floats(s: &Stream) -> Vec<[f32; 4]> {
        let Stream::F32x4(v) = s else { panic!("floats") };
        v.clone()
    }

    fn bytes(s: &Stream) -> Vec<[u8; 4]> {
        let Stream::U8x4(v) = s else { panic!("bytes") };
        v.clone()
    }

    /// Two sets of floats. Vertex 0: six influences (slots 1 to 6: 0.30, 0.05, 0.25, 0.10, 0.20, 0.10). Vertex 1: three
    /// (slots 1, 2, 3: 0.5, 0.25, 0.25). Vertex 2: five of 0.2 each (slots 7, 8, 9, 1, 2).
    fn six_and_three() -> (World, Entity) {
        world(
            &[
                vec![[1, 2, 3, 4], [1, 2, 3, 3], [7, 8, 9, 1]],
                vec![[5, 6, 5, 5], [3, 3, 3, 3], [2, 2, 2, 2]],
            ],
            Stream::F32x4(vec![
                [0.30, 0.05, 0.25, 0.10],
                [0.5, 0.25, 0.25, 0.0],
                [0.2, 0.2, 0.2, 0.2],
            ]),
            vec![Stream::F32x4(vec![
                [0.20, 0.10, 0.0, 0.0],
                [0.0; 4],
                [0.2, 0.0, 0.0, 0.0],
            ])],
        )
    }

    /// Hand-worked, max=4. Vertex 0 keeps 0.30, 0.25, 0.20 and one of its two 0.10s, the one on the lower slot (4, not
    /// 6): joints 1, 3, 4, 5 in stored order, dropping 0.05 + 0.10 and rescaling by 1 / 0.85. Vertex 1 has three
    /// influences and stays bit for bit. Vertex 2's five equal weights keep the four lowest slots (1, 2, 7, 8, stored
    /// order 7, 8, 1, 2), each 0.25. The two sets become one.
    #[test]
    fn limit_keeps_the_largest_and_rescales() {
        let (mut w, e) = six_and_three();
        let report = run(&mut w, "limit-skin-influences{max=4}").unwrap();
        let s = streams(&w, e);
        assert!(!s.contains_key(&Semantic::Weights(1)) && !s.contains_key(&Semantic::Joints(1)));
        assert_eq!(
            w.get::<Skin>(e).unwrap().influences,
            [Some(Influences::Streams { sets: 1 })]
        );
        assert_eq!(
            bytes(&s[&Semantic::Joints(0)]),
            [[1, 3, 4, 5], [1, 2, 3, 3], [7, 8, 1, 2]]
        );
        let wt = floats(&s[&Semantic::Weights(0)]);
        for (got, want) in wt[0].iter().zip([0.30, 0.25, 0.10, 0.20]) {
            assert!((f64::from(*got) - want / 0.85).abs() < 1e-6, "{wt:?}");
        }
        assert!((wt[0].iter().map(|&x| f64::from(x)).sum::<f64>() - 1.0).abs() < 1e-6);
        assert_eq!(wt[1], [0.5, 0.25, 0.25, 0.0]);
        assert!(wt[2].iter().all(|&x| (x - 0.25).abs() < 1e-6), "{wt:?}");
        assert_eq!(
            report.steps[0].log.approximations["skinned vertices that lost influences"],
            2
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    /// max=8 keeps both sets and every stored value; max=6 keeps two sets and, with no vertex over six influences,
    /// changes only the padding past the sixth influence (to the vertex's first slot).
    #[test]
    fn limit_set_counts_follow_max() {
        let (mut w, e) = six_and_three();
        let before = streams(&w, e);
        let report = run(&mut w, "limit-skin-influences{max=8}").unwrap();
        assert_eq!(streams(&w, e), before);
        assert_eq!(
            report.steps[0].log.approximations["skinned vertices that lost influences"],
            0
        );
        let (mut w, e) = six_and_three();
        run(&mut w, "limit-skin-influences{max=6}").unwrap();
        let s = streams(&w, e);
        assert_eq!(
            w.get::<Skin>(e).unwrap().influences,
            [Some(Influences::Streams { sets: 2 })]
        );
        assert_eq!(floats(&s[&Semantic::Weights(1)])[0], [0.20, 0.10, 0.0, 0.0]);
        assert_eq!(
            bytes(&s[&Semantic::Joints(1)]),
            [[5, 6, 1, 1], [3, 3, 1, 1], [2, 2, 7, 7]]
        );
    }

    /// Vertex 0 drops 0.15 and vertex 2 drops 0.2 (vertex 1 nothing): the largest from one vertex is 0.2, the total
    /// 0.35.
    #[test]
    fn limit_reports_what_it_dropped() {
        let (mut w, _) = six_and_three();
        let report = run(&mut w, "limit-skin-influences{max=4}").unwrap();
        let log = &report.steps[0].log;
        assert_eq!(log.approximations["skinned vertices that lost influences"], 2);
        assert_eq!(
            log.counts["largest weight dropped from one vertex, in millionths"],
            200_000
        );
        assert_eq!(log.counts["total weight dropped, in millionths"], 350_000);
    }

    /// Bytes stay bytes and the vertex still sums to exactly 255. 90 + 60 + 50 + 30 + 20 + 5 limited to three keeps 90,
    /// 60, 50 (200): exact shares 114.75, 76.5, 63.75 of 255, floors 253, the two left to the fractions .75 and .75.
    #[test]
    fn limit_keeps_bytes_exact() {
        let (mut w, e) = world(
            &[vec![[1, 2, 3, 4]], vec![[5, 6, 6, 6]]],
            Stream::U8x4(vec![[90, 60, 50, 30]]),
            vec![Stream::U8x4(vec![[20, 5, 0, 0]])],
        );
        run(&mut w, "limit-skin-influences{max=3}").unwrap();
        let s = streams(&w, e);
        assert_eq!(bytes(&s[&Semantic::Weights(0)]), [[115, 76, 64, 0]]);
        assert_eq!(bytes(&s[&Semantic::Joints(0)]), [[1, 2, 3, 1]]);
    }

    /// `max` is required, a whole number from 1 to 255, and the only option.
    #[test]
    fn limit_option_errors() {
        let m = manager();
        for bad in [
            "limit-skin-influences",
            "limit-skin-influences{max=0}",
            "limit-skin-influences{max=x}",
            "limit-skin-influences{max=256}",
            "limit-skin-influences{max=4,n=1}",
        ] {
            assert!(m.registry.parse(bad).is_err(), "{bad}");
        }
        assert_eq!(LimitSkinInfluences::new(255).unwrap().options()["max"], "255");
    }

    fn normalised_floats() -> (World, Entity) {
        world(
            &[vec![[1, 2, 3, 4], [1, 2, 3, 4]]],
            Stream::F32x4(vec![
                [0.5, 0.25, 0.25, 0.0],
                [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0],
            ]),
            vec![],
        )
    }

    /// Hand-worked: 0.5, 0.25, 0.25 of 255 are 127.5, 63.75, 63.75: floors 253, the two left to the fractions .75 and
    /// .75: 127, 64, 64 (error 0.5 - 127/255 = 1961 millionths). A third is 85 of 255 each. Of 65535: 32767.5,
    /// 16383.75, 16383.75 give 32767, 16384, 16384.
    #[test]
    fn quantize_sums_exactly() {
        let (mut w, e) = normalised_floats();
        let report = run(&mut w, "quantize-skin-weights{bits=8}").unwrap();
        assert_eq!(
            bytes(&streams(&w, e)[&Semantic::Weights(0)]),
            [[127, 64, 64, 0], [85, 85, 85, 0]]
        );
        assert_eq!(w.get::<Skin>(e).unwrap().weights, WeightRule::SumsToOne);
        let log = &report.steps[0].log;
        assert_eq!(
            log.approximations["skinned vertices whose float weights were quantised"],
            2
        );
        assert_eq!(log.counts["largest error of one weight, in millionths"], 1961);
        let (mut w, e) = normalised_floats();
        run(&mut w, "quantize-skin-weights{bits=16}").unwrap();
        let Stream::U16x4(v) = &streams(&w, e)[&Semantic::Weights(0)] else {
            panic!("shorts")
        };
        assert_eq!(v[0], [32767, 16384, 16384, 0]);
        assert!(
            v.iter()
                .all(|r| r.iter().map(|&x| u32::from(x)).sum::<u32>() == 65535)
        );
    }

    /// Weights already stored in the asked width are unchanged and not counted.
    #[test]
    fn quantize_leaves_exact_weights() {
        let (mut w, e) = world(&[vec![[1, 2, 3, 4]]], Stream::U8x4(vec![[200, 55, 0, 0]]), vec![]);
        let report = run(&mut w, "quantize-skin-weights{bits=8}").unwrap();
        assert_eq!(bytes(&streams(&w, e)[&Semantic::Weights(0)]), [[200, 55, 0, 0]]);
        assert_eq!(
            report.steps[0].log.approximations["skinned vertices whose float weights were quantised"],
            0
        );
        assert_eq!(w.get::<Skin>(e).unwrap().weights, WeightRule::SumsToOne);
    }

    /// A NaN weight (its sum compares false against the tolerance) and a negative one (1.5 + -0.5 sums to 1) are
    /// refused by name by both passes, not stored as quantised garbage.
    #[test]
    fn nan_and_negative_weights_are_refused() {
        for (weights, what) in [
            ([f32::NAN, 0.5, 0.5, 0.0], "NaN"),
            ([1.5, -0.5, 0.0, 0.0], "-0.5"),
            ([f32::INFINITY, 0.0, 0.0, 0.0], "inf"),
        ] {
            for pass in ["quantize-skin-weights{bits=8}", "normalize-skin-weights"] {
                let (mut w, _) = world(&[vec![[1, 2, 3, 4]]], Stream::F32x4(vec![weights]), vec![]);
                let err = run(&mut w, pass).unwrap_err();
                assert!(
                    err.contains("vertex 0") && err.contains(what) && err.contains("not finite or negative"),
                    "{pass}: {err}"
                );
            }
        }
    }

    /// Weights not summing to 1 are refused with the way out; the other width is refused; options are checked.
    #[test]
    fn quantize_refusals() {
        let (mut w, _) = world(
            &[vec![[1, 2, 3, 4]]],
            Stream::F32x4(vec![[0.5, 0.25, 0.0, 0.0]]),
            vec![],
        );
        let err = run(&mut w, "quantize-skin-weights{bits=8}").unwrap_err();
        assert!(
            err.contains("vertex 0") && err.contains("normalize-skin-weights"),
            "{err}"
        );
        let (mut w, _) = world(&[vec![[1, 2, 3, 4]]], Stream::U8x4(vec![[200, 55, 0, 0]]), vec![]);
        let err = run(&mut w, "quantize-skin-weights{bits=16}").unwrap_err();
        assert!(err.contains("8 bits, not 16"), "{err}");
        let m = manager();
        for bad in [
            "quantize-skin-weights",
            "quantize-skin-weights{bits=12}",
            "quantize-skin-weights{bits=x}",
            "quantize-skin-weights{bits=8,x=1}",
        ] {
            assert!(m.registry.parse(bad).is_err(), "{bad}");
        }
    }
}
