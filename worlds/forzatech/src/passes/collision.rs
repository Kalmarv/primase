//! `collide-forzatech-drawn{see-through=collide|leave-out}`: every prototype with a mesh collides with its own triangles,
//! one surface per material (named `forzatech:<material name>`, no class: what each material is made of is not read
//! here, so a destination's own rule for surfaces without a class applies). The install holds no collision mesh for
//! the world's placed models, so the drawn geometry standing in for it is an approximation, reported. Primitives in a
//! see-through bucket (transparent `0x04` or alpha-to-coverage `0x20`: glass, foliage cards, fences) collide or are
//! left out as the program says. It runs after `select-forzatech-main-view`, so occluders are gone.
use crate::components::MeshRecords;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::world::Resource;
use primase_core::{Collision, Component, Entity, Mesh, Name, Surfaces, Topology, World};

/// The see-through buckets: transparent and alpha-to-coverage.
pub const SEE_THROUGH: u16 = 0x24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeeThrough {
    Collide,
    LeaveOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollideForzaDrawn {
    pub see_through: SeeThrough,
}

pub const COLLIDE_FORZATECH_DRAWN: PassSpec = PassSpec {
    name: "collide-forzatech-drawn",
    options: &[OptionSpec::required(
        "see-through",
        "leave-out",
        "collide or leave-out: primitives in a transparent or alpha-to-coverage bucket",
    )],
    make: |g: &Given<'_>| {
        let see_through = match g.required("see-through") {
            "collide" => SeeThrough::Collide,
            "leave-out" => SeeThrough::LeaveOut,
            other => return Err(format!("see-through {other:?}: needs collide or leave-out")),
        };
        Ok(Box::new(CollideForzaDrawn { see_through }))
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
        Options::from([("see-through".into(), v.into())])
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
            if !c.triangles.is_empty() {
                world.insert(e, c);
            }
        }
        world.insert_resource(surfaces);
        Ok(())
    }
}
