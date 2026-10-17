//! Collision: the reader's side, raw ([`attach_map_bounds`]: the ybn bounds of the region's map archives, with the
//! material names they index; [`GtaEmbeddedBounds`]: the bound a prototype's drawable or fragment embeds, attached
//! with its geometry), and `lower-gta-collision` and `lower-gta-embedded-collision`, which make them `core.collision`.
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
//!
//! `lower-gta-embedded-collision`: every prototype's [`GtaEmbeddedBounds`] becomes its `core.collision`, in its own
//! space (its placements place it), lowered as above (no region: its placements were selected). A placement whose
//! entity flags have bit 2 set (`4`: embedded collision disabled) draws without it: its prototype is copied once
//! without collision for the placements that disable it (counted). A placed prop (a bench, a rail, a pole) collides
//! by these bounds; the map's ybn bounds hold the static map's.
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Component, Entity, Instance, JsonError, Surfaces, Transform, World};

use crate::archive_set::ArchiveSet;
use crate::bounds::{Bound, Contents, Polygon, read_ybn};
use crate::components::GtaEntity;
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

/// The collision bound a prototype's asset embeds, as stored (GTA's frame, metres, the asset's own space): a
/// fragment's first physics LOD's, or a drawable's own; and the asset's file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaEmbeddedBounds {
    pub path: String,
    pub bound: Bound,
}

impl Component for GtaEmbeddedBounds {
    const NAME: &'static str = "gta.embedded_bounds";
    /// A summary: the file and the root bound's kind and box.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "path": self.path,
            "kind": self.bound.header.kind,
            "box": [self.bound.header.box_min, self.bound.header.box_max],
        })))
    }
}

/// Entity flag bit 2: the placement does not collide by its asset's embedded bound.
pub const EMBEDDED_COLLISION_DISABLED: u32 = 1 << 2;

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
        let r = set.install.resource(f, keys)?;
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
        let bytes = set.install.binary(f, keys)?;
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

    /// `local`'s triangles placed by `t`, into this soup.
    fn place_from(&mut self, local: &Soup, t: &[[f32; 3]; 4]) {
        let base = self.positions.len() as u32;
        self.positions
            .extend(local.positions.iter().map(|p| place(t, *p)));
        self.triangles
            .extend(local.triangles.iter().map(|tri| tri.map(|i| i + base)));
        self.materials.extend_from_slice(&local.materials);
    }

    /// A bound placed by `t` (GTA frame), into this soup. A leaf is built in its own space (its spheres, capsules and
    /// cylinders tessellated there) and its vertices placed by `t`, so a scaled or sheared transform shapes them as it
    /// does the triangles beside them.
    fn bound(&mut self, b: &Bound, t: &[[f32; 3]; 4], log: &mut PassLog) {
        match &b.contents {
            Contents::Geometry(g) => {
                let mut local = Soup::default();
                local.geometry(g, log);
                self.place_from(&local, t);
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
                let mut local = Soup::default();
                match h.kind {
                    0 => {
                        log.approximate("sphere bounds tessellated (32 triangles)", 1);
                        local.add_sphere(h.sphere_centre, h.sphere_radius, m);
                    }
                    _ => {
                        log.approximate("box, capsule, disc and cylinder bounds as their boxes", 1);
                        local.add_aabb(h.box_min, h.box_max, &IDENTITY, m);
                    }
                }
                self.place_from(&local, t);
            }
            Contents::Other => log.count("bounds of another kind (cloth) left out", 1),
        }
    }

    /// A geometry bound's polygons in its own space.
    fn geometry(&mut self, g: &crate::bounds::Geometry, log: &mut PassLog) {
        let v = &g.vertices;
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
}

impl Soup {
    /// The soup's triangles whose centre `keep` accepts (GTA frame), in the core frame, each made of its material's
    /// name in `names` (indexed into `surfaces`); triangles with values that are not finite left out (counted).
    fn collision(
        &self,
        names: &[String],
        surfaces: &mut Surfaces,
        keep: impl Fn([f32; 2]) -> bool,
        log: &mut PassLog,
    ) -> std::result::Result<Collision, String> {
        let mut c = Collision::default();
        for (t, m) in self.triangles.iter().zip(&self.materials) {
            let p = t.map(|i| self.positions[i as usize]);
            if !keep([
                (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            ]) {
                continue;
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
        }
        log.count("collision triangles", c.triangles.len() as u64);
        Ok(c)
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
        &["core.collision", "core.transform", "core.surfaces"]
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
        let inside = |[x, y]: [f32; 2]| {
            self.region
                .is_none_or(|[x0, y0, x1, y1]| x >= x0 && x < x1 && y >= y0 && y < y1)
        };
        for e in holders {
            let mut soup = Soup::default();
            soup.bound(&w.get::<GtaBounds>(e).expect("bounds").bound, &IDENTITY, log);
            let c = soup.collision(&names, &mut surfaces, inside, log)?;
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

/// `lower-gta-embedded-collision` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaEmbeddedCollision;

impl Pass for LowerGtaEmbeddedCollision {
    fn name(&self) -> &'static str {
        "lower-gta-embedded-collision"
    }
    // no `requires`: it runs on every loaded part and leaves one without embedded bounds as it is
    fn provides(&self) -> &[&'static str] {
        &["core.collision", "core.surfaces"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.instance"]
    }
    fn approximates(&self) -> &'static str {
        "boxes, spheres, capsules and cylinders tessellated; foliage and river volumes left out"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let holders: Vec<Entity> = w.iter::<GtaEmbeddedBounds>().map(|(e, _)| e).collect();
        if holders.is_empty() {
            return Ok(());
        }
        let names = w
            .resource::<GtaMaterials>()
            .map(|m| m.0.clone())
            .unwrap_or_default();
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        for e in holders {
            let mut soup = Soup::default();
            soup.bound(
                &w.get::<GtaEmbeddedBounds>(e).expect("bounds").bound,
                &IDENTITY,
                log,
            );
            let c = soup.collision(&names, &mut surfaces, |_| true, log)?;
            if c.triangles.is_empty() {
                log.count("embedded bounds with no triangle", 1);
                continue;
            }
            w.insert(e, c);
            log.count("prototypes colliding by their embedded bound", 1);
        }
        if !surfaces.0.is_empty() {
            w.insert_resource(surfaces);
        }
        // the placements that disable it use a copy of their prototype without it
        let disabled: Vec<(Entity, Entity)> = w
            .iter::<Instance>()
            .filter(|(e, i)| {
                w.has::<Collision>(i.prototype)
                    && w.has::<GtaEmbeddedBounds>(i.prototype)
                    && w.get::<GtaEntity>(*e)
                        .is_some_and(|g| g.flags & EMBEDDED_COLLISION_DISABLED != 0)
            })
            .map(|(e, i)| (e, i.prototype))
            .collect();
        let mut copies: BTreeMap<Entity, Entity> = BTreeMap::new();
        for (e, proto) in disabled {
            let copy = match copies.get(&proto) {
                Some(&c) => c,
                None => {
                    let c = crate::components::copy_prototype(w, proto);
                    w.remove::<Collision>(c);
                    w.remove::<GtaEmbeddedBounds>(c);
                    copies.insert(proto, c);
                    c
                }
            };
            w.get_mut::<Instance>(e).expect("instance").prototype = copy;
            log.count("placements with embedded collision disabled (entity flag 4)", 1);
        }
        log.count("prototypes copied without collision", copies.len() as u64);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::BoundHeader;
    use primase_core::Name;

    fn placement(w: &mut World, prototype: Entity, flags: u32) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(
            e,
            GtaEntity {
                ymap: "m.ymap".into(),
                index: 0,
                interior: None,
                lod_parent: None,
                archetype: 0,
                flags,
                guid: 0,
                parent_index: -1,
                lod_dist: 100.0,
                child_lod_dist: 0.0,
                lod_level: 0,
                num_children: 0,
                priority_level: 0,
                ambient_occlusion_multiplier: 255,
                artificial_ambient_occlusion: 255,
                tint_value: 0,
            },
        );
        e
    }

    /// A box bound from GTA's (0, 0, 0) to (1, 2, 3), of material 1.
    fn box_bound() -> Bound {
        Bound {
            header: BoundHeader {
                kind: 3,
                sphere_radius: 2.0,
                sphere_centre: [0.5, 1.0, 1.5],
                box_min: [0.0, 0.0, 0.0],
                box_max: [1.0, 2.0, 3.0],
                box_centre: [0.5, 1.0, 1.5],
                margin: 0.04,
                material: [1, 0, 0, 0],
                volume: 6.0,
            },
            contents: Contents::Primitive,
        }
    }

    /// A prop's box bound (GTA's (0, 0, 0) to (1, 2, 3), material 1) becomes its prototype's collision in the core
    /// frame, every triangle of material 1's name; the placement with entity flag 4 is given a copy of the prototype
    /// without it, the other keeps the prototype.
    #[test]
    fn a_prototype_collides_by_its_embedded_bound_unless_its_placement_disables_it() {
        let mut w = World::new();
        w.insert_resource(GtaMaterials(vec!["DEFAULT".into(), "TARMAC".into()]));
        let proto = w.spawn();
        w.insert(proto, Name("prop_bench".into()));
        w.insert(
            proto,
            GtaEmbeddedBounds {
                path: "prop_bench.ydr".into(),
                bound: box_bound(),
            },
        );
        let on = placement(&mut w, proto, 0);
        let off = placement(&mut w, proto, EMBEDDED_COLLISION_DISABLED | 1);
        let mut log = PassLog::default();
        LowerGtaEmbeddedCollision.run(&mut w, &mut log).unwrap();

        let c = w.get::<Collision>(proto).expect("collision");
        assert_eq!(c.triangles.len(), 12);
        // GTA (x, y, z) is core (x, z, -y)
        for axis in 0..3 {
            let lo = c.positions.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min);
            let hi = c
                .positions
                .iter()
                .map(|p| p[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!(
                [lo, hi],
                [[0.0, 1.0], [0.0, 3.0], [-2.0, 0.0]][axis],
                "axis {axis}"
            );
        }
        let tarmac = w
            .resource::<Surfaces>()
            .unwrap()
            .0
            .iter()
            .position(|s| s.name == "TARMAC");
        assert!(c.surfaces.iter().all(|&s| Some(usize::from(s)) == tarmac));
        assert!(!w.has::<Transform>(proto), "placed by its placements");

        assert_eq!(w.get::<Instance>(on).unwrap().prototype, proto);
        let copy = w.get::<Instance>(off).unwrap().prototype;
        assert_ne!(copy, proto);
        assert!(!w.has::<Collision>(copy) && !w.has::<GtaEmbeddedBounds>(copy));
        assert_eq!(w.get::<Name>(copy), Some(&Name("prop_bench".into())));
        assert_eq!(
            log.counts
                .get("placements with embedded collision disabled (entity flag 4)"),
            Some(&1)
        );
        assert_eq!(log.counts.get("prototypes copied without collision"), Some(&1));
    }

    /// The default passes classify the surfaces a loaded part's props bring: a prop of a material the map's surface
    /// table lacks (METAL_SOLID_SMALL, the map's only surface concrete, classified at selection) is metal after the
    /// default passes' collision and surface steps, in their order.
    #[test]
    fn surfaces_props_bring_are_classified_by_the_default_passes() {
        let steps: Vec<&str> = crate::pipeline::DEFAULT_PASSES
            .split(", ")
            .map(|p| p.split('{').next().unwrap_or(p))
            .filter(|n| ["lower-gta-embedded-collision", "lower-gta-surfaces"].contains(n))
            .collect();
        assert_eq!(steps, ["lower-gta-embedded-collision", "lower-gta-surfaces"]);
        let mut reg = primase_core::pass::PassRegistry::new();
        crate::register(&mut reg);
        let pipeline = reg.parse(&steps.join(", ")).unwrap();

        let mut w = World::new();
        w.insert_resource(GtaMaterials(vec!["DEFAULT".into(), "METAL_SOLID_SMALL".into()]));
        let mut concrete = primase_core::Surface::new("CONCRETE");
        concrete.class = Some(primase_core::SurfaceClass::Concrete);
        w.insert_resource(Surfaces(vec![concrete]));
        let proto = w.spawn();
        w.insert(
            proto,
            GtaEmbeddedBounds {
                path: "prop_bin.ydr".into(),
                bound: box_bound(),
            },
        );
        placement(&mut w, proto, 0);
        let mut log = PassLog::default();
        for p in &pipeline.passes {
            p.run(&mut w, &mut log).unwrap();
        }
        let s = &w.resource::<Surfaces>().unwrap().0;
        let metal = s
            .iter()
            .find(|s| s.name == "METAL_SOLID_SMALL")
            .expect("the prop's surface");
        assert_eq!(metal.class, Some(primase_core::SurfaceClass::Metal));
    }

    /// Worked by hand: a sphere polygon of radius 1 at its geometry's origin and a capsule of radius 0.5 along
    /// GTA y from -1 to 1, in a composite child scaled 2 along GTA x and moved 5 along it, reach x 3 to 7 (radius 1
    /// times 2) and, across, the radius itself: the sphere's tessellation spans GTA y and z -1 to 1; the capsule's x
    /// 4 to 6 (radius 0.5 times 2 around 5), y -1.5 to 1.5 (its ends plus its radius) and z -0.5 to 0.5.
    #[test]
    fn curved_colliders_take_their_childs_scale() {
        use crate::bounds::{Child, Geometry};
        let child = |polygons: Vec<Polygon>, vertices: Vec<[f32; 3]>| {
            let mut b = box_bound();
            b.header.kind = 4;
            b.contents = Contents::Geometry(Geometry {
                vertices,
                quantum: [1.0; 3],
                centre: [0.0; 3],
                polygon_materials: vec![0; polygons.len()],
                polygons,
                materials: vec![[1, 0]],
                vertex_colours: None,
            });
            b
        };
        let sphere = child(
            vec![Polygon::Sphere {
                centre: 0,
                radius: 1.0,
            }],
            vec![[0.0; 3]],
        );
        let capsule = child(
            vec![Polygon::Capsule {
                ends: [0, 1],
                radius: 0.5,
            }],
            vec![[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
        );
        let scaled = [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [5.0, 0.0, 0.0]];
        for (what, leaf, want) in [
            ("sphere", sphere, [[3.0, 7.0], [-1.0, 1.0], [-1.0, 1.0]]),
            ("capsule", capsule, [[4.0, 6.0], [-1.5, 1.5], [-0.5, 0.5]]),
        ] {
            let mut root = box_bound();
            root.header.kind = 10;
            root.contents = Contents::Composite(vec![Child {
                bound: Box::new(leaf),
                transform: scaled,
                flags: [0, 0],
                flags_2: [0, 0],
            }]);
            let mut soup = Soup::default();
            soup.bound(&root, &IDENTITY, &mut PassLog::default());
            for (axis, [lo, hi]) in want.iter().enumerate() {
                let got = [
                    soup.positions
                        .iter()
                        .map(|p| p[axis])
                        .fold(f32::INFINITY, f32::min),
                    soup.positions
                        .iter()
                        .map(|p| p[axis])
                        .fold(f32::NEG_INFINITY, f32::max),
                ];
                assert!(
                    (got[0] - lo).abs() < 1e-5 && (got[1] - hi).abs() < 1e-5,
                    "{what} axis {axis}: {got:?}, worked {:?}",
                    [lo, hi]
                );
            }
        }
    }
}
