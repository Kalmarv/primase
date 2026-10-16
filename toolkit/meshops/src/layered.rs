//! Triangle selection with a caller-chosen material weight policy.
use crate::{Error, Result};
use primase_core::{LayeredMaterial, Primitive, Semantic, Stream, Topology, WeightMapping};

/// Which weights vote for the strongest triangle layer; neither policy normalizes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionPolicy {
    /// Sum the exact core material equation at the triangle's corners.
    CoreWeights,
    /// Ignore (1-r) only while choosing an RgbProduct4 slot. This is a lossy target policy.
    OmitRgbProduct4Darkening,
}

/// Split a triangle primitive into nonempty slots in slot order, preserving the
/// source vertex order and all streams. A null slot still wins normally; the
/// caller supplies its black material. Missing/invalid weight streams fail.
/// Each selected slot keeps a full copy of the vertex streams (up to four
/// copies); a consumer may compact afterward. If source normals are absent,
/// normals use only the selected slot's faces, so boundaries can show seams.
pub fn strongest_layer_parts(
    p: &Primitive,
    material: &LayeredMaterial,
    policy: SelectionPolicy,
) -> Result<Vec<(usize, Primitive)>> {
    if p.topology != Topology::Triangles || !(1..=4).contains(&material.layers.len()) {
        return Err(Error("strongest layer requires triangles and 1-4 slots".into()));
    }
    if material.weights.mapping == WeightMapping::RgbProduct4 && material.layers.len() != 4 {
        return Err(Error("rgb_product_4 requires four slots".into()));
    }
    let count = p
        .positions()
        .ok_or_else(|| Error("missing positions".into()))?
        .len();
    let stream = p
        .streams
        .get(&material.weights.attribute)
        .ok_or_else(|| Error("missing layered weight stream".into()))?;
    let values: Vec<[f32; 4]> = match stream {
        Stream::F32x3(v) => v.iter().map(|&[r, g, b]| [r, g, b, 1.]).collect(),
        Stream::F32x4(v) => v.clone(),
        Stream::U8x4(v) => v.iter().map(|v| v.map(|x| f32::from(x) / 255.)).collect(),
        Stream::U16x4(v) => v.iter().map(|v| v.map(|x| f32::from(x) / 65535.)).collect(),
        _ => {
            return Err(Error(
                "layer weights require VEC3/VEC4 float or normalized unsigned channels".into(),
            ));
        }
    };
    if values.len() != count || !values.iter().flatten().all(|v| v.is_finite()) {
        return Err(Error("invalid layered weight count or nonfinite value".into()));
    }
    let indices = p.indices.clone().unwrap_or_else(|| (0..count as u32).collect());
    if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= count) {
        return Err(Error("invalid layered triangle indices".into()));
    }
    let mut buckets = vec![Vec::new(); material.layers.len()];
    for tri in indices.as_chunks::<3>().0 {
        let mut sums = [0f64; 4];
        for &i in tri {
            let mut v = values[i as usize];
            if policy == SelectionPolicy::OmitRgbProduct4Darkening
                && material.weights.mapping == WeightMapping::RgbProduct4
            {
                v[0] = 0.;
            }
            for (sum, weight) in sums.iter_mut().zip(material.weights.mapping.evaluate(v)) {
                *sum += f64::from(weight);
            }
        }
        let mut best = 0;
        for i in 1..buckets.len() {
            if sums[i] > sums[best] {
                best = i;
            }
        }
        buckets[best].extend_from_slice(tri);
    }
    let mut result = Vec::new();
    for (slot, indices) in buckets.into_iter().enumerate() {
        if indices.is_empty() {
            continue;
        }
        let mut part = p.clone();
        part.indices = Some(indices);
        part.material = material.layers[slot].as_ref().map(|l| l.material);
        if !part.streams.contains_key(&Semantic::Normal) {
            let positions = part.positions().expect("checked positions");
            let mut normals = vec![[0.; 3]; count];
            for tri in part.indices.as_ref().expect("indices").as_chunks::<3>().0 {
                let [a, b, c] = tri.map(|i| i as usize);
                let n = crate::cross(
                    crate::sub(positions[b], positions[a]),
                    crate::sub(positions[c], positions[a]),
                );
                for i in [a, b, c] {
                    normals[i] = crate::add(normals[i], n);
                }
            }
            for n in &mut normals {
                *n = crate::vector::Vec3::from(*n)
                    .normalize_or([0., 1., 0.].into())
                    .into();
            }
            part.streams.insert(Semantic::Normal, Stream::F32x3(normals));
        }
        result.push((slot, part));
    }
    Ok(result)
}
