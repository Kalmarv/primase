//! Collision: the reader's side, raw ([`attach_map_bounds`]: the ybn bounds of the region's map archives, with the
//! material names they index; [`GtaEmbeddedBounds`]: the bound a prototype's drawable or fragment embeds, attached
//! with its geometry), and `lower-gta-collision` and `lower-gta-embedded-collision`, which make them `core.collision`.
//!
//! Which bounds: the live ybn files whose box meets the region (every live one is read for its box, as the game
//! streams them by those boxes), each with its detail: the prefix of a file named `hi@...` or `ma@...` (the finer
//! copies the game keeps for weapons and vehicles), none for the plain bounds a body walks or rolls on.
//!
//! `gta-select-collision{detail}`: the map bounds of the details asked for kept (`plain`, `hi`, `ma`, `;` between;
//! `plain` unless given), the others despawned, counted per detail.
//!
//! `lower-gta-collision{x0,y0,x1,y1,foliage,river}`: every [`GtaBounds`] entity becomes an entity with an identity
//! `core.transform` and a `core.collision` triangle soup in the core frame: geometry triangles as stored, box polygons
//! and box bounds as their twelve triangles (exact, counted); approximated, each counted by what it became: spheres
//! (bounds and polygons) as 32-triangle spheres (an octahedron subdivided once), capsule polygons as 8-sided tubes with
//! such sphere ends, cylinder polygons as closed 8-sided tubes, and capsule, disc and cylinder bounds as their bounding
//! boxes; composite children placed by their transforms. Composite children flagged foliage (type flag bit 19: bushes
//! a body passes through) or river (bit 27: water volumes) are left out or kept as the required `foliage` and `river`
//! options say (`leave-out` or `keep`), counted either way. Each triangle's surface is its material's name in the
//! install's material table (`materials.dat`), in the world's `Surfaces`. Given a region, only triangles whose centre
//! lies in it are kept. The raw bound is removed once lowered, and an entity left with no triangle in the region goes
//! (counted).
//!
//! `lower-gta-embedded-collision{foliage,river}`: every prototype's [`GtaEmbeddedBounds`] becomes its
//! `core.collision`, in its own space (its placements place it), lowered as above (no region: its placements were
//! selected). A placement whose
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

/// One ybn's bound as stored (GTA's frame, metres, world space for map bounds), its file and its detail.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaBounds {
    pub path: String,
    /// The file name's prefix before `@` (`hi`, `ma`: the game's finer copies), lowercase; `None` for plain bounds.
    pub detail: Option<String>,
    pub bound: Bound,
}

/// A ybn file name's detail: what precedes an `@` in it, lowercase.
pub fn ybn_detail(file_name: &str) -> Option<String> {
    file_name.split_once('@').map(|(d, _)| d.to_ascii_lowercase())
}

impl Component for GtaBounds {
    const NAME: &'static str = "gta.bounds";
    /// 2: `detail` added.
    const VERSION: u32 = 2;
    /// Bulk data (a bound's every vertex and polygon): not shown.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
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
    /// Bulk data (a bound's every vertex and polygon): not shown.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
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
    /// Live ybn files, those whose box meets the region (read), and the `hi@`/`ma@` copies among those.
    pub ybn_live: usize,
    pub ybn_read: usize,
    pub ybn_finer_read: usize,
    pub material_names: usize,
}

/// The static map bounds whose box meets `region` (`[x0, y0, x1, y1]`, GTA ground coordinates), each as an entity
/// with [`GtaBounds`], and the material names as the [`GtaMaterials`] resource (see the module documentation). Every
/// live ybn is read for its box (in parallel); the game streams them by those boxes.
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
        let path = set.path(files[k]).to_string();
        let detail = ybn_detail(path.rsplit('\\').next().unwrap_or(&path));
        if detail.is_some() {
            report.ybn_finer_read += 1;
        }
        let e = w.spawn();
        w.insert(e, GtaBounds { path, detail, bound });
    }
    if let Some(f) = set.live("dat", "materials") {
        let bytes = set.install.binary(f, keys)?;
        let names = material_names(&String::from_utf8_lossy(&bytes))?;
        report.material_names = names.len();
        w.insert_resource(GtaMaterials(names));
    }
    Ok(report)
}

/// `gta-select-collision` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectCollision {
    /// The details kept: `None` for plain bounds, `Some("hi")`, `Some("ma")`.
    pub details: std::collections::BTreeSet<Option<String>>,
}

impl SelectCollision {
    pub fn from_options(o: &Options) -> std::result::Result<SelectCollision, String> {
        for k in o.keys() {
            if k != "detail" {
                return Err(format!("unknown option {k}"));
            }
        }
        let details = o
            .get("detail")
            .map_or("plain", String::as_str)
            .split(';')
            .map(|d| match d.trim() {
                "plain" => Ok(None),
                "" => Err("detail: empty".to_string()),
                d => Ok(Some(d.to_ascii_lowercase())),
            })
            .collect::<std::result::Result<_, _>>()?;
        Ok(SelectCollision { details })
    }
}

impl Pass for SelectCollision {
    fn name(&self) -> &'static str {
        "gta-select-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        let d: Vec<&str> = self
            .details
            .iter()
            .map(|d| d.as_deref().unwrap_or("plain"))
            .collect();
        o.insert("detail".to_string(), d.join(";"));
        o
    }
    /// The map bounds of the details not kept.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let mut drop = Vec::new();
        for (e, b) in w.iter::<GtaBounds>() {
            let name = b.detail.as_deref().unwrap_or("plain");
            if self.details.contains(&b.detail) {
                log.count(&format!("map bounds kept: {name}"), 1);
            } else {
                log.count(&format!("map bounds left out: {name}"), 1);
                drop.push(e);
            }
        }
        for e in drop {
            w.despawn(e);
        }
        Ok(())
    }
}

/// What a collision lowering does with the composite children carrying one flag (option values `keep`,
/// `leave-out`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flagged {
    Keep,
    LeaveOut,
}

impl Flagged {
    /// The option value.
    pub fn as_str(self) -> &'static str {
        match self {
            Flagged::Keep => "keep",
            Flagged::LeaveOut => "leave-out",
        }
    }

    /// Option `k`'s value in `o`, required.
    fn from_option(o: &Options, k: &str) -> std::result::Result<Flagged, String> {
        match o.get(k).map(String::as_str) {
            Some("keep") => Ok(Flagged::Keep),
            Some("leave-out") => Ok(Flagged::LeaveOut),
            Some(v) => Err(format!("{k}={v}: keep or leave-out")),
            None => Err(format!(
                "{k} is required (keep or leave-out): whether collision flagged {k} is dropped is the caller's choice"
            )),
        }
    }
}

/// Which flagged composite children the collision lowerings keep: their `foliage` and `river` options, both required
/// (whether collision is dropped is the caller's choice). Any value is valid: the fields are public.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlaggedVolumes {
    /// Children flagged foliage (type flag bit 19: bushes a body passes through).
    pub foliage: Flagged,
    /// Children flagged river (type flag bit 27: water volumes).
    pub river: Flagged,
}

impl FlaggedVolumes {
    /// Both kept.
    pub const KEEP: FlaggedVolumes = FlaggedVolumes {
        foliage: Flagged::Keep,
        river: Flagged::Keep,
    };
    /// Both left out.
    pub const LEAVE_OUT: FlaggedVolumes = FlaggedVolumes {
        foliage: Flagged::LeaveOut,
        river: Flagged::LeaveOut,
    };

    /// The `foliage` and `river` options of `o`.
    pub fn from_options(o: &Options) -> std::result::Result<FlaggedVolumes, String> {
        Ok(FlaggedVolumes {
            foliage: Flagged::from_option(o, "foliage")?,
            river: Flagged::from_option(o, "river")?,
        })
    }

    fn insert_options(self, o: &mut Options) {
        for (k, f) in [("foliage", self.foliage), ("river", self.river)] {
            o.insert(k.to_owned(), f.as_str().to_owned());
        }
    }
}

/// What both collision lowerings approximate.
const APPROXIMATES: &str = "spheres as 32-triangle spheres, capsule and cylinder polygons as 8-sided tubes, capsule, \
                            disc and cylinder bounds as their bounding boxes";

/// `lower-gta-collision` (see the module documentation): every triangle ([`LowerGtaCollision::new`]) or those of a
/// region ([`LowerGtaCollision::within`], checked).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerGtaCollision {
    /// `[x0, y0, x1, y1]`, GTA ground coordinates, finite with x0 < x1 and y0 < y1; `None`: every triangle.
    region: Option<[f32; 4]>,
    pub volumes: FlaggedVolumes,
}

impl LowerGtaCollision {
    /// Every triangle, the flagged volumes as `volumes` says.
    pub fn new(volumes: FlaggedVolumes) -> LowerGtaCollision {
        LowerGtaCollision {
            region: None,
            volumes,
        }
    }

    /// Only the triangles whose centre lies in `[x0, y0, x1, y1]` (GTA ground coordinates, half-open): an error unless
    /// every value is finite, x0 < x1 and y0 < y1.
    pub fn within(mut self, region: [f32; 4]) -> std::result::Result<LowerGtaCollision, String> {
        let [x0, y0, x1, y1] = region;
        if !region.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
            return Err(format!(
                "region {region:?}: x0, y0, x1, y1 finite, with x0 < x1 and y0 < y1"
            ));
        }
        self.region = Some(region);
        Ok(self)
    }

    /// The region, if any.
    pub fn region(&self) -> Option<[f32; 4]> {
        self.region
    }

    pub fn from_options(o: &Options) -> std::result::Result<LowerGtaCollision, String> {
        let num = |k: &str| -> std::result::Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number")))
                .transpose()
        };
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1", "foliage", "river"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let pass = LowerGtaCollision::new(FlaggedVolumes::from_options(o)?);
        match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => pass.within([a, b, c, d]),
            [None, None, None, None] => Ok(pass),
            _ => Err("x0, y0, x1 and y1 go together".into()),
        }
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

/// The determinant of a transform's linear part (columns x, y, z).
fn determinant(t: &[[f32; 3]; 4]) -> f64 {
    let c = |j: usize, i: usize| f64::from(t[j][i]);
    c(0, 0) * (c(1, 1) * c(2, 2) - c(2, 1) * c(1, 2)) - c(1, 0) * (c(0, 1) * c(2, 2) - c(2, 1) * c(0, 2))
        + c(2, 0) * (c(0, 1) * c(1, 2) - c(1, 1) * c(0, 2))
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

    /// `local`'s triangles placed by `t`, into this soup; through a mirror (a negative determinant) each triangle's
    /// order is reversed, so its front stays counter-clockwise.
    fn place_from(&mut self, local: &Soup, t: &[[f32; 3]; 4]) {
        let base = self.positions.len() as u32;
        self.positions
            .extend(local.positions.iter().map(|p| place(t, *p)));
        let mirrored = determinant(t) < 0.0;
        self.triangles.extend(local.triangles.iter().map(|tri| {
            let [a, b, c] = tri.map(|i| i + base);
            if mirrored { [a, c, b] } else { [a, b, c] }
        }));
        self.materials.extend_from_slice(&local.materials);
    }

    /// A bound placed by `t` (GTA frame), into this soup. A leaf is built in its own space (its spheres, capsules and
    /// cylinders tessellated there) and its vertices placed by `t`, so a scaled or sheared transform shapes them as it
    /// does the triangles beside them.
    fn bound(&mut self, b: &Bound, t: &[[f32; 3]; 4], volumes: FlaggedVolumes, log: &mut PassLog) {
        match &b.contents {
            Contents::Geometry(g) => {
                let mut local = Soup::default();
                local.geometry(g, log);
                self.place_from(&local, t);
            }
            Contents::Composite(children) => {
                for c in children {
                    // the caller's choice: foliage and river volumes kept or left out; a child is left out when
                    // a flag it has is (counted by the first), else counted kept by each flag it has
                    let flags = c.flags[0];
                    let flagged: Vec<(&str, Flagged)> =
                        [(19, "foliage", volumes.foliage), (27, "river", volumes.river)]
                            .into_iter()
                            .filter(|(bit, _, _)| flags & (1 << bit) != 0)
                            .map(|(_, what, f)| (what, f))
                            .collect();
                    if let Some((what, _)) = flagged.iter().find(|(_, f)| *f == Flagged::LeaveOut) {
                        log.count(&format!("composite children flagged {what}, left out"), 1);
                        continue;
                    }
                    for (what, _) in &flagged {
                        log.count(&format!("composite children flagged {what}, kept"), 1);
                    }
                    self.bound(&c.bound, &compose(t, &c.transform), volumes, log);
                }
            }
            Contents::Primitive => {
                let h = &b.header;
                let m = h.material[0];
                let mut local = Soup::default();
                match h.kind {
                    0 => {
                        log.approximate("sphere bounds as 32-triangle spheres", 1);
                        local.add_sphere(h.sphere_centre, h.sphere_radius, m);
                    }
                    3 => {
                        log.count("box bounds as their box", 1);
                        local.add_aabb(h.box_min, h.box_max, &IDENTITY, m);
                    }
                    _ => {
                        log.approximate("capsule, disc and cylinder bounds as their bounding boxes", 1);
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
                    log.count("box polygons as their box from its corners", 1);
                    self.add_box(corners.map(|k| v[k as usize]), m);
                }
                Polygon::Sphere { centre, radius } => {
                    log.approximate("sphere polygons as 32-triangle spheres", 1);
                    self.add_sphere(v[usize::from(*centre)], *radius, m);
                }
                Polygon::Capsule { ends, radius } => {
                    log.approximate(
                        "capsule polygons as 8-sided tubes with 32-triangle sphere ends",
                        1,
                    );
                    self.add_tube(v[usize::from(ends[0])], v[usize::from(ends[1])], *radius, true, m);
                }
                Polygon::Cylinder { ends, radius } => {
                    log.approximate("cylinder polygons as closed 8-sided tubes", 1);
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
                log.count("collision triangles outside the region, left out", 1);
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
        self.volumes.insert_options(&mut o);
        o
    }
    fn provides(&self) -> &[&'static str] {
        &["core.collision", "core.transform", "core.surfaces"]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.bounds"]
    }
    /// The bounds entities with no triangle in the region.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        APPROXIMATES
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
            let raw = w.remove::<GtaBounds>(e).expect("bounds");
            soup.bound(&raw.bound, &IDENTITY, self.volumes, log);
            let c = soup.collision(&names, &mut surfaces, inside, log)?;
            if c.triangles.is_empty() {
                log.count("bounds with no triangle in the region, left out", 1);
                w.despawn(e);
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerGtaEmbeddedCollision {
    pub volumes: FlaggedVolumes,
}

impl LowerGtaEmbeddedCollision {
    pub fn from_options(o: &Options) -> std::result::Result<LowerGtaEmbeddedCollision, String> {
        if let Some(k) = o.keys().find(|k| !["foliage", "river"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        Ok(LowerGtaEmbeddedCollision {
            volumes: FlaggedVolumes::from_options(o)?,
        })
    }
}

impl Pass for LowerGtaEmbeddedCollision {
    fn name(&self) -> &'static str {
        "lower-gta-embedded-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        self.volumes.insert_options(&mut o);
        o
    }
    // no `requires`: it runs on every loaded part and leaves one without embedded bounds as it is
    fn provides(&self) -> &[&'static str] {
        &["core.collision", "core.surfaces"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.instance"]
    }
    fn approximates(&self) -> &'static str {
        APPROXIMATES
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
                self.volumes,
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

    /// A sphere placed through a mirror (x to -x) and through no mirror: every triangle faces away from the centre
    /// (counter-clockwise seen from outside) both ways.
    #[test]
    fn triangles_placed_through_a_mirror_keep_their_front() {
        let mut sphere = box_bound();
        sphere.header.kind = 0;
        sphere.header.sphere_centre = [0.0; 3];
        sphere.header.sphere_radius = 1.0;
        for x in [1.0, -1.0] {
            let t = [[x, 0., 0.], [0., 1., 0.], [0., 0., 1.], [5., 0., 0.]];
            let mut soup = Soup::default();
            soup.bound(&sphere, &t, FlaggedVolumes::KEEP, &mut PassLog::default());
            assert_eq!(soup.triangles.len(), 32);
            for tri in &soup.triangles {
                let [a, b, c] = tri.map(|i| soup.positions[i as usize]);
                let (u, v): ([f32; 3], [f32; 3]) = (
                    std::array::from_fn(|i| b[i] - a[i]),
                    std::array::from_fn(|i| c[i] - a[i]),
                );
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let out: [f32; 3] = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0 - [5., 0., 0.][i]);
                assert!((0..3).map(|i| n[i] * out[i]).sum::<f32>() > 0.0, "x {x}: {tri:?}");
            }
        }
    }

    /// A map bound inside the region becomes collision and loses its raw bound; one with no triangle in the region
    /// goes, counted.
    #[test]
    fn map_bounds_are_lowered_and_their_raw_bound_removed() {
        let mut w = World::new();
        w.insert_resource(GtaMaterials(vec!["DEFAULT".into(), "TARMAC".into()]));
        let raw = |w: &mut World| {
            let e = w.spawn();
            w.insert(
                e,
                GtaBounds {
                    path: "m.ybn".into(),
                    detail: None,
                    bound: box_bound(),
                },
            );
            e
        };
        let (inside, outside) = (raw(&mut w), raw(&mut w));
        // the second is moved out of the region: its box's GTA x from 100
        if let Some(b) = w.get_mut::<GtaBounds>(outside) {
            b.bound.header.box_min[0] = 100.0;
            b.bound.header.box_max[0] = 101.0;
        }
        let mut log = PassLog::default();
        LowerGtaCollision::new(FlaggedVolumes::LEAVE_OUT)
            .within([-10.0, -10.0, 10.0, 10.0])
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(w.get::<Collision>(inside).map(|c| c.triangles.len()), Some(12));
        assert!(w.get::<GtaBounds>(inside).is_none());
        assert!(!w.is_alive(outside));
        assert_eq!(
            log.counts.get("bounds with no triangle in the region, left out"),
            Some(&1)
        );
        assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    }

    /// A composite of a box, a sphere, a capsule bound, a child flagged foliage and one flagged river (each a box):
    /// the box is exact and counted, the sphere and the capsule approximated by what they became; the flagged
    /// children are kept or left out as the options say, counted either way. Both options are required.
    #[test]
    fn flagged_volumes_are_the_callers_choice_and_reports_name_what_was_approximated() {
        use crate::bounds::Child;
        let child = |kind: u8, flags: u32| {
            let mut b = box_bound();
            b.header.kind = kind;
            Child {
                bound: Box::new(b),
                transform: IDENTITY,
                flags: [flags, 0],
                flags_2: [0, 0],
            }
        };
        let mut root = box_bound();
        root.header.kind = 10;
        root.contents = Contents::Composite(vec![
            child(3, 0),
            child(0, 0),
            child(1, 0),
            child(3, 1 << 19),
            child(3, 1 << 27),
            child(3, 1 << 19 | 1 << 27),
        ]);
        // kept: the flagged boxes kept (the one flagged both only when both are kept)
        for (options, volumes, kept) in [
            ("foliage=leave-out,river=leave-out", FlaggedVolumes::LEAVE_OUT, 0),
            ("foliage=keep,river=keep", FlaggedVolumes::KEEP, 3),
            (
                "foliage=keep,river=leave-out",
                FlaggedVolumes {
                    foliage: Flagged::Keep,
                    river: Flagged::LeaveOut,
                },
                1,
            ),
        ] {
            let o: Options = options
                .split(',')
                .map(|kv| {
                    let (k, v) = kv.split_once('=').unwrap();
                    (k.to_owned(), v.to_owned())
                })
                .collect();
            assert_eq!(FlaggedVolumes::from_options(&o), Ok(volumes));
            assert_eq!(
                LowerGtaEmbeddedCollision::from_options(&o).map(|p| p.options()),
                Ok(o.clone())
            );
            let mut soup = Soup::default();
            let mut log = PassLog::default();
            soup.bound(&root, &IDENTITY, volumes, &mut log);
            // box 12, sphere 32, capsule as its box 12, each kept flagged box 12
            assert_eq!(soup.triangles.len(), 12 + 32 + 12 + 12 * kept, "{options}");
            assert_eq!(
                log.counts.get("box bounds as their box"),
                Some(&(1 + kept as u64))
            );
            assert_eq!(
                log.approximations.get("sphere bounds as 32-triangle spheres"),
                Some(&1)
            );
            assert_eq!(
                log.approximations
                    .get("capsule, disc and cylinder bounds as their bounding boxes"),
                Some(&1)
            );
            assert_eq!(log.approximations.len(), 2, "{:?}", log.approximations);
            // the child flagged both: left out by foliage first, or counted kept by both
            let (foliage, river) = match (volumes.foliage, volumes.river) {
                (Flagged::LeaveOut, _) => (("left out", 2), ("left out", 1)),
                (Flagged::Keep, Flagged::LeaveOut) => (("kept", 1), ("left out", 2)),
                (Flagged::Keep, Flagged::Keep) => (("kept", 2), ("kept", 2)),
            };
            for (what, (done, n)) in [("foliage", foliage), ("river", river)] {
                assert_eq!(
                    log.counts
                        .get(&format!("composite children flagged {what}, {done}")),
                    Some(&n),
                    "{options} {what}"
                );
            }
        }
        // the struct and its options are one: made from options, printed back as the same options
        let o: Options = [
            ("x0", "-1"),
            ("y0", "-2"),
            ("x1", "3"),
            ("y1", "4"),
            ("foliage", "keep"),
            ("river", "leave-out"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let made = LowerGtaCollision::from_options(&o).unwrap();
        assert_eq!(made.options(), o);
        assert_eq!(made.region(), Some([-1.0, -2.0, 3.0, 4.0]));
        // a region the constructor refuses, from options or not
        for bad in [
            [1.0, 0.0, 1.0, 2.0],
            [0.0, 3.0, 1.0, 2.0],
            [f32::NAN, 0.0, 1.0, 1.0],
        ] {
            assert!(
                LowerGtaCollision::new(FlaggedVolumes::KEEP).within(bad).is_err(),
                "{bad:?}"
            );
        }
        let mut reversed = o.clone();
        reversed.insert("x1".into(), "-5".into());
        assert!(LowerGtaCollision::from_options(&reversed).is_err());
        for missing in ["foliage=keep", "river=keep", ""] {
            let o: Options = missing
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .into_iter()
                .collect();
            assert!(LowerGtaCollision::from_options(&o).is_err(), "{missing:?}");
            assert!(
                LowerGtaEmbeddedCollision::from_options(&o).is_err(),
                "{missing:?}"
            );
        }
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
        LowerGtaEmbeddedCollision {
            volumes: FlaggedVolumes::LEAVE_OUT,
        }
        .run(&mut w, &mut log)
        .unwrap();

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

    /// The surfaces a loaded part's props bring are classified when `lower-gta-surfaces` runs after
    /// `lower-gta-embedded-collision`: a prop of a material the map's surface table lacks (METAL_SOLID_SMALL, the
    /// map's only surface concrete, classified at selection) is metal after them.
    #[test]
    fn surfaces_props_bring_are_classified_after_their_collision() {
        let mut reg = primase_core::pass::PassRegistry::new();
        crate::register(&mut reg);
        let pipeline = reg
            .parse("lower-gta-embedded-collision{foliage=leave-out,river=leave-out}, lower-gta-surfaces")
            .unwrap();

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
            soup.bound(&root, &IDENTITY, FlaggedVolumes::KEEP, &mut PassLog::default());
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

    /// A ybn's detail is its name's prefix before `@`; `gta-select-collision` keeps plain bounds unless asked for
    /// others, and the details named.
    #[test]
    fn map_bounds_are_chosen_by_detail_in_a_pass() {
        assert_eq!(ybn_detail("hi@cs1_08_3.ybn"), Some("hi".into()));
        assert_eq!(ybn_detail("MA@x.ybn"), Some("ma".into()));
        assert_eq!(ybn_detail("cs1_08_3.ybn"), None);
        let mut w = World::new();
        let mut spawn = |detail: Option<&str>| {
            let e = w.spawn();
            w.insert(
                e,
                GtaBounds {
                    path: "x.ybn".into(),
                    detail: detail.map(str::to_string),
                    bound: box_bound(),
                },
            );
            e
        };
        let plain = spawn(None);
        let hi = spawn(Some("hi"));
        let ma = spawn(Some("ma"));
        let mut log = PassLog::default();
        SelectCollision::from_options(&Options::new())
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        let alive: Vec<Entity> = w.iter::<GtaBounds>().map(|(e, _)| e).collect();
        assert_eq!(alive, vec![plain]);
        assert_eq!(log.counts.get("map bounds left out: hi"), Some(&1));
        let _ = (hi, ma);

        let mut w2 = World::new();
        for d in [None, Some("hi"), Some("ma")] {
            let e = w2.spawn();
            w2.insert(
                e,
                GtaBounds {
                    path: "x.ybn".into(),
                    detail: d.map(str::to_string),
                    bound: box_bound(),
                },
            );
        }
        let mut o = Options::new();
        o.insert("detail".into(), "plain;hi".into());
        SelectCollision::from_options(&o)
            .unwrap()
            .run(&mut w2, &mut PassLog::default())
            .unwrap();
        let kept: Vec<Option<String>> = w2.iter::<GtaBounds>().map(|(_, b)| b.detail.clone()).collect();
        assert_eq!(kept, vec![None, Some("hi".into())]);
    }
}
