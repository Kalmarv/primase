//! `lower-fromsoft-object-collision`: object prototypes' collision bodies (`fromsoft.object_collision`) as collision.
//! How convex shapes become triangles, which motion types and HitMtrl classes become collision, the surface of
//! convex bodies (which store none) and the crop are its options.
use super::collision::{attach, beyond, crop, owned, push, surface_of};
use super::settings::{HitClass, Metres, joined};
use crate::pose::FRAME;
use crate::pose::posed;
use crate::sekiro::source::{MaterialSlots, ObjectBodies, ObjectBody, ObjectGeometry, Resolution};
use crate::{components::models::SourceModel, materials::Material};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::Condition;
use primase_core::{
    Collision, Component, Entity, Instance, Name, Surfaces, Transform, World, world::Resource,
};
use primase_meshops::vector::Vec3;
use std::collections::{BTreeMap, BTreeSet};

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
    let posed = crate::pose::posed_positions(model).map_err(|e| format!("{prototype:?}: {e}"))?;
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
                *areas.entry(crate::preview::named_surface(m).0).or_default() +=
                    f64::from(cross.length()) * 0.5;
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

impl Choice for ConvexSurface {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("default", ConvexSurface::Default),
        ("render", ConvexSurface::Render),
    ];
}

/// Which of a prototype's stored collision files `lower-fromsoft-object-collision` places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionChoice {
    High,
    Low,
    /// The high-resolution files where the prototype has one, else its low-resolution ones.
    HighElseLow,
}

impl Choice for ResolutionChoice {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("high", ResolutionChoice::High),
        ("low", ResolutionChoice::Low),
        ("high-else-low", ResolutionChoice::HighElseLow),
    ];
}

/// What `lower-fromsoft-object-collision` makes of convex bodies (box, polytope, capsule), which a triangle soup
/// cannot hold as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConvexBodies {
    /// Triangles of the intersection of the core's planes offset by the radius: sharp where the shape is rounded, a
    /// capsule a box around its segment, a sphere a cube. Counted as approximations.
    Tessellate,
    /// The rounded surface within the stated error ([`primase_havok::ConvexCore::rounded_triangles`]: inside the
    /// shape, no point of it further away than the error): `rounded:<metres>`, the error more than 0. Counted.
    Rounded(Metres),
    /// Not lowered, counted.
    LeaveOut,
}

impl ConvexBodies {
    fn word(self) -> String {
        match self {
            ConvexBodies::Tessellate => "tessellate".to_owned(),
            ConvexBodies::Rounded(error) => format!("rounded:{error}"),
            ConvexBodies::LeaveOut => "leave-out".to_owned(),
        }
    }
    /// The value a `convex=` option states.
    fn parse(text: &str) -> std::result::Result<Self, String> {
        match text {
            "tessellate" => Ok(ConvexBodies::Tessellate),
            "leave-out" => Ok(ConvexBodies::LeaveOut),
            _ => match text.strip_prefix("rounded:") {
                Some(error) => {
                    let error = Metres::parse("convex=rounded", error)?;
                    if error.get() > 0. {
                        Ok(ConvexBodies::Rounded(error))
                    } else {
                        Err("convex=rounded: the error bound must be more than 0 metres".to_owned())
                    }
                }
                None => Err(format!(
                    "convex={text} is not tessellate, rounded:<metres> or leave-out"
                )),
            },
        }
    }
}

/// `lower-fromsoft-object-collision{convex=tessellate|rounded:<metres>|leave-out}`: every placed object prototype's bodies
/// (`fromsoft.object_collision`, of the files of the chosen resolution) through each placement's transform, at their stored pose whatever their motion type
/// (not interpreted), as one `objects` soup, a placement present only in some world states into an
/// `objects_state<n>` soup with its condition; the bodies of the placed prototypes consumed, unplaced ones' kept.
/// Each triangle gets its HitMtrl class's surface (`default` without one, counted). In a part of a conversion
/// (`core.part_scope`), only the placements its roots reach are lowered: a context placement's collision is its own
/// part's.
///
/// Every field is the program's choice, stated (no default): `resolution`, which stored files are placed
/// ([`ResolutionChoice`]; a prototype with none of them is counted); `convex`, what convex bodies become (a soup cannot hold a
/// rounded shape, so the program chooses its approximation); `leave_out`, HitMtrl classes (an ID's base value, modulo
/// 100) not lowered, counted; `leave_out_motion`, stored motion types whose bodies are not lowered, counted;
/// `convex_surface` ([`ConvexSurface`]); `reach`, leaving out, counted, the triangles whose centre lies more than that
/// many metres outside the read region (`fromsoft.read_scope`), or `None` for no crop. Options, all required:
/// `resolution=high|low|high-else-low`, `convex=tessellate|rounded:<metres>|leave-out`, `leave-out=` classes and `leave-out-motion=` types joined by `+` (empty: none),
/// `convex-surface=default|render`, `reach=N|none`.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftObjectCollision {
    pub resolution: ResolutionChoice,
    pub convex: ConvexBodies,
    pub leave_out: BTreeSet<HitClass>,
    pub leave_out_motion: BTreeSet<u64>,
    pub convex_surface: ConvexSurface,
    pub reach: Option<Metres>,
}

impl LowerFromSoftObjectCollision {
    /// From its declared options.
    fn from_given(g: &Given) -> std::result::Result<Self, String> {
        let leave_out_motion = g
            .required("leave-out-motion")
            .split('+')
            .filter(|m| !m.is_empty())
            .map(|m| {
                m.parse::<u64>()
                    .map_err(|_| format!("leave-out-motion: {m} is not a motion type"))
            })
            .collect::<std::result::Result<_, _>>()?;
        Ok(LowerFromSoftObjectCollision {
            resolution: g.choice("resolution"),
            convex: ConvexBodies::parse(g.required("convex"))?,
            leave_out: HitClass::parse_set("leave-out", g.required("leave-out"))?,
            leave_out_motion,
            convex_surface: g.choice("convex-surface"),
            reach: Metres::parse_or_none("reach", g.required("reach"))?,
        })
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
                let stored = match self.convex {
                    ConvexBodies::Rounded(error) => core.rounded_triangles(f64::from(error.get())),
                    _ => core.expanded_triangles(),
                };
                let mut triangles: Vec<[[f32; 3]; 3]> = stored
                    .map_err(|e| format!("body {}: {e}", body.name))?
                    .into_iter()
                    .map(|t| t.map(|v| posed(c, v)))
                    .collect();
                // the stored-order convention is the reflected frame's: a map that does not reflect turns them
                if det > 0. {
                    triangles.iter_mut().for_each(|t| t.swap(1, 2));
                }
                match self.convex {
                    ConvexBodies::Rounded(error) => log.approximate(
                        &format!("convex bodies tessellated as their rounded surface within {error} m"),
                        1,
                    ),
                    _ => {
                        log.approximate(
                            "convex bodies tessellated as their offset planes' intersection, sharp where rounded",
                            1,
                        );
                        if capsule.is_some() {
                            log.approximate("capsules tessellated as boxes around their segment", 1);
                        }
                    }
                }
                let n = triangles.len();
                Ok(Some((triangles, vec![-1; n])))
            }
        }
    }
}

/// `lower-fromsoft-object-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): every choice is the
/// program's, stated.
pub const LOWER_FROMSOFT_OBJECT_COLLISION: PassSpec = PassSpec {
    name: "lower-fromsoft-object-collision",
    options: &[
        OptionSpec::choice::<ResolutionChoice>(
            "resolution",
            "which stored collision files are placed: the high-resolution ones, the low, or the high where there is one",
        ),
        OptionSpec::required(
            "convex",
            "tessellate",
            "convex bodies tessellated, rounded:<metres> (within that error, above 0), or leave-out",
        ),
        OptionSpec::required(
            "leave-out",
            "22+29+36+40",
            "HitMtrl classes (0 to 99) not lowered (counted), joined by + (empty for none)",
        ),
        OptionSpec::required(
            "leave-out-motion",
            "2",
            "stored motion types whose bodies are not lowered (counted), joined by + (empty for none)",
        ),
        OptionSpec::choice::<ConvexSurface>(
            "convex-surface",
            "a convex body's surface the default one, or that of the prototype's largest colliding render area",
        ),
        OptionSpec::required(
            "reach",
            "50",
            "metres, at least 0: the triangles whose centre lies further outside the read region are left out \
             (counted), or none for no crop",
        ),
    ],
    make: |g| Ok(Box::new(LowerFromSoftObjectCollision::from_given(g)?)),
};

impl Pass for LowerFromSoftObjectCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-object-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("resolution".to_owned(), self.resolution.text().to_owned()),
            ("convex".to_owned(), self.convex.word()),
            ("leave-out".to_owned(), joined(&self.leave_out)),
            ("leave-out-motion".to_owned(), joined(&self.leave_out_motion)),
            ("convex-surface".to_owned(), self.convex_surface.text().to_owned()),
            ("reach".to_owned(), Metres::text_or_none(self.reach)),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[ObjectBodies::NAME]
    }
    /// A read region may place no model that has collision files (the reader gives `fromsoft.object_collision` only
    /// to those): nothing to lower, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Collision::NAME,
            crate::sekiro::source::CollisionSource::NAME,
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
        "bodies at their stored pose, their motion type not interpreted; with convex=tessellate, convex bodies as their offset planes' intersection (capsules as boxes); with convex=rounded:<metres>, as their rounded surface within that error; triangles without a HitMtrl, and classes without a known surface, as the default surface; with convex-surface=render, convex bodies' surface inferred from the render geometry"
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
                        .entry(super::condition_key(c))
                        .or_insert_with(|| (c.clone(), Collision::default()))
                        .1
                }
            };
            let has = |r: Resolution| bodies.files.iter().any(|f| f.resolution == r);
            let wanted = match self.resolution {
                ResolutionChoice::High => Resolution::High,
                ResolutionChoice::Low => Resolution::Low,
                ResolutionChoice::HighElseLow if has(Resolution::High) => Resolution::High,
                ResolutionChoice::HighElseLow => Resolution::Low,
            };
            if !has(wanted) {
                log.count(
                    "placements whose prototype has no collision file of the chosen resolution",
                    1,
                );
                continue;
            }
            let chosen = bodies.files.iter().filter(|f| f.resolution == wanted);
            for body in chosen.flat_map(|f| &f.bodies) {
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
                files: vec![crate::sekiro::source::ObjectCollisionFile {
                    file: "o000001.hkx".into(),
                    resolution: Resolution::High,
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
                }],
            },
        );
        let unplaced = w.spawn();
        w.insert(
            unplaced,
            ObjectBodies {
                files: vec![crate::sekiro::source::ObjectCollisionFile {
                    file: "o000002.hkx".into(),
                    resolution: Resolution::High,
                    bodies: vec![convex("box", "hknpBoxShape", cube(0.), None)],
                }],
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

    /// The pass from `options` over every other option stated as nothing left out, the default surface and no reach.
    fn pass(options: &[(&str, &str)]) -> Box<dyn Pass> {
        let mut o = Options::from(
            [
                ("resolution", "high"),
                ("leave-out", ""),
                ("leave-out-motion", ""),
                ("convex-surface", "default"),
                ("reach", "none"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned())),
        );
        o.extend(options.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        LOWER_FROMSOFT_OBJECT_COLLISION.instance(&o).unwrap()
    }

    #[test]
    fn the_read_keeps_every_body_and_the_program_chooses_what_becomes_collision() {
        // the convex shapes are their stored cores: nothing tessellated, the capsule's segment and radius kept
        let (w, placed, _) = world();
        let bodies: Vec<_> = w.get::<ObjectBodies>(placed).unwrap().bodies().cloned().collect();
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
            w.get::<ObjectBodies>(unplaced).unwrap().bodies().count(),
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

        // convex=rounded:0.01: the capsule's rounded surface reaches out by its radius less at most the error
        let (mut w, _, _) = world();
        let mut log = PassLog::default();
        let rounded = pass(&[("convex", "rounded:0.01")]);
        assert_eq!(rounded.options()["convex"], "rounded:0.01");
        rounded.run(&mut w, &mut log).unwrap();
        assert_eq!(
            log.approximations["convex bodies tessellated as their rounded surface within 0.01 m"],
            2
        );
        assert!(
            !log.approximations
                .contains_key("capsules tessellated as boxes around their segment")
        );
        let soup = w.iter::<Collision>().next().unwrap().1;
        let x = soup
            .positions
            .iter()
            .skip(9)
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((10.74..=10.75 + 1e-5).contains(&x), "{x}");
        for bad in ["rounded:0", "rounded:-1", "rounded:", "round"] {
            let mut o = pass(&[("convex", "tessellate")]).options();
            o.insert("convex".to_owned(), bad.to_owned());
            assert!(LOWER_FROMSOFT_OBJECT_COLLISION.instance(&o).is_err(), "{bad}");
        }

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
        assert!(LOWER_FROMSOFT_OBJECT_COLLISION.instance(&Options::new()).is_err());
        let (mut w, _, _) = world();
        let err = pass(&[("convex", "tessellate"), ("convex-surface", "render")])
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("fromsoft.flver"), "{err}");
    }
}
