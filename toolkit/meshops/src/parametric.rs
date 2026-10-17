//! Lowering parametric data for the outputs that need triangles or polylines: `tessellate-colliders`
//! (`core.collider` to `core.collision`) and `flatten-curves` (Bezier `core.curve`s to polylines). A writer that
//! needs them says so in its target, so the planner runs them only for it.

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::{Collider, Collision, Component, Curve, Diagonal, Heightfield, Shape, World};

use crate::{Mesh, V, box_mesh, capsule, convex_hull, cylinder, sphere};

/// Register this crate's passes.
pub(crate) fn register(reg: &mut PassRegistry) {
    reg.register("bake-texture-transforms", |o: &Options| {
        no_options(o)?;
        Ok(Box::new(crate::BakeTextureTransforms))
    });
    reg.register("tessellate-colliders", |o: &Options| {
        no_options(o)?;
        Ok(Box::new(TessellateColliders))
    });
    reg.register("flatten-curves", |o: &Options| {
        let mut tolerance = FlattenCurves::DEFAULT_TOLERANCE;
        for (k, v) in o {
            match k.as_str() {
                "tolerance" => {
                    tolerance = v
                        .parse()
                        .ok()
                        .filter(|t: &f32| t.is_finite() && *t > 0.0)
                        .ok_or_else(|| format!("tolerance {v} is not a positive number of metres"))?;
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Box::new(FlattenCurves { tolerance }))
    });
}

fn no_options(o: &Options) -> Result<(), String> {
    match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(()),
    }
}

/// `tessellate-colliders`: every `core.collider` as triangles appended to its entity's `core.collision`, in the same
/// space, with the collider's surface (a heightfield's per-cell surfaces where it has them). Boxes, convex hulls and
/// heightfields are exact; spheres, capsules and cylinders are tessellated by [`sphere`], [`capsule`] and
/// [`cylinder`]'s radius rule, counted. The colliders are removed; a heightfield stays (it is also a drawn surface).
#[derive(Debug)]
pub struct TessellateColliders;

impl Pass for TessellateColliders {
    fn name(&self) -> &'static str {
        "tessellate-colliders"
    }
    fn requires(&self) -> &[&'static str] {
        &[Collider::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Collider::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Collision::NAME]
    }
    fn approximates(&self) -> &'static str {
        "spheres, capsules and cylinders as tessellated surfaces (segments by radius)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let colliders: Vec<_> = w.iter::<Collider>().map(|(e, c)| (e, c.clone())).collect();
        for (e, c) in colliders {
            let soup = match &c.shape {
                Shape::Heightfield => {
                    let field = w
                        .get::<Heightfield>(e)
                        .ok_or_else(|| format!("{e:?}: a heightfield shape without core.heightfield"))?;
                    log.count("heightfields", 1);
                    heightfield(field, c.surface)
                }
                shape => {
                    let (mesh, kind, exact) = shape_mesh(shape).map_err(|m| format!("{e:?}: {m}"))?;
                    if exact {
                        log.count(kind, 1);
                    } else {
                        log.approximate(kind, 1);
                    }
                    let triangles = mesh.indices.as_chunks::<3>().0.to_vec();
                    let surfaces = vec![c.surface; triangles.len()];
                    (mesh.positions, triangles, surfaces)
                }
            };
            let mut collision = w.get::<Collision>(e).cloned().unwrap_or_default();
            let base = collision.positions.len() as u32;
            collision.positions.extend(soup.0);
            collision
                .triangles
                .extend(soup.1.iter().map(|t| t.map(|i| i + base)));
            collision.surfaces.extend(soup.2);
            w.insert(e, collision);
            w.remove::<Collider>(e);
        }
        Ok(())
    }
}

/// A shape's triangles, its kind for the log, and whether they are exact.
fn shape_mesh(shape: &Shape) -> Result<(Mesh, &'static str, bool), String> {
    let err = |e: crate::Error| e.to_string();
    Ok(match *shape {
        Shape::Box {
            center,
            half_extents: [x, y, z],
        } => (
            box_mesh(center, [[x, 0.0, 0.0], [0.0, y, 0.0], [0.0, 0.0, z]]).map_err(err)?,
            "boxes",
            true,
        ),
        Shape::Sphere { center, radius } => {
            (sphere(center, radius).map_err(err)?, "spheres tessellated", false)
        }
        Shape::Capsule { a, b, radius } => {
            (capsule(a, b, radius).map_err(err)?, "capsules tessellated", false)
        }
        Shape::Cylinder { a, b, radius } => (
            cylinder(a, b, radius).map_err(err)?,
            "cylinders tessellated",
            false,
        ),
        Shape::Convex { ref points } => (convex_hull(points).map_err(err)?, "convex hulls", true),
        Shape::Heightfield => return Err("a heightfield is not a shape mesh".into()),
    })
}

/// Positions, triangles (counter-clockwise seen from +Y) and surfaces of a heightfield: two triangles per cell that
/// is not a hole, split along its diagonal.
fn heightfield(f: &Heightfield, surface: u16) -> (Vec<V>, Vec<[u32; 3]>, Vec<u16>) {
    let (cols, rows) = (f.columns as usize, f.rows as usize);
    let positions = (0..rows)
        .flat_map(|r| {
            (0..cols).map(move |c| {
                [
                    c as f32 * f.spacing[0],
                    f.heights[r * cols + c],
                    r as f32 * f.spacing[1],
                ]
            })
        })
        .collect();
    let (mut triangles, mut surfaces) = (Vec::new(), Vec::new());
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let cell = r * (cols - 1) + c;
            if f.holes.as_ref().is_some_and(|h| h[cell]) {
                continue;
            }
            // corners a (c, r), b (c + 1, r), d (c, r + 1), e (c + 1, r + 1); +Z is south, so a, d, b turns
            // counter-clockwise seen from above (its normal is +Y)
            let at = |dc: usize, dr: usize| ((r + dr) * cols + c + dc) as u32;
            let (a, b, d, e) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
            let pair = match f.diagonal {
                Diagonal::Main => [[a, e, b], [a, d, e]],
                Diagonal::Anti => [[a, d, b], [b, d, e]],
            };
            triangles.extend(pair);
            let s = f.surfaces.as_ref().map_or(surface, |s| s[cell]);
            surfaces.extend([s, s]);
        }
    }
    (positions, triangles, surfaces)
}

/// `flatten-curves{tolerance=M}`: every Bezier `core.curve` (one with tangents) as the polyline through points on it
/// no further than `tolerance` metres from the curve, each segment subdivided at equal parameter steps until its
/// control polygon lies within the tolerance of its chord. Up directions are interpolated along each segment and
/// renormalised. Polylines are unchanged.
#[derive(Debug)]
pub struct FlattenCurves {
    pub tolerance: f32,
}

impl FlattenCurves {
    /// 5 mm.
    pub const DEFAULT_TOLERANCE: f32 = 0.005;
}

impl Pass for FlattenCurves {
    fn name(&self) -> &'static str {
        "flatten-curves"
    }
    fn options(&self) -> Options {
        Options::from([("tolerance".to_owned(), self.tolerance.to_string())])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Curve::NAME]
    }
    fn approximates(&self) -> &'static str {
        "Bezier curves as polylines within the tolerance"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let curved: Vec<_> = w
            .iter::<Curve>()
            .filter(|(_, c)| c.tangents.is_some())
            .map(|(e, c)| (e, c.clone()))
            .collect();
        for (e, c) in curved {
            let flat = flatten(&c, self.tolerance);
            log.approximate("Bezier curves flattened", 1);
            log.count("polyline points", flat.points.len() as u64);
            w.insert(e, flat);
        }
        Ok(())
    }
}

fn lerp(a: V, b: V, t: f32) -> V {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// The point at `t` of the cubic Bezier `p`.
fn bezier(p: [V; 4], t: f32) -> V {
    let (a, b, c) = (lerp(p[0], p[1], t), lerp(p[1], p[2], t), lerp(p[2], p[3], t));
    lerp(lerp(a, b, t), lerp(b, c, t), t)
}

fn distance_to_segment(q: V, a: V, b: V) -> f32 {
    let ab = crate::sub(b, a);
    let len2 = crate::dot(ab, ab);
    let t = if len2 > 0.0 {
        (crate::dot(crate::sub(q, a), ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = crate::sub(q, lerp(a, b, t));
    crate::dot(d, d).sqrt()
}

/// Steps a segment needs: the fewest equal parameter steps whose pieces' control polygons each lie within the
/// tolerance of their chords (a bound on the curve's distance from the chord), at most 1024.
fn steps(p: [V; 4], tolerance: f32) -> usize {
    let mut n = 1;
    while n < 1024 {
        let ok = (0..n).all(|i| {
            let (t0, t1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            // the piece's control points by de Casteljau on [t0, t1]
            let piece = sub_bezier(p, t0, t1);
            distance_to_segment(piece[1], piece[0], piece[3]) <= tolerance
                && distance_to_segment(piece[2], piece[0], piece[3]) <= tolerance
        });
        if ok {
            break;
        }
        n *= 2;
    }
    n
}

/// The control points of `p` restricted to `[t0, t1]`.
fn sub_bezier(p: [V; 4], t0: f32, t1: f32) -> [V; 4] {
    // split at t1, keep the left part; then split that at t0 / t1, keep the right part
    let left = split(p, t1).0;
    if t1 <= 0.0 {
        return [left[0]; 4];
    }
    split(left, t0 / t1).1
}

/// `p` split at `t`: the left and right control points.
fn split(p: [V; 4], t: f32) -> ([V; 4], [V; 4]) {
    let (a, b, c) = (lerp(p[0], p[1], t), lerp(p[1], p[2], t), lerp(p[2], p[3], t));
    let (d, e) = (lerp(a, b, t), lerp(b, c, t));
    let m = lerp(d, e, t);
    ([p[0], a, d, m], [m, e, c, p[3]])
}

fn flatten(c: &Curve, tolerance: f32) -> Curve {
    let Some(t) = &c.tangents else {
        return c.clone();
    };
    let n = c.points.len();
    let segments = if c.closed { n } else { n - 1 };
    let (mut points, mut up) = (vec![c.points[0]], c.up.as_ref().map(|u| vec![u[0]]));
    for s in 0..segments {
        let (i, j) = (s, (s + 1) % n);
        let p = [c.points[i], t[i][1], t[j][0], c.points[j]];
        let k = steps(p, tolerance);
        for step in 1..=k {
            let at = step as f32 / k as f32;
            // the closing point of a closed curve is its first point again: left implicit, as `closed` says
            if c.closed && j == 0 && step == k {
                break;
            }
            points.push(if step == k { c.points[j] } else { bezier(p, at) });
            if let (Some(up), Some(u)) = (up.as_mut(), &c.up) {
                let v = lerp(u[i], u[j], at);
                let l = crate::dot(v, v).sqrt();
                up.push(if l > 0.0 { v.map(|x| x / l) } else { u[j] });
            }
        }
    }
    Curve {
        kind: c.kind.clone(),
        points,
        closed: c.closed,
        tangents: None,
        up,
    }
}
