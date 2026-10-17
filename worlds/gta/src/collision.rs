//! Static map collision: the reader's side ([`attach_map_bounds`]: the ybn bounds of the region's map archives, raw,
//! with the material names they index) and `lower-gta-collision`, which makes them `core.collision`.
//!
//! Which bounds: the live ybn files whose box meets the region (every live one is read for its box, as the game
//! streams them by those boxes). The `hi@` and `ma@` copies (finer collision for weapons and vehicles) are left out:
//! the plain bounds are what a body walks or rolls on.
//!
//! `lower-gta-collision{x0,y0,x1,y1}`: every [`GtaBounds`] entity becomes an entity with an identity `core.transform`
//! and a `core.collision` triangle soup in the core frame: geometry triangles as stored; boxes, spheres, capsules and
//! cylinders tessellated (approximated, counted); composite children placed by their transforms; children flagged
//! foliage (type flag bit 19) or river (bit 27) left out, as the earlier exporter did (counted). Each triangle's
//! surface is its material's name in the install's material table (`materials.dat`), in the world's `Surfaces`.
//! Given a region, only triangles whose centre lies in it are kept.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the map's collision files from the install"
)]
use std::fs::File;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Component, Entity, JsonError, Surfaces, Transform, World};

use crate::archive_set::ArchiveSet;
use crate::bounds::{Bound, Contents, Polygon, read_ybn};
use crate::keys::Keys;
use crate::region::GTA;
use crate::{Result, invalid};

/// One ybn's bound as stored (GTA's frame, metres, world space for map bounds), and its file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaBounds {
    pub path: String,
    pub bound: Bound,
}

impl Component for GtaBounds {
    const NAME: &'static str = "gta.bounds";
    /// A summary: the file and the root bound's kind and box.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "path": self.path,
            "kind": self.bound.header.kind,
            "box": [self.bound.header.box_min, self.bound.header.box_max],
        })))
    }
}

/// The install's collision material names (`materials.dat`), by material index.
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize)]
pub struct GtaMaterials(pub Vec<String>);

impl primase_core::Resource for GtaMaterials {
    const NAME: &'static str = "gta.materials";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// The material names of a `materials.dat`: after its version line, the first word of every line that is not a
/// comment (`#`) or blank, in order.
pub fn material_names(text: &str) -> Result<Vec<String>> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let version = lines.next().ok_or_else(|| invalid("materials.dat", 0, "empty"))?;
    if version.parse::<f32>().is_err() {
        return Err(invalid("materials.dat", 0, format!("version line {version:?}")));
    }
    Ok(lines
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

/// What [`attach_map_bounds`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundsReport {
    /// Live ybn files, the `hi@`/`ma@` copies among them left out, and those whose box meets the region (read).
    pub ybn_live: usize,
    pub ybn_finer_left_out: usize,
    pub ybn_read: usize,
    pub material_names: usize,
}

/// The static map bounds whose box meets `region` (`[x0, y0, x1, y1]`, GTA ground coordinates), each as an entity
/// with [`GtaBounds`], and the material names as the [`GtaMaterials`] resource (see the module documentation). Every
/// live plain ybn is read for its box (in parallel); the game streams them by those boxes.
pub fn attach_map_bounds(
    w: &mut World,
    set: &ArchiveSet,
    keys: &Keys,
    region: [f32; 4],
) -> Result<BoundsReport> {
    let mut report = BoundsReport::default();
    let mut files: Vec<crate::archive_set::FileRef> = set
        .tables
        .get("ybn")
        .into_iter()
        .flat_map(|t| t.values().copied())
        .collect();
    report.ybn_live = files.len();
    files.retain(|f| !set.path(*f).rsplit('\\').next().unwrap_or("").contains('@'));
    report.ybn_finer_left_out = report.ybn_live - files.len();
    files.sort();
    let [x0, y0, x1, y1] = region;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let read = |f: crate::archive_set::FileRef| -> Result<Option<Bound>> {
        let a = &set.install.archives[f.archive];
        let r = a.resource(&mut File::open(&a.file)?, f.entry, keys)?;
        let bound = read_ybn(&r)?;
        let (lo, hi) = (bound.header.box_min, bound.header.box_max);
        let meets = hi[0] >= x0 && lo[0] <= x1 && hi[1] >= y0 && lo[1] <= y1;
        Ok(meets.then_some(bound))
    };
    let results: Vec<Result<Vec<(usize, Bound)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> Result<Vec<(usize, Bound)>> {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&f) = files.get(k) else { break };
                        if let Some(b) = read(f)? {
                            out.push((k, b));
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(invalid("ybn", 0, "a reading thread panicked")))
            })
            .collect()
    });
    let mut found = Vec::new();
    for r in results {
        found.extend(r?);
    }
    found.sort_by_key(|(k, _)| *k);
    for (k, bound) in found {
        report.ybn_read += 1;
        let e = w.spawn();
        w.insert(
            e,
            GtaBounds {
                path: set.path(files[k]).to_string(),
                bound,
            },
        );
    }
    if let Some(f) = set.live("dat", "materials") {
        let a = &set.install.archives[f.archive];
        let bytes = a.binary(&mut File::open(&a.file)?, f.entry, keys)?;
        let names = material_names(&String::from_utf8_lossy(&bytes))?;
        report.material_names = names.len();
        w.insert_resource(GtaMaterials(names));
    }
    Ok(report)
}

/// `lower-gta-collision` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LowerGtaCollision {
    /// `[x0, y0, x1, y1]`, GTA ground coordinates; `None`: every triangle.
    pub region: Option<[f32; 4]>,
}

impl LowerGtaCollision {
    pub fn from_options(o: &Options) -> std::result::Result<LowerGtaCollision, String> {
        let num = |k: &str| -> std::result::Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number")))
                .transpose()
        };
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let region = match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            [None, None, None, None] => None,
            _ => return Err("x0, y0, x1 and y1 go together".into()),
        };
        Ok(LowerGtaCollision { region })
    }
}

/// A soup in GTA's frame with GTA material indices, before the region and the frame.
#[derive(Default)]
struct Soup {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    materials: Vec<u8>,
}

/// `p` through a child transform (columns x, y, z, translation).
fn place(t: &[[f32; 3]; 4], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| {
        (f64::from(t[3][i]) + (0..3).map(|j| f64::from(t[j][i]) * f64::from(p[j])).sum::<f64>()) as f32
    })
}

const IDENTITY: [[f32; 3]; 4] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]];

fn compose(a: &[[f32; 3]; 4], b: &[[f32; 3]; 4]) -> [[f32; 3]; 4] {
    let lin = |v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| (0..3).map(|j| a[j][i] * v[j]).sum()) };
    [lin(b[0]), lin(b[1]), lin(b[2]), place(a, b[3])]
}

/// Unit sphere approximations: an octahedron subdivided once (18 vertices, 32 triangles), scaled per use.
fn sphere_mesh() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut v: Vec<[f32; 3]> = vec![
        [1., 0., 0.],
        [-1., 0., 0.],
        [0., 1., 0.],
        [0., -1., 0.],
        [0., 0., 1.],
        [0., 0., -1.],
    ];
    let faces = [
        [0, 2, 4],
        [2, 1, 4],
        [1, 3, 4],
        [3, 0, 4],
        [2, 0, 5],
        [1, 2, 5],
        [3, 1, 5],
        [0, 3, 5],
    ];
    let mut tris = Vec::new();
    let mut mid = std::collections::BTreeMap::new();
    let mut half = |a: u32, b: u32, v: &mut Vec<[f32; 3]>| -> u32 {
        *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
            let (p, q) = (v[a as usize], v[b as usize]);
            let m: [f32; 3] = std::array::from_fn(|i| (p[i] + q[i]) / 2.0);
            let n = m.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.push(m.map(|x| x / n));
            v.len() as u32 - 1
        })
    };
    for [a, b, c] in faces {
        let (ab, bc, ca) = (half(a, b, &mut v), half(b, c, &mut v), half(c, a, &mut v));
        tris.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
    }
    (v, tris)
}

impl Soup {
    fn add(&mut self, positions: &[[f32; 3]], triangles: &[[u32; 3]], material: u8) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(positions);
        self.triangles
            .extend(triangles.iter().map(|t| t.map(|i| i + base)));
        self.materials
            .extend(std::iter::repeat_n(material, triangles.len()));
    }

    /// A box polygon from its four stored vertices: its axis runs from the middle of the first two to the middle of
    /// the last two, and its half extents across it are (third - first - axis) / 2 and (fourth - first - axis) / 2.
    fn add_box(&mut self, c: [[f32; 3]; 4], material: u8) {
        let mid = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] { std::array::from_fn(|i| (a[i] + b[i]) / 2.0) };
        let (p5, p6) = (mid(c[0], c[1]), mid(c[2], c[3]));
        let a1: [f32; 3] = std::array::from_fn(|i| p6[i] - p5[i]);
        let a2: [f32; 3] = std::array::from_fn(|i| (c[2][i] - (c[0][i] + a1[i])) / 2.0);
        let a3: [f32; 3] = std::array::from_fn(|i| (c[3][i] - (c[0][i] + a1[i])) / 2.0);
        let corner = |end: [f32; 3], s: f32, t: f32| -> [f32; 3] {
            std::array::from_fn(|i| end[i] + s * a2[i] + t * a3[i])
        };
        self.add_cuboid(
            [
                corner(p5, -1., -1.),
                corner(p5, 1., -1.),
                corner(p5, 1., 1.),
                corner(p5, -1., 1.),
                corner(p6, -1., -1.),
                corner(p6, 1., -1.),
                corner(p6, 1., 1.),
                corner(p6, -1., 1.),
            ],
            material,
        );
    }

    /// An axis-aligned box.
    fn add_aabb(&mut self, lo: [f32; 3], hi: [f32; 3], t: &[[f32; 3]; 4], material: u8) {
        let c = |x: bool, y: bool, z: bool| {
            place(
                t,
                [
                    if x { hi[0] } else { lo[0] },
                    if y { hi[1] } else { lo[1] },
                    if z { hi[2] } else { lo[2] },
                ],
            )
        };
        self.add_cuboid(
            [
                c(false, false, false),
                c(true, false, false),
                c(true, true, false),
                c(false, true, false),
                c(false, false, true),
                c(true, false, true),
                c(true, true, true),
                c(false, true, true),
            ],
            material,
        );
    }

    /// Eight corners, the first four one face and the last four the opposite face in the same order: twelve
    /// triangles (outward when the faces wind counter-clockwise seen from outside the first).
    fn add_cuboid(&mut self, pos: [[f32; 3]; 8], material: u8) {
        let tris = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        self.add(&pos, &tris, material);
    }

    fn add_sphere(&mut self, centre: [f32; 3], radius: f32, material: u8) {
        let (v, t) = sphere_mesh();
        let pos: Vec<[f32; 3]> = v
            .iter()
            .map(|p| std::array::from_fn(|i| centre[i] + p[i] * radius))
            .collect();
        self.add(&pos, &t, material);
    }

    /// A capsule or cylinder between `a` and `b`: an 8-sided tube, closed by spheres (capsule) or caps (cylinder).
    fn add_tube(&mut self, a: [f32; 3], b: [f32; 3], radius: f32, capsule: bool, material: u8) {
        let axis: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let len = axis.iter().map(|x| x * x).sum::<f32>().sqrt();
        let n = if len > 0.0 {
            axis.map(|x| x / len)
        } else {
            [0., 0., 1.]
        };
        let helper = if n[0].abs() < 0.9 {
            [1., 0., 0.]
        } else {
            [0., 1., 0.]
        };
        let cross = |p: [f32; 3], q: [f32; 3]| {
            [
                p[1] * q[2] - p[2] * q[1],
                p[2] * q[0] - p[0] * q[2],
                p[0] * q[1] - p[1] * q[0],
            ]
        };
        let u = {
            let c = cross(n, helper);
            let l = c.iter().map(|x| x * x).sum::<f32>().sqrt();
            c.map(|x| x / l)
        };
        let w = cross(n, u);
        let mut pos = Vec::new();
        for end in [a, b] {
            for k in 0..8 {
                let t = k as f32 * std::f32::consts::TAU / 8.0;
                pos.push(std::array::from_fn(|i| {
                    end[i] + radius * (t.cos() * u[i] + t.sin() * w[i])
                }));
            }
        }
        let mut tris = Vec::new();
        for k in 0..8u32 {
            let k1 = (k + 1) % 8;
            tris.push([k, k1, 8 + k1]);
            tris.push([k, 8 + k1, 8 + k]);
        }
        if !capsule {
            pos.push(a);
            pos.push(b);
            for k in 0..8u32 {
                let k1 = (k + 1) % 8;
                tris.push([16, k1, k]);
                tris.push([17, 8 + k, 8 + k1]);
            }
        }
        self.add(&pos, &tris, material);
        if capsule {
            self.add_sphere(a, radius, material);
            self.add_sphere(b, radius, material);
        }
    }

    /// A bound placed by `t` (GTA frame), into this soup.
    fn bound(&mut self, b: &Bound, t: &[[f32; 3]; 4], log: &mut PassLog) {
        match &b.contents {
            Contents::Geometry(g) => {
                let v: Vec<[f32; 3]> = g.vertices.iter().map(|p| place(t, *p)).collect();
                let material = |i: usize| -> u8 {
                    let m = usize::from(g.polygon_materials.get(i).copied().unwrap_or(0));
                    g.materials.get(m).map_or(0, |m| (m[0] & 0xFF) as u8)
                };
                for (i, p) in g.polygons.iter().enumerate() {
                    let m = material(i);
                    match p {
                        Polygon::Triangle { vertices, .. } => {
                            let base = self.positions.len() as u32;
                            self.positions.extend(vertices.iter().map(|&k| v[usize::from(k)]));
                            self.triangles.push([base, base + 1, base + 2]);
                            self.materials.push(m);
                        }
                        Polygon::Box { corners } => {
                            log.approximate("box polygons as boxes from their corners", 1);
                            self.add_box(corners.map(|k| v[k as usize]), m);
                        }
                        Polygon::Sphere { centre, radius } => {
                            log.approximate("sphere polygons tessellated (32 triangles)", 1);
                            self.add_sphere(v[usize::from(*centre)], *radius, m);
                        }
                        Polygon::Capsule { ends, radius } => {
                            log.approximate("capsule polygons tessellated (8-sided)", 1);
                            self.add_tube(v[usize::from(ends[0])], v[usize::from(ends[1])], *radius, true, m);
                        }
                        Polygon::Cylinder { ends, radius } => {
                            log.approximate("cylinder polygons tessellated (8-sided)", 1);
                            self.add_tube(
                                v[usize::from(ends[0])],
                                v[usize::from(ends[1])],
                                *radius,
                                false,
                                m,
                            );
                        }
                    }
                }
            }
            Contents::Composite(children) => {
                for c in children {
                    // the earlier exporter's choice: no foliage or river volumes
                    let flags = c.flags[0];
                    if flags & (1 << 19) != 0 {
                        log.count("composite children flagged foliage, left out", 1);
                        continue;
                    }
                    if flags & (1 << 27) != 0 {
                        log.count("composite children flagged river, left out", 1);
                        continue;
                    }
                    self.bound(&c.bound, &compose(t, &c.transform), log);
                }
            }
            Contents::Primitive => {
                let h = &b.header;
                let m = h.material[0];
                match h.kind {
                    0 => {
                        log.approximate("sphere bounds tessellated (32 triangles)", 1);
                        self.add_sphere(place(t, h.sphere_centre), h.sphere_radius, m);
                    }
                    _ => {
                        log.approximate("box, capsule, disc and cylinder bounds as their boxes", 1);
                        self.add_aabb(h.box_min, h.box_max, t, m);
                    }
                }
            }
            Contents::Other => log.count("bounds of another kind (cloth) left out", 1),
        }
    }
}

impl Pass for LowerGtaCollision {
    fn name(&self) -> &'static str {
        "lower-gta-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if let Some([x0, y0, x1, y1]) = self.region {
            for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
                o.insert(k.to_string(), v.to_string());
            }
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &["core.collision", "core.transform"]
    }
    fn approximates(&self) -> &'static str {
        "boxes, spheres, capsules and cylinders tessellated; foliage and river volumes left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let names = w
            .resource::<GtaMaterials>()
            .map(|m| m.0.clone())
            .unwrap_or_default();
        let holders: Vec<Entity> = w.iter::<GtaBounds>().map(|(e, _)| e).collect();
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        for e in holders {
            let mut soup = Soup::default();
            soup.bound(&w.get::<GtaBounds>(e).expect("bounds").bound, &IDENTITY, log);
            let mut c = Collision::default();
            let mut kept = 0u64;
            for (t, m) in soup.triangles.iter().zip(&soup.materials) {
                let p = t.map(|i| soup.positions[i as usize]);
                if let Some([x0, y0, x1, y1]) = self.region {
                    let (cx, cy) = (
                        (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                        (p[0][1] + p[1][1] + p[2][1]) / 3.0,
                    );
                    if !(cx >= x0 && cx < x1 && cy >= y0 && cy < y1) {
                        continue;
                    }
                }
                if p.iter().flatten().any(|v| !v.is_finite()) {
                    log.count("collision triangles with values that are not finite, left out", 1);
                    continue;
                }
                let name = names
                    .get(usize::from(*m))
                    .map_or_else(|| format!("material {m}"), String::clone);
                let s = surfaces.index(&name).map_err(|e| e.to_string())?;
                let base = c.positions.len() as u32;
                c.positions.extend(p.iter().map(|q| GTA.point(*q)));
                c.triangles.push([base, base + 1, base + 2]);
                c.surfaces.push(s);
                kept += 1;
            }
            log.count("collision triangles", kept);
            if c.triangles.is_empty() {
                log.count("bounds with no triangle in the region", 1);
                continue;
            }
            w.insert(e, c);
            w.insert(e, Transform::IDENTITY);
        }
        if !surfaces.0.is_empty() {
            w.insert_resource(surfaces);
        }
        Ok(())
    }
}
