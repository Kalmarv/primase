use crate::{Result, Value, invalid};
use primase_core::vector::{cross, dot, sub, unit};

/// The most vertices a convex core may hold. Every stage of the tessellation is superlinear in the count (the plane
/// intersections cubic, the hull and the flat-shape test not far from it), and a file states it freely; Sekiro's own
/// shapes have at most 152 of them (177 convex shapes of its objects), and Havok's polytope indices are bytes. This is
/// twice a byte's range: a judgement, not a limit the format states.
pub const MAX_VERTICES: usize = 512;
/// The most planes a convex core may hold (a polytope of [`MAX_VERTICES`] vertices has at most twice that many faces;
/// Sekiro's objects have at most 150). The intersection of half-spaces takes 2.4 s at this many, 27.8 s at twice.
pub const MAX_PLANES: usize = 512;

/// Stored polytope core. Radius expansion is a tessellation policy, not the
/// rounded surface of a Havok convex shape (notably at capsule corners). An
/// `hknpConvexShape` stores its vertices only: its faces and planes are those of
/// the vertices' convex hull (one triangle each), not stored ones.
#[derive(Clone, Debug, PartialEq)]
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
        if stored.len() > MAX_VERTICES {
            return Err(invalid(
                shape.offset,
                format!(
                    "{} convex vertices, more than the {MAX_VERTICES} read",
                    stored.len()
                ),
            ));
        }
        let mut vertices = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let v = stored.get(i)?.vector::<4>()?;
            vertices.push([v[0], v[1], v[2]]);
        }
        if shape.type_name() == "hknpConvexShape" {
            return Self::hull_of(vertices, radius);
        }
        if shape.type_name() == "hknpSphereShape" {
            // its one vertex is the centre; the radius is the sphere's
            if vertices.len() != 1 {
                return Err(invalid(shape.offset, "a sphere shape without exactly one vertex"));
            }
            let core = Self {
                vertices,
                planes: Vec::new(),
                faces: Vec::new(),
                radius,
            };
            core.validate()?;
            return Ok(core);
        }
        let stored = shape.field("planes")?.array()?;
        if stored.len() > MAX_PLANES {
            return Err(invalid(
                shape.offset,
                format!("{} convex planes, more than the {MAX_PLANES} read", stored.len()),
            ));
        }
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
    /// The core of vertices that store no faces: their convex hull's triangles, each with its outward plane in the
    /// stored convention (`n . v + w = 0` on the face).
    fn hull_of(vertices: Vec<[f32; 3]>, radius: f32) -> Result<Self> {
        if vertices.len() < 4 || vertices.iter().flatten().any(|x| !x.is_finite()) {
            return Err(invalid(0, "convex shape without a volume of finite vertices"));
        }
        let points: Vec<[f64; 3]> = vertices.iter().map(|v| v.map(f64::from)).collect();
        if let Some((normal, polygon)) = flat_polygon(&points)? {
            // a polygon: its two faces, each on its side's plane
            let offset = -dot(normal, points[polygon[0]]);
            let plane = |sign: f64| [normal[0], normal[1], normal[2], offset].map(|x| (sign * x) as f32);
            let mut back = polygon.clone();
            back.reverse();
            let core = Self {
                vertices,
                planes: vec![plane(1.), plane(-1.)],
                faces: vec![polygon, back],
                radius,
            };
            core.validate()?;
            return Ok(core);
        }
        let scale = points.iter().flatten().fold(1f64, |m, x| m.max(x.abs()));
        let triangles = hull(&points, 1e-8 * scale)?;
        let mut planes = Vec::with_capacity(triangles.len());
        let mut faces = Vec::with_capacity(triangles.len());
        for t in triangles {
            let n = cross(sub(points[t[1]], points[t[0]]), sub(points[t[2]], points[t[0]]));
            let length = dot(n, n).sqrt();
            let n = n.map(|x| x / length);
            planes.push([
                n[0] as f32,
                n[1] as f32,
                n[2] as f32,
                -dot(n, points[t[0]]) as f32,
            ]);
            faces.push(t.to_vec());
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
        if self.vertices.len() > MAX_VERTICES || self.planes.len() > MAX_PLANES {
            return Err(invalid(
                0,
                format!(
                    "{} convex vertices and {} planes, more than the {MAX_VERTICES} and {MAX_PLANES} read",
                    self.vertices.len(),
                    self.planes.len()
                ),
            ));
        }
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
        if let Some(triangles) = self.point_triangles()? {
            return Ok(triangles);
        }
        if let Some(triangles) = self.flat_triangles()? {
            return Ok(triangles);
        }
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
                let bound = plane_bound(p);
                if p.iter().any(|x| !x.is_finite())
                    || planes.iter().any(|q| distance(*q, p) > bound)
                    || planes.iter().filter(|q| distance(**q, p).abs() <= bound).count() < 3
                {
                    return Err(invalid(
                        0,
                        "convex f32 vertex exceeds its plane bound (0.01 mm or 4 f32 ULP)",
                    ));
                }
            }
            if !planes.iter().any(|q| {
                output.iter().all(|p| {
                    let p = p.map(f64::from);
                    distance(*q, p).abs() <= plane_bound(p)
                })
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
    /// The rounded surface Havok gives a convex shape, the core's vertices swept by a sphere of the convex radius,
    /// within `max_error` metres: the convex hull of every core vertex plus every vertex of a sphere polyhedron
    /// inscribed in the radius ([`sphere_directions`], refined until each face lies within `max_error` of the
    /// sphere). Every hull vertex lies on the true surface, so the hull lies inside the shape, and no point of the
    /// shape is further than `max_error` from it (before f32 rounding). A core of no radius is its own polytope
    /// ([`ConvexCore::expanded_triangles`]). Triangles wind counter-clockwise from outside.
    pub fn rounded_triangles(&self, max_error: f64) -> Result<Vec<[[f32; 3]; 3]>> {
        self.validate()?;
        if !(max_error.is_finite() && max_error > 0.) {
            return Err(invalid(
                0,
                "the rounded surface's error bound must be a positive length",
            ));
        }
        if self.radius == 0. {
            return self.expanded_triangles();
        }
        let radius = f64::from(self.radius);
        let directions = sphere_directions(radius, max_error)?;
        let mut points = Vec::with_capacity(self.vertices.len() * directions.len());
        for v in &self.vertices {
            let v = v.map(f64::from);
            for d in &directions {
                points.push(std::array::from_fn(|k| v[k] + radius * d[k]));
            }
        }
        let scale = points.iter().flatten().fold(1f64, |m, x| m.max(x.abs()));
        let faces = hull(&points, 1e-12 * scale)?;
        let mut triangles = Vec::with_capacity(faces.len());
        for f in faces {
            let output = f.map(|i| points[i].map(|x| x as f32));
            let rounded = output.map(|p| p.map(f64::from));
            let area = cross(sub(rounded[1], rounded[0]), sub(rounded[2], rounded[0]));
            // f32 rounding may collapse a sliver: no zero-area triangle is emitted
            if dot(area, area) > 0. {
                triangles.push(output);
            }
        }
        Ok(triangles)
    }
    /// A core that is a point (a sphere's centre: one vertex, no planes): the cube of half-edge `radius` around it,
    /// the sharp approximation the offset half-spaces give every core (a capsule a box around its segment), twelve
    /// triangles wound counter-clockwise from outside. `None` for a core with planes; an error for a point of no
    /// radius, which bounds nothing.
    fn point_triangles(&self) -> Result<Option<Vec<[[f32; 3]; 3]>>> {
        let ([centre], true) = (self.vertices.as_slice(), self.planes.is_empty()) else {
            return Ok(None);
        };
        if self.radius <= 0. {
            return Err(invalid(0, "a point core without a radius bounds nothing"));
        }
        let (c, r) = (centre.map(f64::from), f64::from(self.radius));
        // corner k: bit 0 x, bit 1 y, bit 2 z (set: +r)
        let corner = |k: usize| -> [f32; 3] {
            std::array::from_fn(|axis| (c[axis] + if k >> axis & 1 == 1 { r } else { -r }) as f32)
        };
        // each face's corners counter-clockwise seen from outside: -x, +x, -y, +y, -z, +z
        const FACES: [[usize; 4]; 6] = [
            [0, 4, 6, 2],
            [1, 3, 7, 5],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 2, 3, 1],
            [4, 5, 7, 6],
        ];
        Ok(Some(
            FACES
                .iter()
                .flat_map(|f| [[f[0], f[1], f[2]], [f[0], f[2], f[3]]])
                .map(|t| t.map(corner))
                .collect(),
        ))
    }
    /// A core of zero thickness (two opposite stored planes at one offset, every vertex on them) bounds no volume:
    /// it is a polygon. Normals agree to f32 rounding; offsets and vertices to the plane bound ([`plane_bound`]: 0.01
    /// mm, or 4 f32 ULP where coarser), as thin slabs are stored. Its faces on those planes, offset by the radius and fan-triangulated, are the tessellation;
    /// the rim of height 2 x radius is not emitted (with radius 0 there is none). `None` for a core with volume.
    fn flat_triangles(&self) -> Result<Option<Vec<[[f32; 3]; 3]>>> {
        let normalized: Vec<[f64; 4]> = self
            .planes
            .iter()
            .map(|p| {
                let p = p.map(f64::from);
                let length = dot([p[0], p[1], p[2]], [p[0], p[1], p[2]]).sqrt();
                if length == 0. {
                    return Err(invalid(0, "zero convex plane normal"));
                }
                Ok(p.map(|x| x / length))
            })
            .collect::<Result<_>>()?;
        // Offsets agree to f32 rounding (four units in the last place of the offset, or of 1).
        let same =
            |a: f64, b: f64| (a - b).abs() <= 4. * f64::from(f32::EPSILON) * a.abs().max(b.abs()).max(1.);
        let opposite = |a: &[f64; 4], b: &[f64; 4]| {
            (0..3).all(|k| same(a[k], -b[k]))
                && (a[3] + b[3]).abs() <= 1e-5f64.max(4. * f64::from(f32::EPSILON) * a[3].abs())
        };
        let Some(base) = normalized
            .iter()
            .find(|a| normalized.iter().any(|b| opposite(a, b)))
            .copied()
        else {
            return Ok(None);
        };
        let on_base = |v: [f32; 3]| {
            let v = v.map(f64::from);
            (dot([base[0], base[1], base[2]], v) + base[3]).abs() <= plane_bound(v)
        };
        if !self.vertices.iter().all(|&v| on_base(v)) {
            return Err(invalid(
                0,
                "zero-thickness convex core with vertices off its plane",
            ));
        }
        let mut triangles = Vec::new();
        let mut emitted = Vec::<[f64; 4]>::new();
        for (face, plane) in self.faces.iter().zip(&normalized) {
            let facing = (0..4).all(|k| same(plane[k], base[k])) || opposite(plane, &base);
            if !facing || emitted.iter().any(|e| (0..4).all(|k| same(e[k], plane[k]))) {
                continue;
            }
            emitted.push(*plane);
            let normal = [plane[0], plane[1], plane[2]];
            let radius = f64::from(self.radius);
            let corner = |i: usize| -> [f64; 3] {
                let v = self.vertices[i].map(f64::from);
                std::array::from_fn(|k| v[k] + radius * normal[k])
            };
            for k in 1..face.len() - 1 {
                let mut t = [corner(face[0]), corner(face[k]), corner(face[k + 1])];
                // Wind each triangle counter-clockwise about its face's outward normal.
                if dot(cross(sub(t[1], t[0]), sub(t[2], t[0])), normal) < 0. {
                    t.swap(1, 2);
                }
                let output = t.map(|p| p.map(|x| x as f32));
                let rounded = output.map(|p| p.map(f64::from));
                let area = cross(sub(rounded[1], rounded[0]), sub(rounded[2], rounded[0]));
                if output.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(0, "nonfinite flat convex vertex"));
                }
                if dot(area, area) > 0. {
                    triangles.push(output);
                }
            }
        }
        if triangles.is_empty() {
            return Err(invalid(0, "zero-thickness convex core without area"));
        }
        Ok(Some(triangles))
    }
}
/// Points within the plane bound of one plane ([`plane_bound`]): its unit normal (from the largest triangle the
/// points span) and their convex hull in it, counter-clockwise about the normal, collinear points left out. `None`
/// for points with volume; an error for points spanning no area.
fn flat_polygon(points: &[[f64; 3]]) -> Result<Option<([f64; 3], Vec<usize>)>> {
    let mut best = (0., [0.; 3]);
    for i in 0..points.len() {
        for j in i + 1..points.len() {
            for k in j + 1..points.len() {
                let n = cross(sub(points[j], points[i]), sub(points[k], points[i]));
                let area = dot(n, n);
                if area > best.0 {
                    best = (area, n);
                }
            }
        }
    }
    if best.0 == 0. {
        return Err(invalid(0, "convex shape spanning no area"));
    }
    let length = best.0.sqrt();
    let normal = best.1.map(|x| x / length);
    let origin = points[0];
    if points
        .iter()
        .any(|&p| dot(normal, sub(p, origin)).abs() > plane_bound(p))
    {
        return Ok(None);
    }
    // in-plane coordinates, then Andrew's monotone chain
    let u = {
        let far = points
            .iter()
            .map(|&p| sub(p, origin))
            .max_by(|a, b| dot(*a, *a).total_cmp(&dot(*b, *b)))
            .expect("points");
        let l = dot(far, far).sqrt();
        far.map(|x| x / l)
    };
    let v = cross(normal, u);
    let flat: Vec<[f64; 2]> = points
        .iter()
        .map(|&p| {
            let d = sub(p, origin);
            [dot(d, u), dot(d, v)]
        })
        .collect();
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&a, &b| {
        flat[a][0]
            .total_cmp(&flat[b][0])
            .then(flat[a][1].total_cmp(&flat[b][1]))
    });
    let turn = |o: usize, a: usize, b: usize| {
        (flat[a][0] - flat[o][0]) * (flat[b][1] - flat[o][1])
            - (flat[a][1] - flat[o][1]) * (flat[b][0] - flat[o][0])
    };
    let mut hull: Vec<usize> = Vec::new();
    for pass in [order.clone(), order.into_iter().rev().collect()] {
        let start = hull.len();
        for i in pass {
            while hull.len() >= start + 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], i) <= 0. {
                hull.pop();
            }
            hull.push(i);
        }
        hull.pop();
    }
    if hull.len() < 3 {
        return Err(invalid(0, "convex shape spanning no area"));
    }
    Ok(Some((normal, hull)))
}

/// The most subdivisions [`sphere_directions`] makes (163,842 directions).
const MAX_SPHERE_LEVEL: u32 = 7;

/// Unit directions of a sphere polyhedron (an icosahedron, each triangle split in four and projected onto the sphere,
/// as often as needed) whose every face, scaled to `radius`, lies within `max_error` of the sphere: a face whose
/// vertices are at angle `a` from its normal lies `radius * (1 - cos a)` inside the sphere at its centre.
fn sphere_directions(radius: f64, max_error: f64) -> Result<Vec<[f64; 3]>> {
    let t = (1. + 5f64.sqrt()) / 2.;
    let mut points: Vec<[f64; 3]> = [
        [-1., t, 0.],
        [1., t, 0.],
        [-1., -t, 0.],
        [1., -t, 0.],
        [0., -1., t],
        [0., 1., t],
        [0., -1., -t],
        [0., 1., -t],
        [t, 0., -1.],
        [t, 0., 1.],
        [-t, 0., -1.],
        [-t, 0., 1.],
    ]
    .into_iter()
    .map(|p| unit(p).ok_or_else(|| invalid(0, "an icosahedron vertex at the centre")))
    .collect::<Result<_>>()?;
    let mut faces: Vec<[usize; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for level in 0..=MAX_SPHERE_LEVEL {
        // the deepest face: its plane's distance from the centre is the cosine of its vertices' angle to its normal
        let mut depth = 0f64;
        for f in &faces {
            let n = unit(cross(
                sub(points[f[1]], points[f[0]]),
                sub(points[f[2]], points[f[0]]),
            ))
            .ok_or_else(|| invalid(0, "a sphere polyhedron's face with no area"))?;
            depth = depth.max(radius * (1. - dot(n, points[f[0]]).abs()));
        }
        if depth <= max_error {
            return Ok(points);
        }
        if level == MAX_SPHERE_LEVEL {
            break;
        }
        let mut midpoints = std::collections::BTreeMap::<(usize, usize), usize>::new();
        let mut midpoint = |a: usize, b: usize, points: &mut Vec<[f64; 3]>| -> Result<usize> {
            if let Some(&i) = midpoints.get(&(a.min(b), a.max(b))) {
                return Ok(i);
            }
            let direction = unit(primase_core::vector::add(points[a], points[b]))
                .ok_or_else(|| invalid(0, "a sphere polyhedron's edge between opposite directions"))?;
            points.push(direction);
            midpoints.insert((a.min(b), a.max(b)), points.len() - 1);
            Ok(points.len() - 1)
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let (ab, bc, ca) = (
                midpoint(a, b, &mut points)?,
                midpoint(b, c, &mut points)?,
                midpoint(c, a, &mut points)?,
            );
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    Err(invalid(
        0,
        format!(
            "a rounded surface of radius {radius} within {max_error} needs more than {MAX_SPHERE_LEVEL} subdivisions"
        ),
    ))
}
/// 0.01 mm, or four f32 units in the last place at the vertex's magnitude when that is coarser (shapes far from their
/// body's origin cannot be rounded to f32 more closely).
fn plane_bound(p: [f64; 3]) -> f64 {
    let magnitude = p.iter().fold(0f64, |m, x| m.max(x.abs()));
    1e-5f64.max(magnitude * 4. * f64::from(f32::EPSILON))
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

#[cfg(test)]
mod tests {
    use super::ConvexCore;

    #[test]
    fn vertices_alone_are_their_hull() {
        // an octahedron's six corners (no four on one plane) and its centre: eight hull triangles over the corners
        let vertices: Vec<[f32; 3]> = vec![
            [1., 0., 0.],
            [-1., 0., 0.],
            [0., 1., 0.],
            [0., -1., 0.],
            [0., 0., 1.],
            [0., 0., -1.],
            [0., 0., 0.],
        ];
        let core = ConvexCore::hull_of(vertices, 0.).unwrap();
        assert_eq!(core.faces.len(), 8);
        assert!(core.faces.iter().flatten().all(|&i| i < 6));
        // every plane faces away from the centre, at distance 1 / sqrt(3)
        for p in &core.planes {
            assert!((p[3] + 1. / 3f32.sqrt()).abs() < 1e-6, "{p:?}");
        }
        assert_eq!(core.expanded_triangles().unwrap().len(), 8);
    }

    #[test]
    fn flat_vertices_are_a_two_sided_polygon() {
        // a 12 m square within 0.06 mm of y = 0, as an asset stores one, and a point inside it
        let vertices = vec![
            [-6., 5.9e-5, -6.],
            [6., 3.6e-5, -6.],
            [6., -2.85e-5, 6.],
            [-6., -5.7e-6, 6.],
            [0., 1.52e-5, 0.],
        ];
        let core = ConvexCore::hull_of(vertices, 0.01).unwrap();
        assert_eq!(core.faces.len(), 2);
        assert_eq!(core.faces[0].len(), 4);
        let reversed: Vec<usize> = core.faces[0].iter().rev().copied().collect();
        assert_eq!(core.faces[1], reversed);
        // the two faces, each offset by the radius along its own normal
        let t = core.expanded_triangles().unwrap();
        assert_eq!(t.len(), 4);
        assert!(t.iter().flatten().all(|v| (v[1].abs() - 0.01).abs() < 1e-4));
    }

    #[test]
    fn points_spanning_no_area_are_refused() {
        let line = vec![[0., 0., 0.], [1., 0., 0.], [2., 0., 0.], [3., 0., 0.]];
        assert!(ConvexCore::hull_of(line, 0.).is_err());
    }
}
