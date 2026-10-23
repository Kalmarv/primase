//! Inline ray tracing: the acceleration structure a caller supplies, the bounding-volume hierarchies the executor builds
//! over it, and the ray query a program runs against it (DXR 1.1's `RayQuery`, as naga carries it).
//!
//! A [`Scene`] is what an acceleration structure is made of: bottom levels of triangle geometry and a top level of
//! instances of them, each with a transform, a mask and the ids a hit reports. Procedural primitives (axis-aligned
//! boxes a shader intersects itself) are not carried: a scene has none, and a program that generates an intersection is
//! refused by name.
//!
//! A query visits the triangles the ray crosses in an order of the hierarchy's choosing, as hardware does; the closest
//! committed hit does not depend on the order. An opaque triangle in range is committed on the spot (and with
//! `ACCEPT_FIRST_HIT_AND_END_SEARCH` ends the search); a non-opaque one is a candidate the program commits or lets go.
//! The triangle test is the watertight one of DXR: a ray through an edge or a vertex shared by triangles hits one of
//! them.
//!
//! Checked against WARP's DXR 1.1 (`toolkit/dxil/tests/exec_warp_rq.rs`): 768 rays through random scenes with instance
//! transforms, masks and flags, every committed hit equal in its integer fields, facing included, and in distance and
//! barycentrics to 1e-4 relative. Not checked: rays that hit an edge or a vertex exactly (the scenes are random), and
//! `ACCEPT_FIRST_HIT_AND_END_SEARCH`, whose hit depends on the order a traversal visits triangles in.

use std::collections::BTreeMap;

/// A ray's flags, as DXR numbers them (and naga's `RayFlag` after them).
pub mod flags {
    pub const FORCE_OPAQUE: u32 = 0x1;
    pub const FORCE_NON_OPAQUE: u32 = 0x2;
    pub const ACCEPT_FIRST_HIT: u32 = 0x4;
    pub const SKIP_CLOSEST_HIT: u32 = 0x8;
    pub const CULL_BACK_FACING: u32 = 0x10;
    pub const CULL_FRONT_FACING: u32 = 0x20;
    pub const CULL_OPAQUE: u32 = 0x40;
    pub const CULL_NON_OPAQUE: u32 = 0x80;
    pub const SKIP_TRIANGLES: u32 = 0x100;
    pub const SKIP_PROCEDURAL: u32 = 0x200;
}

/// One geometry of a bottom level: triangles.
#[derive(Clone, Debug)]
pub struct Geometry {
    pub positions: Vec<[f32; 3]>,
    /// Vertex indices, three to a triangle; none: the positions are the triangles, three to each.
    pub indices: Option<Vec<u32>>,
    /// Whether its triangles are opaque (D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE).
    pub opaque: bool,
}

impl Geometry {
    fn triangles(&self) -> usize {
        self.indices.as_ref().map_or(self.positions.len(), Vec::len) / 3
    }
    fn triangle(&self, k: usize) -> Option<[[f32; 3]; 3]> {
        let vertex = |j: usize| match &self.indices {
            Some(i) => self.positions.get(*i.get(3 * k + j)? as usize).copied(),
            None => self.positions.get(3 * k + j).copied(),
        };
        Some([vertex(0)?, vertex(1)?, vertex(2)?])
    }
}

/// What an instance does besides place its bottom level.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstanceFlags {
    /// D3D12_RAYTRACING_INSTANCE_FLAG_TRIANGLE_CULL_DISABLE: the ray's culling of facings does not apply.
    pub cull_disable: bool,
    /// ..._TRIANGLE_FRONT_COUNTERCLOCKWISE: a triangle is front-facing when counter-clockwise (clockwise by default).
    pub front_counter_clockwise: bool,
    /// ..._FORCE_OPAQUE and ..._FORCE_NON_OPAQUE.
    pub force_opaque: bool,
    pub force_non_opaque: bool,
}

/// An instance of a bottom level in the top level.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Which bottom level.
    pub bottom: usize,
    /// Object to world: three rows of four.
    pub transform: [[f32; 4]; 3],
    /// InstanceID (24 bits).
    pub id: u32,
    /// InstanceMask: the instance is seen by a ray whose mask shares a bit with it.
    pub mask: u8,
    /// InstanceContributionToHitGroupIndex (24 bits).
    pub contribution: u32,
    pub flags: InstanceFlags,
}

/// The bottom levels and the top level of an acceleration structure.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub bottoms: Vec<Vec<Geometry>>,
    pub instances: Vec<Instance>,
}

type Vec3 = [f32; 3];

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

impl Aabb {
    const EMPTY: Aabb = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    fn grow(&mut self, p: Vec3) {
        for (k, &x) in p.iter().enumerate() {
            self.min[k] = self.min[k].min(x);
            self.max[k] = self.max[k].max(x);
        }
    }
    fn union(&mut self, other: &Aabb) {
        self.grow(other.min);
        self.grow(other.max);
    }
    /// A little larger, so that rounding never lets a ray past a primitive the box holds.
    fn padded(self) -> Aabb {
        let mut b = self;
        for k in 0..3 {
            let pad = (b.min[k].abs().max(b.max[k].abs()) * 1e-5).max(1e-20);
            b.min[k] -= pad;
            b.max[k] += pad;
        }
        b
    }
}

/// A node of a hierarchy: an inner node has its children at `first` and `first + 1`, a leaf its `count` primitives at
/// `first` of the hierarchy's order.
#[derive(Clone, Debug)]
struct Node {
    bounds: Aabb,
    first: u32,
    count: u32,
}

#[derive(Clone, Debug, Default)]
struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl Bvh {
    /// A hierarchy over primitives with these bounds: split at the median of the centroids along the longest axis.
    fn build(bounds: &[Aabb]) -> Bvh {
        let mut bvh = Bvh::default();
        if bounds.is_empty() {
            return bvh;
        }
        bvh.order = (0..bounds.len() as u32).collect();
        bvh.nodes.push(Node {
            bounds: Aabb::EMPTY,
            first: 0,
            count: 0,
        });
        bvh.split(0, 0, bounds.len(), bounds);
        bvh
    }

    fn split(&mut self, node: usize, lo: usize, hi: usize, bounds: &[Aabb]) {
        let mut total = Aabb::EMPTY;
        let mut centroids = Aabb::EMPTY;
        for &p in &self.order[lo..hi] {
            let b = &bounds[p as usize];
            total.union(b);
            centroids.grow(std::array::from_fn(|k| 0.5 * (b.min[k] + b.max[k])));
        }
        self.nodes[node].bounds = total.padded();
        if hi - lo <= 4 {
            self.nodes[node].first = lo as u32;
            self.nodes[node].count = (hi - lo) as u32;
            return;
        }
        let axis = (0..3)
            .max_by(|&a, &b| {
                (centroids.max[a] - centroids.min[a]).total_cmp(&(centroids.max[b] - centroids.min[b]))
            })
            .unwrap_or(0);
        self.order[lo..hi].sort_by(|&a, &b| {
            let (ca, cb) = (&bounds[a as usize], &bounds[b as usize]);
            (ca.min[axis] + ca.max[axis]).total_cmp(&(cb.min[axis] + cb.max[axis]))
        });
        let mid = lo + (hi - lo) / 2;
        let left = self.nodes.len();
        for _ in 0..2 {
            self.nodes.push(Node {
                bounds: Aabb::EMPTY,
                first: 0,
                count: 0,
            });
        }
        self.nodes[node].first = left as u32;
        self.nodes[node].count = 0;
        self.split(left, lo, mid, bounds);
        self.split(left + 1, mid, hi, bounds);
    }
}

/// Whether the ray crosses the box within `[tmin, tmax]` (the slab test).
fn crosses(b: &Aabb, origin: Vec3, inv: Vec3, tmin: f32, tmax: f32) -> bool {
    let (mut lo, mut hi) = (tmin, tmax);
    for k in 0..3 {
        let (a, c) = ((b.min[k] - origin[k]) * inv[k], (b.max[k] - origin[k]) * inv[k]);
        // a NaN (a zero direction on the box's plane) leaves the bounds as they are
        lo = lo.max(a.min(c));
        hi = hi.min(a.max(c));
    }
    lo <= hi
}

#[derive(Clone, Debug)]
struct BuiltBottom {
    geometries: Vec<Geometry>,
    /// The primitives of the hierarchy: (geometry, triangle).
    primitives: Vec<(u32, u32)>,
    bvh: Bvh,
}

#[derive(Clone, Debug)]
struct BuiltInstance {
    instance: Instance,
    /// World to object: three rows of four.
    inverse: [[f32; 4]; 3],
}

/// A scene with its hierarchies.
#[derive(Clone, Debug)]
pub struct Built {
    bottoms: Vec<BuiltBottom>,
    instances: Vec<BuiltInstance>,
    top: Bvh,
}

fn transform_point(m: &[[f32; 4]; 3], p: Vec3) -> Vec3 {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}
fn transform_vector(m: &[[f32; 4]; 3], v: Vec3) -> Vec3 {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// The inverse of an affine transform of three rows of four (None when it is singular).
fn invert(m: &[[f32; 4]; 3]) -> Option<[[f32; 4]; 3]> {
    let a = |r: usize, c: usize| f64::from(m[r][c]);
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv = [
        [
            (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) / det,
            (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) / det,
            (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) / det,
        ],
        [
            (a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2)) / det,
            (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) / det,
            (a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)) / det,
        ],
        [
            (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0)) / det,
            (a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1)) / det,
            (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) / det,
        ],
    ];
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [[0f32; 4]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = inv[r][c] as f32;
        }
        out[r][3] = -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]) as f32;
    }
    Some(out)
}

impl Built {
    /// Build the hierarchies of a scene. An instance of a bottom level that does not exist, or with a singular
    /// transform, is an error.
    pub fn new(scene: &Scene) -> Result<Built, String> {
        let mut bottoms = Vec::new();
        for geometries in &scene.bottoms {
            let mut primitives = Vec::new();
            let mut bounds = Vec::new();
            for (g, geometry) in geometries.iter().enumerate() {
                for k in 0..geometry.triangles() {
                    let tri = geometry.triangle(k).ok_or_else(|| {
                        format!("geometry {g}: triangle {k} names a vertex that does not exist")
                    })?;
                    let mut b = Aabb::EMPTY;
                    tri.iter().for_each(|&p| b.grow(p));
                    primitives.push((g as u32, k as u32));
                    bounds.push(b);
                }
            }
            let bvh = Bvh::build(&bounds);
            bottoms.push(BuiltBottom {
                geometries: geometries.clone(),
                primitives,
                bvh,
            });
        }
        let mut instances = Vec::new();
        let mut bounds = Vec::new();
        for (i, instance) in scene.instances.iter().enumerate() {
            let bottom = bottoms.get(instance.bottom).ok_or_else(|| {
                format!(
                    "instance {i} is of bottom level {}, which does not exist",
                    instance.bottom
                )
            })?;
            let inverse =
                invert(&instance.transform).ok_or_else(|| format!("instance {i}: a singular transform"))?;
            let mut b = Aabb::EMPTY;
            if let Some(root) = bottom.bvh.nodes.first() {
                for corner in 0..8 {
                    let c: Vec3 = std::array::from_fn(|k| {
                        if corner >> k & 1 == 0 {
                            root.bounds.min[k]
                        } else {
                            root.bounds.max[k]
                        }
                    });
                    b.grow(transform_point(&instance.transform, c));
                }
            }
            bounds.push(b);
            instances.push(BuiltInstance {
                instance: instance.clone(),
                inverse,
            });
        }
        let top = Bvh::build(&bounds);
        Ok(Built {
            bottoms,
            instances,
            top,
        })
    }
}

/// A committed or candidate hit.
#[derive(Clone, Copy, Debug)]
pub(super) struct Hit {
    /// naga's intersection kind: 1 a triangle, 2 a generated intersection, 3 a procedural candidate.
    pub kind: u32,
    pub t: f32,
    pub barycentrics: [f32; 2],
    pub front: bool,
    pub instance: u32,
    pub geometry: u32,
    pub primitive: u32,
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Top(u32),
    Instance(u32),
    Bottom(u32),
    Triangle(u32, u32),
}

/// A ray query: the ray, the search's place, and what it has found.
#[derive(Clone, Debug)]
pub(super) struct Query {
    scene: crate::ResourceId,
    pub flags: u32,
    mask: u32,
    pub tmin: f32,
    tmax: f32,
    pub origin: Vec3,
    pub direction: Vec3,
    stack: Vec<Frame>,
    started: bool,
    done: bool,
    /// The instance being searched, and the ray in its object space.
    inside: Option<(u32, Vec3, Vec3)>,
    pub candidate: Option<Hit>,
    pub committed: Option<Hit>,
}

fn inverse_direction(d: Vec3) -> Vec3 {
    d.map(|x| 1.0 / x)
}

impl Query {
    pub(super) fn new(
        scene: crate::ResourceId,
        flags: u32,
        mask: u32,
        tmin: f32,
        tmax: f32,
        origin: Vec3,
        direction: Vec3,
    ) -> Query {
        Query {
            scene,
            flags,
            mask,
            tmin,
            tmax,
            origin,
            direction,
            stack: Vec::new(),
            started: false,
            done: false,
            inside: None,
            candidate: None,
            committed: None,
        }
    }

    pub(super) fn scene(&self) -> crate::ResourceId {
        self.scene
    }

    /// End the search.
    pub(super) fn terminate(&mut self) {
        self.done = true;
        self.stack.clear();
    }

    /// Commit the candidate (a non-opaque triangle the program accepts).
    pub(super) fn confirm(&mut self) -> Result<(), String> {
        let Some(hit) = self.candidate.take() else {
            return Err("a candidate hit committed when there is none".into());
        };
        self.tmax = hit.t;
        self.committed = Some(hit);
        if self.flags & flags::ACCEPT_FIRST_HIT != 0 {
            self.terminate();
        }
        Ok(())
    }

    /// Go on to the next candidate: true when there is one for the program to commit or let go.
    pub(super) fn proceed(&mut self, scene: &Built) -> bool {
        self.candidate = None;
        if self.done {
            return false;
        }
        if !self.started {
            self.started = true;
            if self.flags & flags::SKIP_TRIANGLES == 0 && !scene.top.nodes.is_empty() {
                self.stack.push(Frame::Top(0));
            }
        }
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Top(n) => {
                    let node = &scene.top.nodes[n as usize];
                    let inv = inverse_direction(self.direction);
                    if !crosses(&node.bounds, self.origin, inv, self.tmin, self.tmax) {
                        continue;
                    }
                    if node.count == 0 {
                        self.stack.push(Frame::Top(node.first + 1));
                        self.stack.push(Frame::Top(node.first));
                    } else {
                        for k in (node.first..node.first + node.count).rev() {
                            self.stack.push(Frame::Instance(scene.top.order[k as usize]));
                        }
                    }
                }
                Frame::Instance(i) => {
                    let instance = &scene.instances[i as usize];
                    if self.mask & u32::from(instance.instance.mask) == 0 {
                        continue;
                    }
                    let bottom = &scene.bottoms[instance.instance.bottom];
                    if bottom.bvh.nodes.is_empty() {
                        continue;
                    }
                    self.inside = Some((
                        i,
                        transform_point(&instance.inverse, self.origin),
                        transform_vector(&instance.inverse, self.direction),
                    ));
                    self.stack.push(Frame::Bottom(0));
                }
                Frame::Bottom(n) => {
                    let (i, origin, direction) = self.inside.expect("inside an instance");
                    let bottom = &scene.bottoms[scene.instances[i as usize].instance.bottom];
                    let node = &bottom.bvh.nodes[n as usize];
                    if !crosses(
                        &node.bounds,
                        origin,
                        inverse_direction(direction),
                        self.tmin,
                        self.tmax,
                    ) {
                        continue;
                    }
                    if node.count == 0 {
                        self.stack.push(Frame::Bottom(node.first + 1));
                        self.stack.push(Frame::Bottom(node.first));
                    } else {
                        for k in (node.first..node.first + node.count).rev() {
                            let (g, t) = bottom.primitives[bottom.bvh.order[k as usize] as usize];
                            self.stack.push(Frame::Triangle(g, t));
                        }
                    }
                }
                Frame::Triangle(g, k) => {
                    let (i, origin, direction) = self.inside.expect("inside an instance");
                    let built = &scene.instances[i as usize];
                    let geometry = &scene.bottoms[built.instance.bottom].geometries[g as usize];
                    let Some(tri) = geometry.triangle(k as usize) else {
                        continue;
                    };
                    let Some((t, b1, b2, front_clockwise)) = intersect(&tri, origin, direction) else {
                        continue;
                    };
                    if !(self.tmin <= t && t <= self.tmax) {
                        continue;
                    }
                    let f = built.instance.flags;
                    // front-facing is clockwise unless the instance says counter-clockwise
                    let front = front_clockwise != f.front_counter_clockwise;
                    let opaque = if self.flags & flags::FORCE_OPAQUE != 0 {
                        true
                    } else if self.flags & flags::FORCE_NON_OPAQUE != 0 {
                        false
                    } else if f.force_opaque {
                        true
                    } else if f.force_non_opaque {
                        false
                    } else {
                        geometry.opaque
                    };
                    if (self.flags & flags::CULL_OPAQUE != 0 && opaque)
                        || (self.flags & flags::CULL_NON_OPAQUE != 0 && !opaque)
                    {
                        continue;
                    }
                    if !f.cull_disable
                        && ((self.flags & flags::CULL_BACK_FACING != 0 && !front)
                            || (self.flags & flags::CULL_FRONT_FACING != 0 && front))
                    {
                        continue;
                    }
                    let hit = Hit {
                        kind: 1,
                        t,
                        barycentrics: [b1, b2],
                        front,
                        instance: i,
                        geometry: g,
                        primitive: k,
                    };
                    if opaque {
                        self.committed = Some(hit);
                        self.tmax = t;
                        if self.flags & flags::ACCEPT_FIRST_HIT != 0 {
                            self.terminate();
                            return false;
                        }
                    } else {
                        self.candidate = Some(hit);
                        return true;
                    }
                }
            }
        }
        self.done = true;
        false
    }
}

/// The watertight ray-triangle test: the distance along the ray, the barycentric weights of the second and third
/// vertices, and whether the triangle's winding is clockwise as the ray origin sees it. None for a miss.
fn intersect(tri: &[Vec3; 3], origin: Vec3, direction: Vec3) -> Option<(f32, f32, f32, bool)> {
    // the axis the ray runs most along, and the other two in an order that keeps the winding
    let abs = direction.map(f32::abs);
    let kz = if abs[0] >= abs[1] && abs[0] >= abs[2] {
        0
    } else if abs[1] >= abs[2] {
        1
    } else {
        2
    };
    let (mut kx, mut ky) = ((kz + 1) % 3, (kz + 2) % 3);
    if direction[kz] < 0.0 {
        std::mem::swap(&mut kx, &mut ky);
    }
    let s = [
        direction[kx] / direction[kz],
        direction[ky] / direction[kz],
        1.0 / direction[kz],
    ];
    let rel: [Vec3; 3] = std::array::from_fn(|j| std::array::from_fn(|k| tri[j][k] - origin[k]));
    let shear = |p: &Vec3| -> [f32; 2] { [p[kx] - s[0] * p[kz], p[ky] - s[1] * p[kz]] };
    let (a, b, c) = (shear(&rel[0]), shear(&rel[1]), shear(&rel[2]));
    let mut u = c[0] * b[1] - c[1] * b[0];
    let mut v = a[0] * c[1] - a[1] * c[0];
    let mut w = b[0] * a[1] - b[1] * a[0];
    if u == 0.0 || v == 0.0 || w == 0.0 {
        // an edge: redo the products in double precision, as the algorithm does
        let (c0, c1, b0, b1, a0, a1) = (
            f64::from(c[0]),
            f64::from(c[1]),
            f64::from(b[0]),
            f64::from(b[1]),
            f64::from(a[0]),
            f64::from(a[1]),
        );
        u = (c0 * b1 - c1 * b0) as f32;
        v = (a0 * c1 - a1 * c0) as f32;
        w = (b0 * a1 - b1 * a0) as f32;
    }
    if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) {
        return None;
    }
    let det = u + v + w;
    if det == 0.0 {
        return None;
    }
    let (az, bz, cz) = (s[2] * rel[0][kz], s[2] * rel[1][kz], s[2] * rel[2][kz]);
    let t = (u * az + v * bz + w * cz) / det;
    // the winding the ray origin sees: the sign of the triangle's normal against the direction
    let e1: Vec3 = std::array::from_fn(|k| tri[1][k] - tri[0][k]);
    let e2: Vec3 = std::array::from_fn(|k| tri[2][k] - tri[0][k]);
    let normal = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let along: f32 = (0..3).map(|k| normal[k] * direction[k]).sum();
    Some((t, v / det, w / det, along < 0.0))
}

/// The intersection a query reports, as the fields of naga's `RayIntersection`, from `hit` in `scene`.
pub(super) struct Reported {
    pub kind: u32,
    pub t: f32,
    pub custom: u32,
    pub instance: u32,
    pub sbt: u32,
    pub geometry: u32,
    pub primitive: u32,
    pub barycentrics: [f32; 2],
    pub front: bool,
    /// Four columns of three rows each.
    pub object_to_world: [Vec3; 4],
    pub world_to_object: [Vec3; 4],
}

impl Reported {
    pub(super) fn none() -> Reported {
        Reported {
            kind: 0,
            t: 0.0,
            custom: 0,
            instance: 0,
            sbt: 0,
            geometry: 0,
            primitive: 0,
            barycentrics: [0.0; 2],
            front: false,
            object_to_world: [[0.0; 3]; 4],
            world_to_object: [[0.0; 3]; 4],
        }
    }

    pub(super) fn of(hit: &Hit, scene: &Built) -> Reported {
        let built = &scene.instances[hit.instance as usize];
        let columns =
            |m: &[[f32; 4]; 3]| -> [Vec3; 4] { std::array::from_fn(|c| std::array::from_fn(|r| m[r][c])) };
        Reported {
            kind: hit.kind,
            t: hit.t,
            custom: built.instance.id & 0x00ff_ffff,
            instance: hit.instance,
            sbt: built.instance.contribution & 0x00ff_ffff,
            geometry: hit.geometry,
            primitive: hit.primitive,
            barycentrics: hit.barycentrics,
            front: hit.front,
            object_to_world: columns(&built.instance.transform),
            world_to_object: columns(&built.inverse),
        }
    }
}

/// The scenes a run was bound with.
pub(super) type Scenes = BTreeMap<crate::ResourceId, Built>;
