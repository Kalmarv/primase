//! Object collision as the objects store it, and the pass that places it. The reader keeps each object prototype's
//! main HKX bodies (`fromsoft.object_collision`); which motion types and HitMtrl classes become collision, the
//! surface of convex bodies (which store none) and the crop are `lower-fromsoft-object-collision`'s options.
use super::collision::{attach, beyond, crop, push, surface_of};
use super::settings::{HitClass, Metres, joined};
use super::source::{MaterialSlots, ObjectBodies, ObjectBody};
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
    /// Their bodies by stored motion type, and their triangles by shape class.
    pub bodies_by_motion: BTreeMap<u64, usize>,
    pub triangles_by_shape: BTreeMap<String, usize>,
}

/// An object's main HKX `file` (decoded `bytes`) as its prototype's `fromsoft.object_collision`: every body as
/// stored, its triangles in the prototype's space.
pub fn bodies(file: &str, bytes: &[u8], report: &mut Report) -> Result<ObjectBodies> {
    let bodies = collision::read_bodies(bytes, None)?
        .into_iter()
        .map(|body| {
            *report.bodies_by_motion.entry(body.motion_type).or_default() += 1;
            *report.triangles_by_shape.entry(body.shape.clone()).or_default() += body.soup.triangles.len();
            ObjectBody {
                name: body.name,
                motion_type: body.motion_type,
                shape: body.shape,
                triangles: body
                    .soup
                    .triangles
                    .into_iter()
                    .map(|t| t.map(|v| FRAME.point(v)))
                    .collect(),
                materials: body.soup.materials,
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

/// The surface `lower-fromsoft-object-collision` gives convex bodies, which store no material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConvexSurface {
    /// `default`, as any triangle without a HitMtrl.
    #[default]
    Default,
    /// The surface of the prototype's largest colliding render area, an inference from its drawn geometry.
    Render,
}

/// `lower-fromsoft-object-collision`: every placed object prototype's bodies (`fromsoft.object_collision`) through
/// each placement's transform, at their stored pose whatever their motion type (not interpreted), as one `objects` soup, a
/// placement present only in some world states into an `objects_state<n>` soup with its condition; the bodies of the
/// placed prototypes consumed, unplaced ones' kept. Each triangle gets its HitMtrl class's surface (`default` without
/// one, counted).
///
/// Options: `leave-out=` HitMtrl classes (an ID's base value, modulo 100) not lowered, counted; `leave-out-motion=`
/// stored motion types whose bodies are not lowered, counted; `convex-surface=default|render` ([`ConvexSurface`]);
/// `reach=N` leaves out, counted, the triangles whose centre lies more than N metres outside the read region
/// (`fromsoft.read_scope`). Without these every body is lowered whole with its stored materials.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LowerFromSoftObjectCollision {
    /// HitMtrl classes whose triangles are not lowered.
    pub leave_out: BTreeSet<HitClass>,
    /// Stored motion types whose bodies are not lowered.
    pub leave_out_motion: BTreeSet<u64>,
    /// The surface of convex bodies.
    pub convex_surface: ConvexSurface,
    /// The crop around the read region, if any.
    pub reach: Option<Metres>,
}

impl LowerFromSoftObjectCollision {
    pub fn from_options(o: &Options) -> std::result::Result<Self, String> {
        let mut pass = Self::default();
        for (k, v) in o {
            match (k.as_str(), v.as_str()) {
                ("leave-out", v) => pass.leave_out = HitClass::parse_set(k, v)?,
                ("leave-out-motion", v) => {
                    pass.leave_out_motion = v
                        .split('+')
                        .filter(|m| !m.is_empty())
                        .map(|m| {
                            m.parse::<u64>()
                                .map_err(|_| format!("leave-out-motion: {m} is not a motion type"))
                        })
                        .collect::<std::result::Result<_, _>>()?
                }
                ("convex-surface", "default") => pass.convex_surface = ConvexSurface::Default,
                ("convex-surface", "render") => pass.convex_surface = ConvexSurface::Render,
                ("convex-surface", v) => return Err(format!("convex-surface={v} is not default or render")),
                ("reach", v) => pass.reach = Some(Metres::parse(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(pass)
    }
}

impl Pass for LowerFromSoftObjectCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-object-collision"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
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
        "bodies at their stored pose, their motion type not interpreted; triangles without a HitMtrl, and classes without a known surface, as the default surface; with convex-surface=render, convex bodies' surface inferred from the render geometry"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let crop = crop(world, self.reach)?;
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
            let transform = world
                .get::<Transform>(e)
                .ok_or_else(|| format!("{e:?}: placement without a transform"))?;
            let soup = match world.get::<Condition>(e) {
                None => &mut soup,
                Some(c) => {
                    &mut conditioned
                        .entry(super::states::condition_key(c))
                        .or_insert_with(|| (c.clone(), Collision::default()))
                        .1
                }
            };
            for body in &bodies.bodies {
                if self.leave_out_motion.contains(&body.motion_type) {
                    log.count(&format!("bodies of motion type {} left out", body.motion_type), 1);
                    continue;
                }
                log.approximate(
                    "bodies placed at their stored pose, their motion type not interpreted",
                    1,
                );
                for (tri, &material) in body.triangles.iter().zip(&body.materials) {
                    let v = tri.map(|p| super::point(transform, p));
                    if crop.is_some_and(|c| beyond(&v, c)) {
                        log.count("triangles beyond the reach around the read region, left out", 1);
                        continue;
                    }
                    if HitClass::of(material).is_some_and(|c| self.leave_out.contains(&c)) {
                        log.count("triangles of left-out HitMtrl classes", 1);
                        continue;
                    }
                    let surface =
                        if material < 0 && body.convex() && self.convex_surface == ConvexSurface::Render {
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

    fn body(name: &str, motion_type: u64, shape: &str, materials: Vec<i32>) -> ObjectBody {
        ObjectBody {
            name: name.into(),
            motion_type,
            shape: shape.into(),
            triangles: vec![[[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]; materials.len()],
            materials,
        }
    }

    /// A placed prototype with a static mesh body, a motion-type-2 mesh body and a convex box; an unplaced one.
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let placed = w.spawn();
        w.insert(
            placed,
            ObjectBodies {
                file: "o000001.hkx".into(),
                bodies: vec![
                    body("static", 0, "fsnpCustomParamCompressedMeshShape", vec![104, 29]),
                    body("moving", 2, "fsnpCustomParamCompressedMeshShape", vec![111]),
                    body("box", 0, "hknpBoxShape", vec![-1, -1]),
                ],
            },
        );
        let unplaced = w.spawn();
        w.insert(
            unplaced,
            ObjectBodies {
                file: "o000002.hkx".into(),
                bodies: vec![body("static", 0, "hknpBoxShape", vec![-1])],
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

    #[test]
    fn the_read_keeps_every_body_and_the_program_chooses_what_becomes_collision() {
        // without options: every body at its stored pose (counted), the box with the default surface
        let (mut w, placed, unplaced) = world();
        let mut log = PassLog::default();
        LowerFromSoftObjectCollision::default()
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(surfaces(&w), ["wood", "default", "metal", "default", "default"]);
        assert_eq!(
            log.approximations["bodies placed at their stored pose, their motion type not interpreted"],
            3
        );
        assert!(
            !w.has::<ObjectBodies>(placed),
            "the placed prototype's bodies were consumed"
        );
        assert_eq!(
            w.get::<ObjectBodies>(unplaced).unwrap().bodies.len(),
            1,
            "the unplaced one's stay"
        );
        let placed_soup = w.iter::<Collision>().next().unwrap().1;
        assert_eq!(placed_soup.positions[0], [10., 0., 0.]);

        // motion type 2 and class 29 left out by choice, counted
        let (mut w, _, _) = world();
        let mut log = PassLog::default();
        LowerFromSoftObjectCollision::from_options(&Options::from([
            ("leave-out".into(), "29".into()),
            ("leave-out-motion".into(), "2".into()),
        ]))
        .unwrap()
        .run(&mut w, &mut log)
        .unwrap();
        assert_eq!(surfaces(&w), ["wood", "default", "default"]);
        assert_eq!(log.counts["bodies of motion type 2 left out"], 1);
        assert_eq!(log.counts["triangles of left-out HitMtrl classes"], 1);

        // the render inference needs the prototype's model
        let (mut w, _, _) = world();
        let err = LowerFromSoftObjectCollision::from_options(&Options::from([(
            "convex-surface".into(),
            "render".into(),
        )]))
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
        assert!(err.contains("fromsoft.flver"), "{err}");
    }
}
