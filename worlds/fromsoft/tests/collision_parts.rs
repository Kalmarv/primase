//! The collision passes in a conversion in parts: each part lowers only the collision its roots reach, so two parts,
//! each loading the other's placement and collision file as context, write every body and every file once, exactly
//! what the whole world writes.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, Part, PartSource, Reader, Report, Result, WholeWorld, Writer, convert_parts,
};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, Target};
use primase_core::{Collision, Entity, Instance, Surfaces, Transform, World};
use primase_fromsoft::sekiro::collision::LowerFromSoftCollision;
use primase_fromsoft::sekiro::objects::{ConvexBodies, ConvexSurface, LowerFromSoftObjectCollision};
use primase_fromsoft::sekiro::settings::HitFilters;
use primase_fromsoft::sekiro::source::{
    CollisionPart, MapCollision, ObjectBodies, ObjectBody, ObjectGeometry,
};

/// A triangle by its three corners' bits and its surface name.
type Tri = ([[u32; 3]; 3], String);
/// One part's triangles by group.
type Groups = BTreeMap<String, Vec<Tri>>;

/// Two map collision files and two object placements, one of each per cell (x < 100, x >= 100).
fn world() -> (World, [Entity; 4]) {
    let mut w = World::new();
    let tri = |x: f32| [[x, 0., 0.], [x + 1., 0., 0.], [x, 0., -1.]];
    let mut records = Vec::new();
    for (file, x) in [("a.hkx", 0.), ("b.hkx", 200.)] {
        let e = w.spawn();
        w.insert(
            e,
            MapCollision {
                file: file.into(),
                parts: vec![CollisionPart {
                    name: file.into(),
                    hit_filter: 8,
                    condition: None,
                }],
                triangles: vec![tri(x), tri(x + 2.)],
                materials: vec![104, 1],
                invalid_material_keys: 0,
                dead_primitives: 0,
                unread_bodies: Vec::new(),
            },
        );
        records.push(e);
    }
    let proto = w.spawn();
    w.insert(
        proto,
        ObjectBodies {
            files: vec![primase_fromsoft::sekiro::source::ObjectCollisionFile {
                file: "o000001.hkx".into(),
                resolution: primase_fromsoft::sekiro::source::Resolution::High,
                bodies: vec![ObjectBody {
                    name: "body".into(),
                    motion_type: 0,
                    shape: "fsnpCustomParamCompressedMeshShape".into(),
                    geometry: ObjectGeometry::Mesh {
                        triangles: vec![tri(0.)],
                        materials: vec![111],
                    },
                }],
            }],
        },
    );
    let mut placements = Vec::new();
    for x in [10., 210.] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, 0., 0.]],
            },
        );
        placements.push(e);
    }
    (w, [records[0], records[1], placements[0], placements[1]])
}

struct Whole(World);
impl Reader for Whole {
    fn read(&mut self) -> Result<(World, Report)> {
        Ok((self.0.clone(), Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "fromsoft"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some([[0.; 3], [1.; 3]])
    }
}

/// The given parts, in order.
struct Fixed(Vec<Part>);
impl PartSource for Fixed {
    fn plan(&mut self, _world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.0.reverse();
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        Ok(self.0.pop())
    }
}

/// Every part's collision triangles, by group.
struct Soups(Rc<RefCell<Vec<Groups>>>);
impl Writer for Soups {
    fn target(&self) -> Target {
        Target::new("soups").carry_unknown()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let names = part.resource::<Surfaces>();
        let mut groups = BTreeMap::<String, Vec<Tri>>::new();
        for (e, c) in part.iter::<Collision>() {
            let group = part
                .get::<primase_fromsoft::sekiro::source::CollisionSource>(e)
                .unwrap()
                .group
                .clone();
            for (t, &s) in c.triangles.iter().zip(&c.surfaces) {
                groups.entry(group.clone()).or_default().push((
                    t.map(|i| c.positions[i as usize].map(f32::to_bits)),
                    names.unwrap().0[usize::from(s)].name.clone(),
                ));
            }
        }
        self.0.borrow_mut().push(groups);
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn convert(parts: &mut dyn PartSource) -> Vec<BTreeMap<String, Vec<Tri>>> {
    let (w, _) = world();
    let passes = Pipeline {
        passes: vec![
            Box::new(LowerFromSoftCollision {
                hit_filters: HitFilters::new([8]).unwrap(),
                water: BTreeSet::new(),
                leave_out: BTreeSet::new(),
                reach: None,
            }),
            Box::new(LowerFromSoftObjectCollision {
                resolution: primase_fromsoft::sekiro::objects::ResolutionChoice::High,
                convex: ConvexBodies::Tessellate,
                leave_out: BTreeSet::new(),
                leave_out_motion: BTreeSet::new(),
                convex_surface: ConvexSurface::Default,
                reach: None,
            }),
        ],
    };
    let written = Rc::new(RefCell::new(Vec::new()));
    convert_parts(
        &mut Whole(w),
        parts,
        Box::new(Soups(written.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &passes,
    )
    .unwrap();
    written.take()
}

/// Every triangle of every part, by group, sorted.
fn merged(parts: &[BTreeMap<String, Vec<Tri>>]) -> BTreeMap<String, Vec<Tri>> {
    let mut out = BTreeMap::<String, Vec<Tri>>::new();
    for part in parts {
        for (group, tris) in part {
            out.entry(group.clone()).or_default().extend(tris.iter().cloned());
        }
    }
    out.values_mut().for_each(|v| v.sort());
    out
}

#[test]
fn two_parts_with_a_halo_write_each_file_and_body_once() {
    let whole = merged(&convert(&mut WholeWorld::default()));
    assert_eq!((whole["game"].len(), whole["objects"].len()), (4, 2));
    let (_, [a, b, pa, pb]) = world();
    // each part owns one file and one placement and loads the other's as context
    let parts = convert(&mut Fixed(vec![
        Part {
            roots: BTreeSet::from([a, pa]),
            shared: BTreeSet::new(),
            context: BTreeSet::from([b, pb]),
            whole: false,
            area: None,
        },
        Part {
            roots: BTreeSet::from([b, pb]),
            shared: BTreeSet::new(),
            context: BTreeSet::from([a, pa]),
            whole: false,
            area: None,
        },
    ]));
    assert_eq!(parts.len(), 2);
    for part in &parts {
        assert_eq!((part["game"].len(), part["objects"].len()), (2, 1), "{part:?}");
    }
    assert_eq!(merged(&parts), whole);
}
