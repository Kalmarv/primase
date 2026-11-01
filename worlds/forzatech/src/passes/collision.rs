//! `collide-forzatech-drawn{see-through=collide|leave-out}`: every prototype with a mesh collides with its own triangles,
//! one surface per material (named `forzatech:<material name>`, no class: what each material is made of is not read
//! here, so a destination's own rule for surfaces without a class applies). The install holds no collision mesh for
//! the world's placed models, so the drawn geometry standing in for it is an approximation, reported. Primitives in a
//! see-through bucket (transparent `0x04` or alpha-to-coverage `0x20`: glass, foliage cards, fences) collide or are
//! left out as the program says. It runs after `select-forzatech-main-view`, so occluders are gone.
//!
//! The world's ground (roads, pavements, terrain, water surfaces) is drawn by the heightfield cluster models, files
//! under the track's `tbheightfield` folder, each placed by one hand-placed instance. With `ground=world` a prototype
//! whose model files are all heightfield clusters collides as the world's own ground: each of its placements becomes a
//! collision soup of its own (an entity with `core.collision` and the placement's transform, made from the prototype),
//! and the prototype keeps none, counted; a destination that tells the static world from placed props (a street search
//! that reads only the former) then finds the ground. With `ground=placed` the clusters collide as any prototype does.
use std::collections::BTreeMap;

use crate::components::{MeshRecords, ModelFiles};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::world::Resource;
use primase_core::{
    Collision, Component, Entity, Instance, Mesh, Name, Surfaces, Topology, Transform, World,
};

/// The see-through buckets: transparent and alpha-to-coverage.
pub const SEE_THROUGH: u16 = 0x24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeeThrough {
    Collide,
    LeaveOut,
}

/// How the heightfield clusters (the world's ground) collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    /// As the world's own collision: a soup per placement (`ground=world`).
    World,
    /// As placed prototypes, like any other (`ground=placed`).
    Placed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollideForzaDrawn {
    pub see_through: SeeThrough,
    pub ground: Ground,
}

/// Whether every model file of `e` is a heightfield cluster: a file under the track's `tbheightfield` folder.
fn heightfield(world: &World, e: Entity) -> bool {
    world.get::<ModelFiles>(e).is_some_and(|m| {
        !m.files.is_empty()
            && m.files.iter().all(|f| {
                f.path
                    .split(['\\', '/'])
                    .any(|dir| dir.eq_ignore_ascii_case("tbheightfield"))
            })
    })
}

pub const COLLIDE_FORZATECH_DRAWN: PassSpec = PassSpec {
    name: "collide-forzatech-drawn",
    options: &[
        OptionSpec::required(
            "see-through",
            "leave-out",
            "collide or leave-out: primitives in a transparent or alpha-to-coverage bucket",
        ),
        OptionSpec::required(
            "ground",
            "world",
            "world or placed: the heightfield clusters collide as the world's own ground or as placed prototypes",
        ),
    ],
    make: |g: &Given<'_>| {
        let see_through = match g.required("see-through") {
            "collide" => SeeThrough::Collide,
            "leave-out" => SeeThrough::LeaveOut,
            other => return Err(format!("see-through {other:?}: needs collide or leave-out")),
        };
        let ground = match g.required("ground") {
            "world" => Ground::World,
            "placed" => Ground::Placed,
            other => return Err(format!("ground {other:?}: needs world or placed")),
        };
        Ok(Box::new(CollideForzaDrawn { see_through, ground }))
    },
};

impl Pass for CollideForzaDrawn {
    fn name(&self) -> &'static str {
        "collide-forzatech-drawn"
    }
    fn options(&self) -> Options {
        let v = match self.see_through {
            SeeThrough::Collide => "collide",
            SeeThrough::LeaveOut => "leave-out",
        };
        let g = match self.ground {
            Ground::World => "world",
            Ground::Placed => "placed",
        };
        Options::from([("see-through".into(), v.into()), ("ground".into(), g.into())])
    }
    fn requires(&self) -> &[&'static str] {
        &[MeshRecords::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Surfaces::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the drawn triangles stand in for collision: the install holds no collision mesh for placed models"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
        let entities: Vec<Entity> = world.iter::<Mesh>().map(|(e, _)| e).collect();
        let mut placements: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
        for (e, i) in world.iter::<Instance>() {
            placements.entry(i.prototype).or_default().push(e);
        }
        for e in entities {
            if world.has::<Collision>(e) {
                continue;
            }
            let (Some(mesh), Some(records)) = (world.get::<Mesh>(e), world.get::<MeshRecords>(e)) else {
                continue;
            };
            let mut c = Collision::default();
            for (p, r) in mesh.primitives().iter().zip(&records.0) {
                if p.topology != Topology::Triangles {
                    continue;
                }
                let (Some(positions), Some(indices)) = (p.positions(), p.indices.as_ref()) else {
                    continue;
                };
                let triangles = indices.as_chunks::<3>().0;
                if self.see_through == SeeThrough::LeaveOut && r.bucket_flags & SEE_THROUGH != 0 {
                    log.count(
                        "see-through triangles left out of collision",
                        triangles.len() as u64,
                    );
                    continue;
                }
                let name = p.material.and_then(|m| world.get::<Name>(m)).map_or_else(
                    || "forzatech:no material".to_owned(),
                    |n| format!("forzatech:{}", n.0),
                );
                let surface = surfaces.index(&name).map_err(|err| err.to_string())?;
                c.extend(positions, triangles, surface, |v| v);
                log.count("drawn triangles made collision", triangles.len() as u64);
            }
            if c.triangles.is_empty() {
                continue;
            }
            if self.ground == Ground::World && heightfield(world, e) {
                let name = world.get::<Name>(e).map(|n| format!("{} ground", n.0));
                let by = placements.get(&e).map_or(&[][..], Vec::as_slice);
                if by.is_empty() {
                    log.count("heightfield clusters nothing places: no ground made", 1);
                }
                for &at in by {
                    let soup = world.spawn();
                    world.insert(soup, c.clone());
                    if let Some(t) = world.get::<Transform>(at).cloned() {
                        world.insert(soup, t);
                    }
                    if let Some(n) = &name {
                        world.insert(soup, Name(n.clone()));
                    }
                    log.place_from(soup, e);
                    log.count("heightfield cluster placements made the world's ground", 1);
                    log.count("triangles of the world's ground", c.triangles.len() as u64);
                }
                continue;
            }
            world.insert(e, c);
        }
        world.insert_resource(surfaces);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{MeshRecord, ModelFile};
    use primase_core::{Primitive, Stream};

    fn record() -> MeshRecord {
        MeshRecord {
            file: "f".into(),
            name: "m".into(),
            lod_flags: 2,
            bucket_flags: 0,
            bucket_order: 0,
        }
    }

    fn file(path: &str) -> ModelFile {
        ModelFile {
            chunk: 0,
            entry: 0,
            path: path.into(),
            lod_flags: 2,
            has_lod: false,
            min_lod: 0,
            max_lod: 0,
            min: [0.0; 3],
            max: [1.0; 3],
        }
    }

    /// A prototype drawing one triangle from `path`, placed once at `x` metres east.
    fn placed(w: &mut World, path: &str, x: f32) -> Entity {
        let p = w.spawn();
        w.insert(
            p,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    primase_core::Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]),
                )]),
                indices: Some(vec![0, 1, 2]),
                material: None,
            }]),
        );
        w.insert(p, MeshRecords(vec![record()]));
        w.insert(
            p,
            ModelFiles {
                level: Some(0),
                files: vec![file(path)],
            },
        );
        w.insert(p, Name("cluster".into()));
        let i = w.spawn();
        w.insert(
            i,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        w.insert(
            i,
            Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, 0.0]],
            },
        );
        p
    }

    #[test]
    fn heightfield_clusters_become_the_worlds_ground_and_other_prototypes_keep_their_collision() {
        let mut w = World::new();
        let ground = placed(
            &mut w,
            "media\\tracks\\brio\\scene\\TBHeightfield\\autoterrain_c.modelbin",
            5.0,
        );
        let prop = placed(&mut w, "media\\tracks\\brio\\props\\bench.modelbin", 0.0);
        let mut log = PassLog::default();
        CollideForzaDrawn {
            see_through: SeeThrough::LeaveOut,
            ground: Ground::World,
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert!(
            !w.has::<Collision>(ground),
            "the cluster prototype keeps no collision"
        );
        assert!(w.has::<Collision>(prop));
        let soups: Vec<Entity> = w
            .iter::<Collision>()
            .map(|(e, _)| e)
            .filter(|&e| e != prop)
            .collect();
        assert_eq!(soups.len(), 1);
        let soup = soups[0];
        assert!(
            !w.has::<Mesh>(soup) && !w.has::<Instance>(soup),
            "a soup of its own, nothing places it"
        );
        assert_eq!(
            w.get::<Transform>(soup).unwrap().cols[3],
            [5.0, 0.0, 0.0],
            "the placement's transform"
        );
        assert_eq!(w.get::<Name>(soup).unwrap().0, "cluster ground");
        assert_eq!(log.placed_from[&soup], ground);
        assert_eq!(
            log.counts["heightfield cluster placements made the world's ground"],
            1
        );
        assert_eq!(log.counts["triangles of the world's ground"], 1);
    }

    #[test]
    fn placed_ground_collides_as_any_prototype() {
        let mut w = World::new();
        let ground = placed(&mut w, "scene/tbheightfield/autoterrain_c.modelbin", 0.0);
        let mut log = PassLog::default();
        CollideForzaDrawn {
            see_through: SeeThrough::LeaveOut,
            ground: Ground::Placed,
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert!(w.has::<Collision>(ground));
        assert_eq!(w.iter::<Collision>().count(), 1);
    }
}
