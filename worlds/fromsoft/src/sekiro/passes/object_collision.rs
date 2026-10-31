//! `lower-fromsoft-object-collision`: object prototypes' collision bodies (`fromsoft.object_collision`) as collision.
//! How convex shapes become triangles, which motion types and HitMtrl classes become collision, the surface of
//! convex bodies (which store none) and the crop are its options.
use super::collision::{attach, beyond, crop, game_of, owned, push, surface_of};
use super::settings::Metres;
use crate::pose::FRAME;
use crate::sekiro::{
    source::MaterialSlots, source::ObjectBodies, source::ObjectBody, source::ObjectGeometry,
};
use crate::{components::models::SourceModel, materials::Material};
use fromsoft_formats::affine::posed;
use fromsoft_formats::collision::{BodyGeometry, shape_to_file};
use fromsoft_formats::collision::{HitMeaning, MotionKind};
use fromsoft_formats::game::Resolution;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::Condition;
use primase_core::{
    Collision, Component, Entity, Instance, Name, Surfaces, Transform, World, world::Resource,
};
use primase_havok::ConvexCore;
use primase_meshops::vector::Vec3;
use std::collections::{BTreeMap, BTreeSet};

/// Whether a collision export includes authored dynamic bodies at their stored pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionSelection {
    /// Include every body; this does not simulate its motion.
    All,
    /// Exclude bodies named Dynamic, retaining unknown kinds with the stored-pose approximation.
    Fixed,
}
impl Choice for MotionSelection {
    const VALUES: &'static [(&'static str, Self)] = &[("all", Self::All), ("fixed", Self::Fixed)];
}
impl MotionSelection {
    fn includes(self, kind: MotionKind) -> bool {
        self == Self::All || kind != MotionKind::Dynamic
    }
}

/// The approximation both collision lowerings state for a body of any motion type they lower.
const STORED_POSE: &str = "bodies placed at their stored pose, their motion type not interpreted";

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
type BodyTriangles = (Vec<[[f32; 3]; 3]>, Vec<i32>, Vec<HitMeaning>);

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

/// Place object collision using named material and motion metadata retained by the reader. Swimming water,
/// Empty and Cloud classes do not form solid object collision. Unknown materials remain counted geometry.
/// Resolution, convex approximation, surface inference, crop and including dynamic bodies are real choices.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftObjectCollision {
    pub resolution: ResolutionChoice,
    pub convex: ConvexBodies,
    pub motion: MotionSelection,
    pub convex_surface: ConvexSurface,
    pub reach: Option<Metres>,
}

impl LowerFromSoftObjectCollision {
    /// From its declared options.
    fn from_given(g: &Given) -> std::result::Result<Self, String> {
        Ok(LowerFromSoftObjectCollision {
            resolution: g.choice("resolution"),
            convex: ConvexBodies::parse(g.required("convex"))?,
            motion: g.choice("motion"),
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
            ObjectGeometry::Mesh {
                triangles,
                materials,
                material_meanings,
            } => {
                if triangles.len() != materials.len() || materials.len() != material_meanings.len() {
                    return Err(format!(
                        "body {}: triangles, material IDs and named meanings differ in length",
                        body.name
                    ));
                }
                Ok(Some((
                    triangles.clone(),
                    materials.clone(),
                    material_meanings.clone(),
                )))
            }
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
                Ok(Some((triangles, vec![-1; n], vec![HitMeaning::Unbound; n])))
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
        OptionSpec::choice::<MotionSelection>(
            "motion",
            "all stored bodies, or fixed bodies without positive-mass dynamics",
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
            ("motion".to_owned(), self.motion.text().to_owned()),
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
        let game = game_of(world);
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
                if !self.motion.includes(body.motion_kind) {
                    log.count(&format!("bodies of motion type {} left out", body.motion_type), 1);
                    continue;
                }
                let Some((triangles, materials, meanings)) = self.triangles(body, log)? else {
                    continue;
                };
                log.approximate(STORED_POSE, 1);
                for ((tri, material), meaning) in triangles.into_iter().zip(materials).zip(meanings) {
                    let v = tri.map(|p| transform.apply(p));
                    if crop.is_some_and(|c| beyond(&v, c)) {
                        log.count("triangles beyond the reach around the read region, left out", 1);
                        continue;
                    }
                    if matches!(
                        meaning,
                        HitMeaning::SwimmingWater | HitMeaning::Empty | HitMeaning::Cloud
                    ) {
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
                        surface_of(game, material, meaning, log)
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

/// `lower-fromsoft-part-collision{convex=tessellate|rounded:<metres>|leave-out,motion=all|fixed}`: the collision a
/// character's part keeps (`fromsoft.object_collision` on a prototype no placement uses: a weapon's `_collidable.hkx`) as
/// `core.collision` on that prototype, in the prototype's own space, so a view of the part shows it where the part is. The
/// bodies are lowered as `lower-fromsoft-object-collision` lowers an object's (stored pose, motion type not
/// interpreted, convex bodies as the option says, a triangle's HitMtrl class its surface); there is no crop, no
/// HitMtrl leave-out and no resolution choice (a part stores one file, a high one). The bodies lowered are consumed;
/// prototypes a placement uses are left to the object pass.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerFromSoftPartCollision {
    pub convex: ConvexBodies,
    pub motion: MotionSelection,
}

/// `lower-fromsoft-part-collision`'s declaration: both choices are the program's, stated.
pub const LOWER_FROMSOFT_PART_COLLISION: PassSpec = PassSpec {
    name: "lower-fromsoft-part-collision",
    options: &[
        OptionSpec::required(
            "convex",
            "rounded:0.01",
            "convex bodies tessellated, rounded:<metres> (within that error, above 0), or leave-out",
        ),
        OptionSpec::choice::<MotionSelection>(
            "motion",
            "all stored bodies, or fixed bodies without positive-mass dynamics",
        ),
    ],
    make: |g| {
        Ok(Box::new(LowerFromSoftPartCollision {
            convex: ConvexBodies::parse(g.required("convex"))?,
            motion: g.choice("motion"),
        }))
    },
};

impl Pass for LowerFromSoftPartCollision {
    fn name(&self) -> &'static str {
        "lower-fromsoft-part-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("convex".to_owned(), self.convex.word()),
            ("motion".to_owned(), self.motion.text().to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[ObjectBodies::NAME]
    }
    /// An area's world holds object collision only under placements: nothing for this pass, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Collision::NAME,
            crate::sekiro::source::CollisionSource::NAME,
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
        "bodies at their stored pose, their motion type not interpreted; convex bodies as the convex option says; triangles without a HitMtrl, and classes without a known surface, as the default surface"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let placed: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let parts: Vec<Entity> = world
            .iter::<ObjectBodies>()
            .map(|(e, _)| e)
            .filter(|e| !placed.contains(e))
            .collect();
        // the object pass's lowering of a body, with the options this pass has
        let object = LowerFromSoftObjectCollision {
            resolution: ResolutionChoice::High,
            convex: self.convex,
            motion: self.motion,
            convex_surface: ConvexSurface::Default,
            reach: None,
        };
        let game = game_of(world);
        let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
        let mut lowered = Vec::new();
        for part in parts {
            let Some(bodies) = world.get::<ObjectBodies>(part) else {
                continue;
            };
            let mut soup = Collision::default();
            for body in bodies.bodies() {
                if !self.motion.includes(body.motion_kind) {
                    log.count(&format!("bodies of motion type {} left out", body.motion_type), 1);
                    continue;
                }
                let Some((triangles, materials, meanings)) = object.triangles(body, log)? else {
                    continue;
                };
                log.approximate(STORED_POSE, 1);
                for ((tri, material), meaning) in triangles.into_iter().zip(materials).zip(meanings) {
                    let surface = surface_of(game, material, meaning, log);
                    push(
                        &mut soup,
                        tri,
                        surfaces.index(surface).map_err(|e| e.to_string())?,
                    )?;
                    log.count("part collision triangles", 1);
                }
            }
            lowered.push((part, soup));
        }
        world.insert_resource(surfaces);
        for (part, soup) in lowered {
            world.remove::<ObjectBodies>(part);
            log.count("parts whose collision was lowered", 1);
            if soup.triangles.is_empty() {
                continue;
            }
            world.insert(part, soup);
            world.insert(
                part,
                crate::sekiro::source::CollisionSource {
                    group: "parts".to_owned(),
                    water: false,
                },
            );
        }
        Ok(())
    }
}

/// A body's triangles as its file stores them, in the file frame: a mesh's as stored; a convex shape's core tessellated
/// as the intersection of its planes offset by the radius ([`ConvexCore::expanded_triangles`]: sharp where the shape is
/// rounded, a capsule a box around its core), posed by the body. The convex tessellation is an approximation.
pub fn body_triangles(geometry: &BodyGeometry) -> fromsoft_formats::Result<Vec<[[f32; 3]; 3]>> {
    match geometry {
        BodyGeometry::Mesh(soup) => Ok(soup.triangles.clone()),
        BodyGeometry::Convex {
            core,
            instance,
            position,
            orientation,
            ..
        } => {
            let pose = shape_to_file(instance.as_ref(), *position, *orientation);
            Ok(ConvexCore::from_shape(core)?
                .expanded_triangles()?
                .into_iter()
                .map(|t| t.map(|v| posed(&pose, v)))
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(name: &str, motion_type: u64, materials: Vec<i32>) -> ObjectBody {
        ObjectBody {
            name: name.into(),
            motion_type,
            motion_kind: fromsoft_formats::collision::motion_kind(motion_type),
            shape: "fsnpCustomParamCompressedMeshShape".into(),
            geometry: ObjectGeometry::Mesh {
                triangles: vec![[[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]; materials.len()],
                material_meanings: materials
                    .iter()
                    .map(|&id| {
                        if id == 29 {
                            HitMeaning::Other
                        } else {
                            fromsoft_formats::collision::hit_meaning(fromsoft_formats::Game::Sekiro, id)
                        }
                    })
                    .collect(),
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
            motion_kind: MotionKind::Static,
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
                ("motion", "all"),
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

        // Named fixture meanings replace the removed numeric class settings; body policy is fixed.
        let (mut w, placed, _) = world();
        if let ObjectGeometry::Mesh {
            material_meanings, ..
        } = &mut w.get_mut::<ObjectBodies>(placed).unwrap().files[0].bodies[0].geometry
        {
            material_meanings[1] = HitMeaning::Empty;
        }
        let mut log = PassLog::default();
        pass(&[("convex", "leave-out"), ("motion", "fixed")])
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

    #[test]
    fn a_parts_collision_becomes_collision_on_the_part_itself_and_a_placed_prototype_is_left_to_the_object_pass()
     {
        let (mut w, placed, unplaced) = world();
        let mut log = PassLog::default();
        let options = Options::from(
            [("convex", "tessellate"), ("motion", "fixed")].map(|(k, v)| (k.to_owned(), v.to_owned())),
        );
        let part = LOWER_FROMSOFT_PART_COLLISION.instance(&options).unwrap();
        assert_eq!(part.options(), options);
        part.run(&mut w, &mut log).unwrap();
        // the unplaced prototype (a part) has the collision itself, in its own space (no placement moved it), its
        // bodies consumed; the placed one keeps its bodies for the object pass
        let soup = w.get::<Collision>(unplaced).unwrap();
        assert!(!soup.triangles.is_empty());
        assert!(soup.positions.iter().all(|p| p[0].abs() <= 0.5 + 1e-5));
        assert!(!w.has::<ObjectBodies>(unplaced));
        assert!(w.has::<ObjectBodies>(placed) && !w.has::<Collision>(placed));
        assert_eq!(
            w.get::<crate::sekiro::source::CollisionSource>(unplaced)
                .unwrap()
                .group,
            "parts"
        );
        assert_eq!(log.counts["parts whose collision was lowered"], 1);
        assert!(w.verify_resources().is_empty());
        // the choices are the program's
        assert!(LOWER_FROMSOFT_PART_COLLISION.instance(&Options::new()).is_err());
    }
}
