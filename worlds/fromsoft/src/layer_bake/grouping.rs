//! Named static approximation: at most three area-weighted layer levels,
//! quantized to 1/64. This approximates a continuously varying shader input;
//! it is not a property of FLVER or the game's material system.
use crate::{Result, invalid};
use primase_core::{Primitive, Topology};

pub type Features = [f32; 8];
#[derive(Clone, Debug)]
pub struct Groups {
    pub centers: Vec<Features>,
    pub triangles: Vec<usize>,
}
fn distance(a: &Features, b: &Features) -> f64 {
    a.iter()
        .zip(b)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum()
}
fn nearest(centers: &[Features], f: &Features) -> usize {
    centers
        .iter()
        .enumerate()
        .min_by(|(ia, a), (ib, b)| distance(a, f).total_cmp(&distance(b, f)).then(ia.cmp(ib)))
        .expect("nonempty centers")
        .0
}
fn mean(features: &[Features], weights: &[f64], members: impl Iterator<Item = usize>) -> Features {
    let mut sum = [0.; 8];
    let mut weight = 0.;
    for i in members {
        weight += weights[i];
        for (k, s) in sum.iter_mut().enumerate() {
            *s += f64::from(features[i][k]) * weights[i];
        }
    }
    sum.map(|v| if weight > 0. { (v / weight) as f32 } else { 0. })
}
/// Triangle weights are area times placement count. Zero-area triangles retain
/// an assignment but do not contribute to centroids. Features are normalized
/// packed values in [0, 1]. All sums use f64.
pub fn cluster(features: &[Features], weights: &[f64]) -> Result<Groups> {
    if features.is_empty()
        || features.len() != weights.len()
        || !features.iter().flatten().all(|x| (0. ..=1.).contains(x))
        || !weights.iter().all(|x| x.is_finite() && *x >= 0.)
    {
        return Err(invalid(
            "layer groups",
            0,
            "empty, mismatched or out-of-domain features/weights",
        ));
    }
    let total: f64 = weights.iter().sum();
    if !total.is_finite() {
        return Err(invalid("layer groups", 0, "weight sum overflow"));
    }
    let mut centers = vec![mean(features, weights, 0..features.len())];
    while centers.len() < 3 {
        let mut far = None;
        let mut best = 0.;
        for (i, f) in features.iter().enumerate() {
            let d = distance(&centers[nearest(&centers, f)], f);
            if d > best + 1e-9 {
                best = d;
                far = Some(i);
            }
        }
        if best < 0.08 * 0.08 {
            break;
        }
        let Some(i) = far else {
            break;
        };
        centers.push(features[i]);
    }
    let mut assignment = vec![usize::MAX; features.len()];
    for iteration in 0..30 {
        let next: Vec<_> = features.iter().map(|f| nearest(&centers, f)).collect();
        let changed = next != assignment;
        assignment = next;
        for (c, center) in centers.iter_mut().enumerate() {
            if assignment.contains(&c) {
                *center = mean(
                    features,
                    weights,
                    (0..features.len()).filter(|&i| assignment[i] == c),
                );
            }
        }
        if !changed && iteration > 0 {
            break;
        }
    }
    while centers.len() > 1 {
        let mut shares = vec![0.; centers.len()];
        for (i, f) in features.iter().enumerate() {
            shares[nearest(&centers, f)] += if total > 0. { weights[i] / total } else { 0. };
        }
        let mut small = (0..centers.len())
            .min_by(|&a, &b| shares[a].total_cmp(&shares[b]).then(a.cmp(&b)))
            .expect("centers");
        let mut close = false;
        'pairs: for a in 0..centers.len() {
            for b in a + 1..centers.len() {
                if centers[a]
                    .iter()
                    .zip(centers[b])
                    .all(|(&x, y)| (x - y).abs() < 0.08)
                {
                    small = if shares[a] < shares[b] { a } else { b };
                    close = true;
                    break 'pairs;
                }
            }
        }
        if shares[small] >= 0.02 && !close {
            break;
        }
        centers.remove(small);
        let assignment: Vec<_> = features.iter().map(|f| nearest(&centers, f)).collect();
        for (c, center) in centers.iter_mut().enumerate() {
            if assignment.contains(&c) {
                *center = mean(
                    features,
                    weights,
                    (0..features.len()).filter(|&i| assignment[i] == c),
                );
            }
        }
    }
    for c in &mut centers {
        for x in c {
            *x = (*x * 64.).round_ties_even() / 64.;
        }
    }
    centers.sort_by(|a, b| {
        a[..4]
            .iter()
            .sum::<f32>()
            .total_cmp(&b[..4].iter().sum::<f32>())
            .then_with(|| {
                a.iter()
                    .zip(b)
                    .map(|(a, b)| a.total_cmp(b))
                    .find(|o| !o.is_eq())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let triangles = features.iter().map(|f| nearest(&centers, f)).collect();
    Ok(Groups { centers, triangles })
}

/// Split indexed triangles by their assigned level, preserving every vertex
/// stream through the shared compactor. A level with no triangles is omitted.
pub fn split(primitive: &Primitive, assignments: &[usize], levels: usize) -> Result<Vec<(usize, Primitive)>> {
    if primitive.topology != Topology::Triangles {
        return Err(invalid("layer split", 0, "triangle topology required"));
    }
    let indices = primitive
        .indices
        .as_ref()
        .ok_or_else(|| invalid("layer split", 0, "indexed geometry required"))?;
    if !indices.len().is_multiple_of(3)
        || assignments.len() != indices.len() / 3
        || assignments.iter().any(|&x| x >= levels)
    {
        return Err(invalid("layer split", 0, "invalid triangle assignments"));
    }
    let mut result = Vec::new();
    for level in 0..levels {
        let selected: Vec<_> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .zip(assignments)
            .filter(|(_, a)| **a == level)
            .flat_map(|(tri, _)| tri.iter().copied())
            .collect();
        if selected.is_empty() {
            continue;
        }
        let mut part = primitive.clone();
        part.indices = Some(selected);
        let compacted =
            primase_meshops::compact_primitive(&part).map_err(|e| invalid("layer split", 0, e))?;
        result.push((level, compacted.primitive));
    }
    Ok(result)
}
