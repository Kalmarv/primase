use super::*;
fn segments(r: f32) -> usize {
    if r <= 0.25 {
        4
    } else if r <= 0.5 {
        6
    } else if r <= 2. {
        8
    } else if r <= 8. {
        12
    } else {
        16
    }
}
fn basis(axis: V) -> (V, V) {
    let helper = if axis[2].abs() < 0.9 {
        [0., 0., 1.]
    } else {
        [1., 0., 0.]
    };
    let u = cross(helper, axis);
    let u = unit(u);
    (u, cross(axis, u))
}
fn vertex(m: &mut Mesh, p: V) -> u32 {
    let i = m.positions.len() as u32;
    m.positions.push(p);
    i
}
fn tri(m: &mut Mesh, a: u32, b: u32, c: u32) {
    m.indices.extend_from_slice(&[a, b, c]);
}
fn ring(m: &mut Mesh, c: V, u: V, v: V, r: f32, s: usize) -> u32 {
    let first = m.positions.len() as u32;
    for i in 0..s {
        let a = 2. * std::f64::consts::PI * i as f64 / s as f64;
        vertex(
            m,
            add(
                add(c, mul(u, (a.cos() * r as f64) as f32)),
                mul(v, (a.sin() * r as f64) as f32),
            ),
        );
    }
    first
}
fn band(m: &mut Mesh, a: u32, b: u32, s: usize) {
    for i in 0..s {
        let (i, j) = (i as u32, ((i + 1) % s) as u32);
        tri(m, a + i, a + j, b + j);
        tri(m, a + i, b + j, b + i);
    }
}
fn finish(m: Mesh) -> Result<Mesh> {
    m.validate()?;
    Ok(m)
}
/// Eight corners in x/y/z bit order and twelve outward triangles. A left-handed
/// half-axis basis negates its z axis first. No deduplication or degenerate removal.
pub fn box_mesh(c: V, mut axes: [V; 3]) -> Result<Mesh> {
    finite(&c)?;
    for a in axes {
        finite(&a)?;
    }
    if dot(cross(axes[0], axes[1]), axes[2]) < 0. {
        axes[2] = mul(axes[2], -1.);
    }
    let mut m = Mesh::default();
    for i in 0..8 {
        let mut p = c;
        for (k, a) in axes.iter().enumerate() {
            p = add(p, if i & (1 << k) != 0 { *a } else { mul(*a, -1.) });
        }
        vertex(&mut m, p);
    }
    m.indices = vec![
        0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 1, 5, 0, 5, 4, 2, 6, 7, 2, 7, 3, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1,
        7, 5,
    ];
    finish(m)
}
/// Latitude sphere, +Z poles. Radius uses abs; segments are 4/6/8/12/16
/// at radius boundaries .25/.5/2/8, with max(2,segments/2) latitude divisions.
/// Trig is f64 followed by f32 vector arithmetic; zero-radius faces are retained.
pub fn sphere(c: V, r: f32) -> Result<Mesh> {
    finite(&c)?;
    finite(&[r])?;
    let r = r.abs();
    let s = segments(r);
    let rows = 2.max(s / 2);
    let axis = [0., 0., 1.];
    let (u, v) = basis(axis);
    let mut m = Mesh::default();
    let bottom = vertex(&mut m, sub(c, mul(axis, r)));
    let mut prev = 0;
    for j in 1..rows {
        let phi = -std::f64::consts::PI / 2. + std::f64::consts::PI * j as f64 / rows as f64;
        let rr = ring(
            &mut m,
            add(c, mul(axis, (phi.sin() * r as f64) as f32)),
            u,
            v,
            (phi.cos() * r as f64) as f32,
            s,
        );
        if j == 1 {
            for i in 0..s {
                tri(&mut m, bottom, rr + ((i + 1) % s) as u32, rr + i as u32);
            }
        } else {
            band(&mut m, prev, rr, s);
        }
        prev = rr;
    }
    let top = vertex(&mut m, add(c, mul(axis, r)));
    for i in 0..s {
        tri(&mut m, top, prev + i as u32, prev + ((i + 1) % s) as u32);
    }
    finish(m)
}
fn axial(a: V, b: V, r: f32) -> Result<(V, V, V, f32, usize)> {
    finite(&a)?;
    finite(&b)?;
    finite(&[r])?;
    let d = sub(b, a);
    let len = vector::Vec3::from(d).length();
    finite(&[len])?;
    let axis = if len > 1e-6 { unit(d) } else { [0., 0., 1.] };
    let (u, v) = basis(axis);
    Ok((axis, u, v, r.abs(), segments(r.abs())))
}
/// Two endpoint rings and fan caps using their first vertices (no added centres).
/// Radius/segment rules match sphere; axes shorter than 1e-6 use +Z.
pub fn cylinder(a: V, b: V, r: f32) -> Result<Mesh> {
    let (_, u, v, r, s) = axial(a, b, r)?;
    let mut m = Mesh::default();
    let ra = ring(&mut m, a, u, v, r, s);
    let rb = ring(&mut m, b, u, v, r, s);
    band(&mut m, ra, rb, s);
    for i in 1..s - 1 {
        tri(&mut m, ra, ra + (i + 1) as u32, ra + i as u32);
        tri(&mut m, rb, rb + i as u32, rb + (i + 1) as u32);
    }
    finish(m)
}
/// Capsule hemispheres use one ring when radius<=1, otherwise max(2,segments/4).
/// Rings, poles and triangle traversal match the reference primitive writer.
pub fn capsule(a: V, b: V, r: f32) -> Result<Mesh> {
    let (axis, u, v, r, s) = axial(a, b, r)?;
    let h = if r <= 1. { 1 } else { 2.max(s / 4) };
    let mut m = Mesh::default();
    let pole = vertex(&mut m, sub(a, mul(axis, r)));
    let mut prev = 0;
    for j in 1..=h {
        let phi = -std::f64::consts::PI / 2. + (std::f64::consts::PI / 2.) * j as f64 / h as f64;
        let rr = ring(
            &mut m,
            add(a, mul(axis, (phi.sin() * r as f64) as f32)),
            u,
            v,
            (phi.cos() * r as f64) as f32,
            s,
        );
        if j == 1 {
            for i in 0..s {
                tri(&mut m, pole, rr + ((i + 1) % s) as u32, rr + i as u32);
            }
        } else {
            band(&mut m, prev, rr, s);
        }
        prev = rr;
    }
    let rb = ring(&mut m, b, u, v, r, s);
    band(&mut m, prev, rb, s);
    prev = rb;
    for j in 1..h {
        let phi = (std::f64::consts::PI / 2.) * j as f64 / h as f64;
        let rr = ring(
            &mut m,
            add(b, mul(axis, (phi.sin() * r as f64) as f32)),
            u,
            v,
            (phi.cos() * r as f64) as f32,
            s,
        );
        band(&mut m, prev, rr, s);
        prev = rr;
    }
    let pole = vertex(&mut m, add(b, mul(axis, r)));
    for i in 0..s {
        tri(&mut m, pole, prev + i as u32, prev + ((i + 1) % s) as u32);
    }
    finish(m)
}
/// A disc is a cylinder with endpoint offsets +/-half_axis from its centre.
/// Pass [margin,0,0] for the reference local-X disc; transform endpoints beforehand
/// when the enclosing bound has a placement transform.
pub fn disc(c: V, half_axis: V, r: f32) -> Result<Mesh> {
    finite(&c)?;
    finite(&half_axis)?;
    cylinder(sub(c, half_axis), add(c, half_axis), r)
}
