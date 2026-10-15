use crate::{Result, Value, invalid};

/// Stored polytope core. Radius expansion is a tessellation policy, not the
/// rounded surface of a Havok convex shape (notably at capsule corners).
#[derive(Debug)]
pub struct ConvexCore {
    pub vertices: Vec<[f32; 3]>,
    pub planes: Vec<[f32; 4]>,
    pub faces: Vec<Vec<usize>>,
    pub radius: f32,
}
impl ConvexCore {
    pub fn read(shape: Value<'_, '_>) -> Result<Self> {
        let radius = shape.field("convexRadius")?.float()?;
        let stored = shape.field("vertices")?.array()?;
        let mut vertices = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let v = stored.get(i)?.vector::<4>()?;
            vertices.push([v[0], v[1], v[2]]);
        }
        let stored = shape.field("planes")?.array()?;
        let mut planes = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            planes.push(stored.get(i)?.vector::<4>()?);
        }
        let indices = shape.field("indices")?.array()?;
        let stored = shape.field("faces")?.array()?;
        let mut faces = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let face = stored.get(i)?;
            let start = usize::try_from(face.field("firstIndex")?.unsigned()?)
                .map_err(|_| invalid(face.offset, "face index overflow"))?;
            let count = usize::try_from(face.field("numIndices")?.unsigned()?)
                .map_err(|_| invalid(face.offset, "face size overflow"))?;
            let end = start
                .checked_add(count)
                .filter(|&e| e <= indices.len())
                .ok_or_else(|| invalid(face.offset, "face exceeds index array"))?;
            let mut polygon = Vec::with_capacity(count);
            for k in start..end {
                polygon.push(
                    usize::try_from(indices.get(k)?.unsigned()?)
                        .map_err(|_| invalid(face.offset, "vertex index overflow"))?,
                );
            }
            faces.push(polygon);
        }
        let core = Self {
            vertices,
            planes,
            faces,
            radius,
        };
        core.validate()?;
        Ok(core)
    }
    fn validate(&self) -> Result<()> {
        if !self.radius.is_finite()
            || self.radius < 0.
            || self
                .vertices
                .iter()
                .flatten()
                .chain(self.planes.iter().flatten())
                .any(|x| !x.is_finite())
        {
            return Err(invalid(0, "invalid convex geometry"));
        }
        if self.faces.len() != self.planes.len() {
            return Err(invalid(0, "convex face/plane counts differ"));
        }
        for face in &self.faces {
            if face.len() < 3 || face.iter().any(|&i| i >= self.vertices.len()) {
                return Err(invalid(0, "invalid convex face"));
            }
        }
        Ok(())
    }
    /// Tessellate the intersection of outward-offset half-spaces in f64.
    /// This is a sharp polytope approximation to the rounded Havok shape; a
    /// capsule core becomes a box. Extra corners are retained where an original
    /// vertex's incident offset planes no longer meet at a single point.
    pub fn expanded_triangles(&self) -> Result<Vec<[[f32; 3]; 3]>> {
        self.validate()?;
        let mut planes = Vec::<[f64; 4]>::new();
        for plane in &self.planes {
            let p = plane.map(f64::from);
            let length = dot([p[0], p[1], p[2]], [p[0], p[1], p[2]]).sqrt();
            if length == 0. {
                return Err(invalid(0, "zero convex plane normal"));
            }
            let normalized = [
                p[0] / length,
                p[1] / length,
                p[2] / length,
                (f64::from(self.radius) - p[3]) / length,
            ];
            if !planes
                .iter()
                .any(|q| q.iter().zip(normalized).all(|(a, b)| (a - b).abs() <= 1e-12))
            {
                planes.push(normalized);
            }
        }
        if planes.len() < 4 {
            return Err(invalid(0, "unbounded convex half-spaces"));
        }
        let scale = planes.iter().map(|p| p[3].abs()).fold(1., f64::max);
        let epsilon = 1e-10 * scale;
        // Weld sub-nanometre corners before f32 emission; the independent
        // 0.01 mm containment/incidence check still applies to every output.
        let weld = 1e-9;
        let mut vertices = Vec::<[f64; 3]>::new();
        for i in 0..planes.len() {
            for j in i + 1..planes.len() {
                for k in j + 1..planes.len() {
                    let Some(v) = solve([planes[i], planes[j], planes[k]]) else {
                        continue;
                    };
                    if !v.iter().all(|x| x.is_finite()) || planes.iter().any(|p| distance(*p, v) > epsilon) {
                        continue;
                    }
                    let existing = vertices
                        .iter()
                        .position(|q| (0..3).all(|c| (v[c] - q[c]).abs() <= weld));
                    if existing.is_none() {
                        vertices.push(v);
                    }
                }
            }
        }
        if vertices.len() < 4 {
            return Err(invalid(0, "empty or degenerate convex intersection"));
        }
        let faces = hull(&vertices, 1e-8 * scale)?;
        let mut triangles = Vec::with_capacity(faces.len());
        for face in faces {
            let output = face.map(|i| vertices[i].map(|x| x as f32));
            for p in output {
                let p = p.map(f64::from);
                if p.iter().any(|x| !x.is_finite())
                    || planes.iter().any(|q| distance(*q, p) > 1e-5)
                    || planes.iter().filter(|q| distance(**q, p).abs() <= 1e-5).count() < 3
                {
                    return Err(invalid(0, "convex f32 vertex exceeds 0.01 mm plane bound"));
                }
            }
            if !planes.iter().any(|q| {
                output
                    .iter()
                    .all(|p| distance(*q, p.map(f64::from)).abs() <= 1e-5)
            }) {
                return Err(invalid(0, "convex hull has an unsupported closing face"));
            }
            let rounded = output.map(|p| p.map(f64::from));
            let area = cross(sub(rounded[1], rounded[0]), sub(rounded[2], rounded[0]));
            // Quantization may collapse sub-ULP slivers; do not emit zero-area
            // collision triangles after converting to the consumer precision.
            if dot(area, area) > 0. {
                triangles.push(output);
            }
        }
        Ok(triangles)
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn distance(p: [f64; 4], v: [f64; 3]) -> f64 {
    p[0] * v[0] + p[1] * v[1] + p[2] * v[2] - p[3]
}

/// Incremental hull of the accepted intersections. The seed centroid stays
/// strictly inside the final hull and gives every new face its outward winding.
fn hull(points: &[[f64; 3]], epsilon: f64) -> Result<Vec<[usize; 3]>> {
    let a = 0;
    let b = (1..points.len())
        .max_by(|&i, &j| {
            dot(sub(points[i], points[a]), sub(points[i], points[a]))
                .total_cmp(&dot(sub(points[j], points[a]), sub(points[j], points[a])))
        })
        .expect("four points");
    let ab = sub(points[b], points[a]);
    let c = (0..points.len())
        .max_by(|&i, &j| {
            let u = cross(ab, sub(points[i], points[a]));
            let v = cross(ab, sub(points[j], points[a]));
            dot(u, u).total_cmp(&dot(v, v))
        })
        .expect("four points");
    let normal = cross(ab, sub(points[c], points[a]));
    let d = (0..points.len())
        .max_by(|&i, &j| {
            dot(normal, sub(points[i], points[a]))
                .abs()
                .total_cmp(&dot(normal, sub(points[j], points[a])).abs())
        })
        .expect("four points");
    if dot(normal, sub(points[d], points[a])).abs() <= epsilon * dot(normal, normal).sqrt() {
        return Err(invalid(0, "degenerate convex hull"));
    }
    let center = std::array::from_fn(|k| (points[a][k] + points[b][k] + points[c][k] + points[d][k]) * 0.25);
    let orient = |mut f: [usize; 3]| {
        if side(points[f[0]], points[f[1]], points[f[2]], center) > 0. {
            f.swap(1, 2);
        }
        f
    };
    let mut faces = vec![
        orient([a, b, c]),
        orient([a, d, b]),
        orient([a, c, d]),
        orient([b, d, c]),
    ];
    for i in 0..points.len() {
        if [a, b, c, d].contains(&i) {
            continue;
        }
        let mut retained = Vec::new();
        let mut horizon = std::collections::BTreeMap::<(usize, usize), usize>::new();
        for f in faces {
            if side(points[f[0]], points[f[1]], points[f[2]], points[i]) > 0. {
                for k in 0..3 {
                    let u = f[k];
                    let v = f[(k + 1) % 3];
                    *horizon.entry((u.min(v), u.max(v))).or_default() += 1;
                }
            } else {
                retained.push(f);
            }
        }
        for ((u, v), n) in horizon {
            if n == 1 {
                retained.push(orient([u, v, i]));
            }
        }
        faces = retained;
    }
    let mut edges = std::collections::BTreeMap::new();
    for f in &faces {
        for k in 0..3 {
            let u = f[k];
            let v = f[(k + 1) % 3];
            *edges.entry((u.min(v), u.max(v))).or_insert(0) += 1;
        }
        let n = cross(sub(points[f[1]], points[f[0]]), sub(points[f[2]], points[f[0]]));
        if dot(n, n) == 0.
            || points
                .iter()
                .any(|&p| side(points[f[0]], points[f[1]], points[f[2]], p) > 0.)
        {
            return Err(invalid(
                0,
                format!(
                    "invalid convex hull face {:?}; normal length {}; maximum plane error {}",
                    f,
                    dot(n, n).sqrt(),
                    points
                        .iter()
                        .map(|&p| dot(n, sub(p, points[f[0]])) / dot(n, n).sqrt())
                        .fold(f64::NEG_INFINITY, f64::max)
                ),
            ));
        }
    }
    if edges.values().any(|&n| n != 2) {
        return Err(invalid(0, "convex hull is not closed"));
    }
    Ok(faces)
}

// Adaptive orientation: ordinary f64 away from cancellation, exact expansions
// of the represented inputs for nearly coplanar points. A distance tolerance
// during horizon construction can turn a sliver into a non-convex face.
fn side(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> f64 {
    let x = sub(b, a);
    let y = sub(c, a);
    let z = sub(d, a);
    let terms = [
        x[0] * y[1] * z[2],
        x[1] * y[2] * z[0],
        x[2] * y[0] * z[1],
        -x[2] * y[1] * z[0],
        -x[1] * y[0] * z[2],
        -x[0] * y[2] * z[1],
    ];
    let det = terms.iter().sum::<f64>();
    if det.abs() > terms.iter().map(|v| v.abs()).sum::<f64>() * 32. * f64::EPSILON {
        return det;
    }
    let delta = |p: [f64; 3]| std::array::from_fn::<_, 3, _>(|i| add(&[p[i]], &[-a[i]]));
    let x = delta(b);
    let y = delta(c);
    let z = delta(d);
    let mut result = Vec::new();
    for (i, j, k, sign) in [
        (0, 1, 2, 1.),
        (1, 2, 0, 1.),
        (2, 0, 1, 1.),
        (2, 1, 0, -1.),
        (1, 0, 2, -1.),
        (0, 2, 1, -1.),
    ] {
        let term = multiply(&multiply(&x[i], &y[j]), &z[k]);
        result = add(&result, &term.into_iter().map(|v| v * sign).collect::<Vec<_>>());
    }
    result.last().copied().unwrap_or(0.)
}
fn add(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut result = a.to_vec();
    for &v in b {
        let mut next = Vec::new();
        let mut carry = v;
        for x in result {
            let sum = carry + x;
            let virtual_x = sum - carry;
            let error = (carry - (sum - virtual_x)) + (x - virtual_x);
            if error != 0. {
                next.push(error);
            }
            carry = sum;
        }
        if carry != 0. {
            next.push(carry);
        }
        result = next;
    }
    result
}
fn multiply(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut result = Vec::new();
    for &x in a {
        for &y in b {
            let product = x * y;
            let error = x.mul_add(y, -product);
            result = add(&result, &[error, product]);
        }
    }
    result
}

fn solve(mut a: [[f64; 4]; 3]) -> Option<[f64; 3]> {
    let scale = a.iter().flat_map(|r| &r[..3]).map(|v| v.abs()).fold(0., f64::max);
    for col in 0..3 {
        let pivot = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() <= scale * 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let divisor = a[col][col];
        for value in &mut a[col][col..] {
            *value /= divisor;
        }
        let pivot_row = a[col];
        for (row, values) in a.iter_mut().enumerate() {
            if row == col {
                continue;
            }
            let factor = values[col];
            for (value, pivot) in values[col..].iter_mut().zip(&pivot_row[col..]) {
                *value -= factor * pivot;
            }
        }
    }
    Some([a[0][3], a[1][3], a[2][3]])
}
