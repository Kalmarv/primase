//! `lower-stuntboost-collision{plain=C,rails=C,kill=leave-out|C,unassigned=C_...}`: every solid node (`<`, [`StuntboostNode`]) gets
//! `core.collision` on its own entity, placed by its transform: its mesh's triangles, each made of the surface named by
//! its primitive's material (a physics material `C_*`, its `.NNN` suffix dropped, so `C_Default_Wood_no_grind` keeps
//! the grind flag in its name). A surface's class comes from the material's substance by a named table
//! ([`SUBSTANCES`]); a physics material with no substance takes `C_Ice`'s ice, `plain` (`C_Default`, `C_Ramp`,
//! `C_SlowSkate`, ...) or, for the rails (`C_RailBoost`, `C_RailNoBoost`), `rails`; `C_Kill` triangles (soft props:
//! laundry, mattresses, leaves) are left out (counted) or take the class `kill` names. A primitive with no material
//! (common on collision meshes) is made of the physics material `unassigned` names (`C_Default`; the game's own default
//! is not read), its surface named so. A substance outside the table, or a material that is not a physics material, is
//! unclassified (counted).
//! Every option is the program's, stated (no default). The node keeps its mesh: whether a collision-only node (`_<`,
//! which the game never draws) is drawn is `omit-stuntboost-hidden`'s. Triggers (`#`) are not solid and get none.

use std::collections::BTreeMap;

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::world::Resource;
use primase_core::{
    Collision, Component, Entity, Instance, Mesh, Primitive, SurfaceClass, Surfaces, Topology, World,
};

use crate::components::{StuntboostCollision, StuntboostMaterial, StuntboostNode, StuntboostPhysics};

/// Physics material substances and their surface class.
pub const SUBSTANCES: [(&str, SurfaceClass); 7] = [
    ("Wood", SurfaceClass::Wood),
    ("Metal", SurfaceClass::Metal),
    ("MetalHollow", SurfaceClass::Metal),
    ("Cardboard", SurfaceClass::Other),
    ("Fabric", SurfaceClass::Fabric),
    ("PlasticHard", SurfaceClass::Plastic),
    ("PlasticSoft", SurfaceClass::Plastic),
];

/// A triangle primitive's triangles (its indices, or sequential); `None` for points and lines.
fn triangles(p: &Primitive) -> Option<Vec<[u32; 3]>> {
    if p.topology != Topology::Triangles {
        return None;
    }
    let n = p.positions()?.len();
    let indices: Vec<u32> = match &p.indices {
        Some(i) => i.clone(),
        None => (0..u32::try_from(n).ok()?).collect(),
    };
    Some(indices.as_chunks::<3>().0.to_vec())
}

/// What `C_Kill` triangles become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillSurface {
    /// Not collided with (counted).
    LeaveOut,
    /// Kept, of this class.
    Class(SurfaceClass),
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerStuntboostCollision {
    pub plain: SurfaceClass,
    pub rails: SurfaceClass,
    pub kill: KillSurface,
    /// The physics material a primitive without one is made of (`C_...`).
    pub unassigned: String,
}

impl LowerStuntboostCollision {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<LowerStuntboostCollision, String> {
        let class = |k: &str| -> Result<SurfaceClass, String> {
            let v = g.required(k);
            SurfaceClass::parse(v).ok_or_else(|| format!("{k}: {v} is not a surface class"))
        };
        Ok(LowerStuntboostCollision {
            plain: class("plain")?,
            rails: class("rails")?,
            kill: match g.required("kill") {
                "leave-out" => KillSurface::LeaveOut,
                _ => KillSurface::Class(class("kill")?),
            },
            unassigned: g.required("unassigned").to_owned(),
        })
    }

    /// The class of a physics material's triangles; `None` inside: unclassified; outer `None`: left out.
    fn class(&self, p: &StuntboostPhysics) -> Option<Option<SurfaceClass>> {
        if p.behaviour == "Kill" {
            return match self.kill {
                KillSurface::LeaveOut => None,
                KillSurface::Class(c) => Some(Some(c)),
            };
        }
        Some(match &p.substance {
            Some(s) => SUBSTANCES.iter().find(|(n, _)| n == s).map(|(_, c)| *c),
            None if p.behaviour == "Ice" => Some(SurfaceClass::Ice),
            None if p.behaviour.starts_with("Rail") => Some(self.rails),
            None => Some(self.plain),
        })
    }
}

/// `lower-stuntboost-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): every option is the
/// program's, without defaults.
pub const LOWER_STUNTBOOST_COLLISION: PassSpec = PassSpec {
    name: "lower-stuntboost-collision",
    options: &[
        OptionSpec::required(
            "plain",
            "wood",
            "the surface class of a solid physics material with no substance",
        ),
        OptionSpec::required(
            "rails",
            "metal",
            "the surface class of a rail physics material with no substance",
        ),
        OptionSpec::required(
            "kill",
            "leave-out",
            "what C_Kill triangles become: leave-out (not collided with, counted) or a surface class",
        ),
        OptionSpec::required(
            "unassigned",
            "C_Default",
            "the physics material (C_...) a primitive without one is made of",
        ),
    ],
    make: |g| Ok(Box::new(LowerStuntboostCollision::from_given(g)?)),
};

impl Pass for LowerStuntboostCollision {
    fn name(&self) -> &'static str {
        "lower-stuntboost-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("plain".to_owned(), self.plain.as_str().to_owned()),
            ("rails".to_owned(), self.rails.as_str().to_owned()),
            (
                "kill".to_owned(),
                match self.kill {
                    KillSurface::LeaveOut => "leave-out".to_owned(),
                    KillSurface::Class(c) => c.as_str().to_owned(),
                },
            ),
            ("unassigned".to_owned(), self.unassigned.clone()),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if StuntboostPhysics::parse(&self.unassigned).is_none() {
            return Err(format!(
                "unassigned={}: not a physics material (C_...)",
                self.unassigned
            ));
        }
        let solid: Vec<(Entity, Entity)> = w
            .iter::<StuntboostNode>()
            .filter(|(_, n)| n.name.collision == StuntboostCollision::Solid)
            .filter_map(|(e, _)| Some((e, w.get::<Instance>(e)?.prototype)))
            .collect();
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        // surface index by material entity (None: left out)
        let mut by_material: BTreeMap<Option<Entity>, Option<u16>> = BTreeMap::new();
        for (e, prototype) in solid {
            let Some(mesh) = w.get::<Mesh>(prototype) else {
                log.count("solid nodes without a mesh, no collision", 1);
                continue;
            };
            let mut collision = Collision::default();
            for p in mesh.primitives() {
                let surface = match by_material.get(&p.material) {
                    Some(s) => *s,
                    None => {
                        let material = p.material.and_then(|m| w.get::<StuntboostMaterial>(m));
                        if material.is_none() {
                            log.count(
                                "collision primitives without a material, as the unassigned one",
                                1,
                            );
                        }
                        let name = material.map_or(self.unassigned.as_str(), |m| m.name.as_str());
                        let physics = StuntboostPhysics::parse(name);
                        let class = match &physics {
                            Some(p) => self.class(p),
                            None => Some(None),
                        };
                        let s = match class {
                            None => None,
                            Some(class) => {
                                let surface_name = physics.as_ref().map_or(name, |p| p.name.as_str());
                                let id = surfaces.index(surface_name).map_err(|e| e.to_string())?;
                                if let Some(s) = surfaces.0.get_mut(usize::from(id)) {
                                    s.class = class;
                                }
                                if class.is_none() {
                                    log.count(&format!("unclassified collision surface {surface_name}"), 1);
                                }
                                Some(id)
                            }
                        };
                        by_material.insert(p.material, s);
                        s
                    }
                };
                let (Some(positions), Some(triangles)) = (p.positions(), triangles(p)) else {
                    continue;
                };
                let Some(surface) = surface else {
                    log.count("C_Kill triangles left out", triangles.len() as u64);
                    continue;
                };
                collision.extend(positions, &triangles, surface, |v| v);
            }
            if collision.triangles.is_empty() {
                log.count("solid nodes with no triangles kept", 1);
                continue;
            }
            log.count("solid nodes given collision", 1);
            w.insert(e, collision);
        }
        if !surfaces.0.is_empty() {
            w.insert_resource(surfaces);
        }
        Ok(())
    }
}
