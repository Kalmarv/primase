//! Object collision as the objects store it, and the pass that places it. The reader keeps each object prototype's
//! main HKX bodies (`fromsoft.object_collision`): meshes as their posed triangles, convex shapes as their stored core
//! and radius. How convex shapes become triangles, which motion types and HitMtrl classes become collision, the
//! surface of convex bodies (which store none) and the crop are `lower-fromsoft-object-collision`'s options.
use super::passes::collision::{attach, beyond, crop, owned, push, surface_of};
use super::settings::{HitClass, Metres, joined};
use super::source::{MaterialSlots, ObjectBodies, ObjectBody, ObjectGeometry};
use crate::collision::{BodyGeometry, pose_columns, posed};
use crate::{Result, collision, materials::Material, rigid::SourceModel};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::state::Condition;
use primase_core::{
    Collision, Component, Entity, Instance, Name, Surfaces, Transform, World,
    frame::{Axis, SourceFrame},
    world::Resource,
};
use primase_meshops::vector::Vec3;
use std::collections::{BTreeMap, BTreeSet};
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// What the reader found of the kept object prototypes' collision.
#[derive(Debug, Default)]
pub struct Report {
    /// Object prototypes kept, and those whose binder holds their main HKX.
    pub models: usize,
    pub models_with_collision: usize,
    /// Their bodies by stored motion type, and by shape class.
    pub bodies_by_motion: BTreeMap<u64, usize>,
    pub bodies_by_shape: BTreeMap<String, usize>,
}

/// An object's main HKX `file` (decoded `bytes`) as its prototype's `fromsoft.object_collision`: every body as
/// stored, in the prototype's space.
pub fn bodies(file: &str, bytes: &[u8], report: &mut Report) -> Result<ObjectBodies> {
    let bodies = collision::read_bodies(bytes, None)?
        .into_iter()
        .map(|body| {
            *report.bodies_by_motion.entry(body.motion_type).or_default() += 1;
            *report.bodies_by_shape.entry(body.shape.clone()).or_default() += 1;
            let geometry = match body.geometry {
                BodyGeometry::Mesh(soup) => ObjectGeometry::Mesh {
                    triangles: soup
                        .triangles
                        .into_iter()
                        .map(|t| t.map(|v| FRAME.point(v)))
                        .collect(),
                    materials: soup.materials,
                },
                BodyGeometry::Convex {
                    core,
                    capsule,
                    position,
                    orientation,
                } => ObjectGeometry::Convex {
                    core,
                    capsule,
                    // the body's pose, then the file frame's reflection into the world frame's axes
                    to_prototype: pose_columns(position, orientation).map(|c| FRAME.point(c)),
                },
            };
            ObjectBody {
                name: body.name,
                motion_type: body.motion_type,
                shape: body.shape,
                geometry,
            }
        })
        .collect();
    report.models_with_collision += 1;
    Ok(ObjectBodies {
        file: file.to_owned(),
        bodies,
    })
}

/// The surface of the largest colliding render area of a prototype's model: the first level-0 face set of each mesh
/// whose material is drawn as opaque or cutout (not a skipped class, not a blocker card), its triangles posed as the
/// rigid lowering poses them, degenerate ones (doubled area squared at most 1e-14) not counted; `default` when that
/// surface is not one a HitMtrl class stands for.
fn largest_render_surface(world: &World, prototype: Entity) -> std::result::Result<&'static str, String> {
    let (Some(SourceModel(model)), Some(MaterialSlots(slots))) = (
        world.get::<SourceModel>(prototype),
        world.get::<MaterialSlots>(prototype),
    ) else {
        return Err(format!(
            "{prototype:?}: convex-surface=render needs the prototype's fromsoft.flver and fromsoft.material_slots"
        ));
    };
    let posed = crate::rigid::posed_positions(model).map_err(|e| format!("{prototype:?}: {e}"))?;
    let mut areas = BTreeMap::<String, f64>::new();
    for (mesh, positions) in model.meshes.iter().zip(&posed) {
        let Some(m) = slots.get(mesh.material).and_then(|e| world.get::<Material>(*e)) else {
            return Err(format!("{prototype:?}: material slot {}", mesh.material));
        };
        if m.skip.is_some()
            || super::blockers::is_card(m)
            || !["opaque", "cutout"].contains(&m.class.as_str())
        {
            continue;
        }
        let Some(face_set) = mesh.faces.iter().find(|f| f.is_lod0()) else {
            continue;
        };
        let mut indices = face_set.triangulate();
        indices.truncate(indices.len() / 3 * 3);
        for tri in indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| Vec3::from(FRAME.point(positions[i as usize])));
            let cross = (b - a).cross(c - a);
            if cross.length_squared() > 1e-14 {
                *areas.entry(m.surface.clone()).or_default() += f64::from(cross.length()) * 0.5;
            }
        }
    }
    let dominant = areas
        .iter()
        .min_by(|(ka, a), (kb, b)| b.total_cmp(a).then(ka.cmp(kb)))
        .map(|(k, _)| k.as_str());
    Ok([
        "concrete", "earth", "wood", "grass", "gravel", "metal", "sand", "roofing", "marble",
    ]
    .into_iter()
    .find(|s| Some(*s) == dominant)
    .unwrap_or("default"))
}

/// A body's triangles in its prototype's space and their HitMtrl IDs.
type BodyTriangles = (Vec<[[f32; 3]; 3]>, Vec<i32>);

/// The surface `lower-fromsoft-object-collision` gives convex bodies, which store no material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConvexSurface {
    /// `default`, as any triangle without a HitMtrl.
    #[default]
    Default,
    /// The surface of the prototype's largest colliding render area, an inference from its drawn geometry.
    Render,
}

/// What `lower-fromsoft-object-collision` makes of convex bodies (box, polytope, capsule), which a triangle soup
/// cannot hold as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvexBodies {
    /// Triangles of the intersection of the core's planes offset by the radius: sharp where the shape is rounded, a
    /// capsule a box around its segment. Counted as approximations.
    Tessellate,
    /// Not lowered, counted.
    LeaveOut,
}

impl ConvexBodies {
    fn word(self) -> &'static str {
        match self {
            ConvexBodies::Tessellate => "tessellate",
            ConvexBodies::LeaveOut => "leave-out",
        }
    }
}

/// `lower-fromsoft-object-collision{convex=tessellate|leave-out}`: every placed object prototype's bodies
/// (`fromsoft.object_collision`) through each placement's transform, at their stored pose whatever their motion type
/// (not interpreted), as one `objects` soup, a placement present only in some world states into an
/// `objects_state<n>` soup with its condition; the bodies of the placed prototypes consumed, unplaced ones' kept.
/// Each triangle gets its HitMtrl class's surface (`default` without one, counted). In a part of a conversion
/// (`core.part_scope`), only the placements its roots reach are lowered: a context placement's collision is its own
/// part's.
///
/// Options: `convex=` ([`ConvexBodies`], required: a soup cannot hold a rounded shape, so the program chooses its
/// approximation); `leave-out=` HitMtrl classes (an ID's base value, modulo 100) not lowered, counted;
/// `leave-out-motion=` stored motion types whose bodies are not lowered, counted; `convex-surface=default|render`
/// ([`ConvexSurface`]); `reach=N` leaves out, counted, the triangles whose centre lies more than N metres outside the
/// read region (`fromsoft.read_scope`).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftObjectCollision {
    /// What convex bodies become.
    convex: ConvexBodies,
    /// HitMtrl classes whose triangles are not lowered.
    leave_out: BTreeSet<HitClass>,
    /// Stored motion types whose bodies are not lowered.
    leave_out_motion: BTreeSet<u64>,
    /// The surface of convex bodies.
    convex_surface: ConvexSurface,
    /// The crop around the read region, if any.
    reach: Option<Metres>,
}

impl LowerFromSoftObjectCollision {
    /// Every body lowered whole at its stored pose, convex ones as `convex` says, with their stored materials.
    pub fn new(convex: ConvexBodies) -> Self {
        LowerFromSoftObjectCollision {
            convex,
            leave_out: BTreeSet::new(),
            leave_out_motion: BTreeSet::new(),
            convex_surface: ConvexSurface::Default,
            reach: None,
        }
    }

    /// The HitMtrl classes whose triangles are not lowered.
    pub fn with_leave_out(mut self, leave_out: BTreeSet<HitClass>) -> Self {
        self.leave_out = leave_out;
        self
    }
    /// The stored motion types whose bodies are not lowered.
    pub fn with_leave_out_motion(mut self, leave_out_motion: BTreeSet<u64>) -> Self {
        self.leave_out_motion = leave_out_motion;
        self
    }
    pub fn with_convex_surface(mut self, convex_surface: ConvexSurface) -> Self {
        self.convex_surface = convex_surface;
        self
    }
    /// The triangles whose centre lies more than `reach` outside the read region left out.
    pub fn with_reach(mut self, reach: Metres) -> Self {
        self.reach = Some(reach);
        self
    }
    pub fn convex(&self) -> ConvexBodies {
        self.convex
    }
    pub fn leave_out(&self) -> &BTreeSet<HitClass> {
        &self.leave_out
    }
    pub fn leave_out_motion(&self) -> &BTreeSet<u64> {
        &self.leave_out_motion
    }
    pub fn convex_surface(&self) -> ConvexSurface {
        self.convex_surface
    }
    pub fn reach(&self) -> Option<Metres> {
        self.reach
    }

    /// From its options, through [`LowerFromSoftObjectCollision::new`] and its setters.
    pub fn from_options(o: &Options) -> std::result::Result<Self, String> {
        let convex = match o.get("convex").map(String::as_str) {
            Some("tessellate") => ConvexBodies::Tessellate,
            Some("leave-out") => ConvexBodies::LeaveOut,
            Some(v) => return Err(format!("convex={v} is not tessellate or leave-out")),
            None => {
                return Err(
                    "convex=tessellate|leave-out is required: a soup cannot hold a rounded shape as stored"
                        .into(),
                );
            }
        };
        let mut pass = Self::new(convex);
        for (k, v) in o {
            pass = match (k.as_str(), v.as_str()) {
                ("convex", _) => pass,
                ("leave-out", v) => pass.with_leave_out(HitClass::parse_set(k, v)?),
                ("leave-out-motion", v) => pass.with_leave_out_motion(
                    v.split('+')
                        .filter(|m| !m.is_empty())
                        .map(|m| {
                            m.parse::<u64>()
                                .map_err(|_| format!("leave-out-motion: {m} is not a motion type"))
                        })
                        .collect::<std::result::Result<_, _>>()?,
                ),
                ("convex-surface", "default") => pass.with_convex_surface(ConvexSurface::Default),
                ("convex-surface", "render") => pass.with_convex_surface(ConvexSurface::Render),
                ("convex-surface", v) => return Err(format!("convex-surface={v} is not default or render")),
                ("reach", v) => pass.with_reach(Metres::parse(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            };
        }
        Ok(pass)
    }

    /// A body's triangles in its prototype's space and their HitMtrl IDs, in the stored-order convention (facing
    /// away: lowering reverses); `None` for a convex body left out.
    fn triangles(
        &self,
        body: &ObjectBody,
        log: &mut PassLog,
    ) -> std::result::Result<Option<BodyTriangles>, String> {
        match &body.geometry {
            ObjectGeometry::Mesh { triangles, materials } => Ok(Some((triangles.clone(), materials.clone()))),
            ObjectGeometry::Convex { .. } if self.convex == ConvexBodies::LeaveOut => {
                log.count("convex bodies left out", 1);
                Ok(None)
            }
            ObjectGeometry::Convex {
                core,
                capsule,
                to_prototype,
            } => {
                let c = to_prototype;
                let det = c[0][0] * (c[1][1] * c[2][2] - c[2][1] * c[1][2])
                    - c[1][0] * (c[0][1] * c[2][2] - c[2][1] * c[0][2])
                    + c[2][0] * (c[0][1] * c[1][2] - c[1][1] * c[0][2]);
                let mut triangles: Vec<[[f32; 3]; 3]> = core
                    .expanded_triangles()
                    .map_err(|e| format!("body {}: {e}", body.name))?
                    .into_iter()
                    .map(|t| t.map(|v| posed(c, v)))
                    .collect();
                // the stored-order convention is the reflected frame's: a map that does not reflect turns them
                if det > 0. {
                    triangles.iter_mut().for_each(|t| t.swap(1, 2));
                }
                log.approximate(
                    "convex bodies tessellated as their offset planes' intersection, sharp where rounded",
                    1,
                );
                if capsule.is_some() {
                    log.approximate("capsules tessellated as boxes around their segment", 1);
                }
                let n = triangles.len();
                Ok(Some((triangles, vec![-1; n])))
            }
        }
    }
}

impl Pass for LowerFromSoftObjectCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-object-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([("convex".to_owned(), self.convex.word().to_owned())]);
        if !self.leave_out.is_empty() {
            o.insert("leave-out".into(), joined(&self.leave_out));
        }
        if !self.leave_out_motion.is_empty() {
            o.insert("leave-out-motion".into(), joined(&self.leave_out_motion));
        }
        if self.convex_surface == ConvexSurface::Render {
            o.insert("convex-surface".into(), "render".into());
        }
        if let Some(r) = self.reach {
            o.insert("reach".into(), r.to_string());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[ObjectBodies::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Collision::NAME,
            super::source::CollisionSource::NAME,
            Name::NAME,
            Condition::NAME,
            Transform::NAME,
            Surfaces::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[ObjectBodies::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn approximates(&self) -> &'static str {
        "bodies at their stored pose, their motion type not interpreted; with convex=tessellate, convex bodies as their offset planes' intersection (capsules as boxes); triangles without a HitMtrl, and classes without a known surface, as the default surface; with convex-surface=render, convex bodies' surface inferred from the render geometry"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let crop = crop(world, self.reach)?;
        let owned = owned(world);
        let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
        let mut soup = Collision::default();
        // collision of objects present only in some world states, by the placement's condition
        let mut conditioned: BTreeMap<String, (Condition, Collision)> = BTreeMap::new();
        let mut convex = BTreeMap::<Entity, &'static str>::new();
        let mut lowered = BTreeSet::new();
        for (e, instance) in world.iter::<Instance>() {
            let Some(bodies) = world.get::<ObjectBodies>(instance.prototype) else {
                continue;
            };
            if owned.as_ref().is_some_and(|o| !o.contains(&e)) {
                log.count("context placements, their collision left to their own part", 1);
                continue;
            }
            let transform = world
                .get::<Transform>(e)
                .ok_or_else(|| format!("{e:?}: placement without a transform"))?;
            let soup = match world.get::<Condition>(e) {
                None => &mut soup,
                Some(c) => {
                    &mut conditioned
                        .entry(super::passes::condition_key(c))
                        .or_insert_with(|| (c.clone(), Collision::default()))
                        .1
                }
            };
            for body in &bodies.bodies {
                if self.leave_out_motion.contains(&body.motion_type) {
                    log.count(&format!("bodies of motion type {} left out", body.motion_type), 1);
                    continue;
                }
                let Some((triangles, materials)) = self.triangles(body, log)? else {
                    continue;
                };
                log.approximate(
                    "bodies placed at their stored pose, their motion type not interpreted",
                    1,
                );
                for (tri, material) in triangles.into_iter().zip(materials) {
                    let v = tri.map(|p| crate::pose::point(transform, p));
                    if crop.is_some_and(|c| beyond(&v, c)) {
                        log.count("triangles beyond the reach around the read region, left out", 1);
                        continue;
                    }
                    if HitClass::of(material).is_some_and(|c| self.leave_out.contains(&c)) {
                        log.count("triangles of left-out HitMtrl classes", 1);
                        continue;
                    }
                    let surface = if material < 0
                        && body.geometry.convex()
                        && self.convex_surface == ConvexSurface::Render
                    {
                        let surface = match convex.get(&instance.prototype) {
                            Some(s) => *s,
                            None => {
                                let s = largest_render_surface(world, instance.prototype)?;
                                convex.insert(instance.prototype, s);
                                s
                            }
                        };
                        log.approximate("convex body triangles given the largest render surface", 1);
                        surface
                    } else {
                        surface_of(material, log)
                    };
                    push(soup, v, surfaces.index(surface).map_err(|e| e.to_string())?)?;
                    log.count("object collision triangles", 1);
                }
            }
            log.count("placements with collision", 1);
            lowered.insert(instance.prototype);
        }
        for p in lowered {
            world.remove::<ObjectBodies>(p);
            log.count("object prototypes whose collision was placed", 1);
        }
        world.insert_resource(surfaces);
        attach(world, soup, "objects", false, None, log);
        for (n, (condition, soup)) in conditioned.into_values().enumerate() {
            log.count("soups present only in some world states", 1);
            attach(
                world,
                soup,
                &format!("objects_state{n}"),
                false,
                Some(&condition),
                log,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_havok::ConvexCore;

    fn mesh(name: &str, motion_type: u64, materials: Vec<i32>) -> ObjectBody {
        ObjectBody {
            name: name.into(),
            motion_type,
            shape: "fsnpCustomParamCompressedMeshShape".into(),
            geometry: ObjectGeometry::Mesh {
                triangles: vec![[[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]; materials.len()],
                materials,
            },
        }
    }

    /// A unit cube's stored core (planes `n.x + d <= 0` inside), with `radius`.
    fn cube(radius: f32) -> ConvexCore {
        let vertices = (0..8)
            .map(|i| [0, 1, 2].map(|k| if i & (1 << k) != 0 { 0.5 } else { -0.5 }))
            .collect();
        let mut planes = Vec::new();
        let mut faces = Vec::new();
        for k in 0..3 {
            for sign in [-1., 1.] {
                let mut n = [0., 0., 0., -0.5];
                n[k] = sign;
                planes.push(n);
                let side = usize::from(sign > 0.);
                let face: Vec<usize> = (0..8).filter(|i| (i >> k) & 1 == side).collect();
                faces.push(face);
            }
        }
        ConvexCore {
            vertices,
            planes,
            faces,
            radius,
        }
    }

    fn convex(name: &str, shape: &str, core: ConvexCore, capsule: Option<[[f32; 3]; 2]>) -> ObjectBody {
        ObjectBody {
            name: name.into(),
            motion_type: 0,
            shape: shape.into(),
            geometry: ObjectGeometry::Convex {
                core,
                capsule,
                // the file frame's reflection, unposed
                to_prototype: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.], [0.; 3]],
            },
        }
    }

    /// A placed prototype with a static mesh body, a motion-type-2 mesh body, a rounded box and a capsule; an unplaced
    /// one.
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let placed = w.spawn();
        w.insert(
            placed,
            ObjectBodies {
                file: "o000001.hkx".into(),
                bodies: vec![
                    mesh("static", 0, vec![104, 29]),
                    mesh("moving", 2, vec![111]),
                    convex("box", "hknpBoxShape", cube(0.05), None),
                    convex(
                        "capsule",
                        "hknpCapsuleShape",
                        cube(0.25),
                        Some([[0., -0.5, 0.], [0., 0.5, 0.]]),
                    ),
                ],
            },
        );
        let unplaced = w.spawn();
        w.insert(
            unplaced,
            ObjectBodies {
                file: "o000002.hkx".into(),
                bodies: vec![convex("box", "hknpBoxShape", cube(0.), None)],
            },
        );
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: placed,
                variant: 0,
            },
        );
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [10., 0., 0.]],
            },
        );
        (w, placed, unplaced)
    }

    fn surfaces(w: &World) -> Vec<String> {
        let names = w.resource::<Surfaces>().unwrap();
        w.iter::<Collision>()
            .flat_map(|(_, c)| c.surfaces.iter().map(|&s| names.0[usize::from(s)].name.clone()))
            .collect()
    }

    fn pass(options: &[(&str, &str)]) -> LowerFromSoftObjectCollision {
        LowerFromSoftObjectCollision::from_options(
            &options
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn the_read_keeps_every_body_and_the_program_chooses_what_becomes_collision() {
        // the convex shapes are their stored cores: nothing tessellated, the capsule's segment and radius kept
        let (w, placed, _) = world();
        let bodies = &w.get::<ObjectBodies>(placed).unwrap().bodies;
        assert!(matches!(
            &bodies[3].geometry,
            ObjectGeometry::Convex { core, capsule: Some(_), .. } if core.radius == 0.25
        ));
        // convex=tessellate: every body at its stored pose (counted), convex ones as their offset planes (counted)
        let (mut w, placed, unplaced) = world();
        let mut log = PassLog::default();
        pass(&[("convex", "tessellate")]).run(&mut w, &mut log).unwrap();
        let got = surfaces(&w);
        assert_eq!(got[..3], ["wood", "default", "metal"]);
        assert!(got.len() > 3 + 2 * 12 - 1 && got[3..].iter().all(|s| s == "default"));
        for (what, n) in [
            (
                "bodies placed at their stored pose, their motion type not interpreted",
                4,
            ),
            (
                "convex bodies tessellated as their offset planes' intersection, sharp where rounded",
                2,
            ),
            ("capsules tessellated as boxes around their segment", 1),
        ] {
            assert_eq!(log.approximations[what], n, "{what}");
        }
        assert!(
            !w.has::<ObjectBodies>(placed),
            "the placed prototype's bodies were consumed"
        );
        assert_eq!(
            w.get::<ObjectBodies>(unplaced).unwrap().bodies.len(),
            1,
            "the unplaced one's stay"
        );
        let soup = w.iter::<Collision>().next().unwrap().1;
        assert_eq!(soup.positions[0], [10., 0., 0.]);
        // after the three mesh triangles, the convex ones: the capsule's core reaches out by its radius, 0.75 on x
        let x = soup
            .positions
            .iter()
            .skip(9)
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((x - 10.75).abs() < 1e-5, "{x}");

        // convex=leave-out, motion type 2 and class 29 left out by choice, counted
        let (mut w, _, _) = world();
        let mut log = PassLog::default();
        pass(&[
            ("convex", "leave-out"),
            ("leave-out", "29"),
            ("leave-out-motion", "2"),
        ])
        .run(&mut w, &mut log)
        .unwrap();
        assert_eq!(surfaces(&w), ["wood"]);
        assert_eq!(log.counts["convex bodies left out"], 2);
        assert_eq!(log.counts["bodies of motion type 2 left out"], 1);
        assert_eq!(log.counts["triangles of left-out HitMtrl classes"], 1);
        assert!(log.approximations.keys().all(|k| !k.contains("tessellated")));

        // the convex choice is the program's; the render inference needs the prototype's model
        assert!(LowerFromSoftObjectCollision::from_options(&Options::new()).is_err());
        let (mut w, _, _) = world();
        let err = pass(&[("convex", "tessellate"), ("convex-surface", "render")])
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("fromsoft.flver"), "{err}");
    }
}
