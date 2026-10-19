//! What a pass leaves out on purpose, by name: the measures a drop is counted in ([`Measure`]) and the helper that
//! despawns an entity declaring what it held ([`despawn`]). A pass records a drop with [`PassLog::dropped`]; a
//! conversion reports every drop as a selection notice ([`crate::convert::ConvertReport::notices`]).
//!
//! **Conservation diagnostics** (opt in: [`crate::pass::PassManager::with_conservation`]; off by default, and then
//! nothing below runs): the manager counts the four measures before and after each pass and compares what it lost
//! with what it declared ([`Conservation::Report`] records each difference in the step's report and as a diagnostic
//! notice; [`Conservation::Enforce`] fails the pass, for tests). A difference is a coverage finding (a pass that leaves
//! something out without saying so, or says so wrongly), never a reason to change content to satisfy the count.
//!
//! - **Placements**: entities with a `core.instance`. Exact: each one despawned, or that lost its instance, is a drop.
//! - **Textures**: entities with a `core.texture` or a `core.texture_file`. Exact, by entity: loading a file into
//!   pixels keeps the entity and is no drop; a texture despawned or left with neither is one.
//! - **Primitives**: the primitives of a `core.mesh`. Exact through [`World::rebuild_primitives`]: a source primitive
//!   none of the rebuilt ones comes from is a drop. A mesh replaced another way (a new [`Mesh`] inserted or assigned,
//!   moved with [`World::move_components`], despawned with its entity), before, between or after rebuilds, is counted
//!   net over all such meshes, so a move between entities is no drop; a mesh on an entity the pass made (a
//!   [`World::duplicate`]) adds to that balance. Meshes are told apart by identity ([`Mesh::new`] makes a new one; a
//!   clone, an in-place edit and [`World::apply_linear`] keep it), never by their primitive count.
//! - **Triangles**: inside kept primitives only (a dropped primitive's triangles go with it): per source primitive of a
//!   rebuild, what its pieces lost; per mesh edited in place ([`Mesh::primitives_mut`]), what each primitive lost. Net
//!   per primitive. Triangles are those of [`Topology::Triangles`] primitives, core's only triangle topology.
//!
//! What it does not see: a replaced mesh's triangle losses (counted net by primitives only), topologies other than
//! triangle lists, anything outside the four measures (collision soups, lights, markers, source namespaces) and
//! changes that keep the counts (a moved vertex, a recoloured texture). It is a check of declarations, not proof of
//! fidelity.
//!
//! Adding is never a drop and needs no declaration.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::components::{Instance, Mesh, Texture, TextureFile, Topology};
use crate::pass::PassLog;
use crate::world::{Entity, RebuildLog, World};

/// What a drop is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Measure {
    Placements,
    Primitives,
    Triangles,
    Textures,
}

impl Measure {
    pub const ALL: [Measure; 4] = [
        Measure::Placements,
        Measure::Primitives,
        Measure::Triangles,
        Measure::Textures,
    ];
}

impl fmt::Display for Measure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Measure::Placements => "placements",
            Measure::Primitives => "primitives",
            Measure::Triangles => "triangles",
            Measure::Textures => "textures",
        })
    }
}

/// What `e` holds of the measures: a placement, a texture, its mesh's primitives.
pub fn held(w: &World, e: Entity) -> Vec<(Measure, u64)> {
    let mut out = Vec::new();
    if w.has::<Instance>(e) {
        out.push((Measure::Placements, 1));
    }
    if w.has::<Texture>(e) || w.has::<TextureFile>(e) {
        out.push((Measure::Textures, 1));
    }
    if let Some(m) = w.get::<Mesh>(e) {
        out.push((Measure::Primitives, m.primitives().len() as u64));
    }
    out
}

/// Despawn `e` on purpose, declaring in `log` what it held ([`held`]) as left out for `why`. False, and nothing
/// declared, when it was not alive.
pub fn despawn(w: &mut World, e: Entity, log: &mut PassLog, why: &str) -> bool {
    if !w.is_alive(e) {
        return false;
    }
    for (m, n) in held(w, e) {
        log.dropped(m, why, n);
    }
    w.despawn(e)
}

/// Whether the pass manager runs the conservation diagnostics (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conservation {
    /// Not counted (the default): no scan, no cost.
    #[default]
    Off,
    /// Counted; each difference recorded in the step's report ([`crate::pass::StepReport::conservation`]).
    Report,
    /// Counted; a difference fails the pass ([`crate::pass::Error::Conservation`]).
    Enforce,
}

/// A pass whose losses in `measure` differ from what it declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub measure: Measure,
    /// What it left out, counted.
    pub dropped: u64,
    /// What it declared ([`PassLog::declared`]).
    pub declared: u64,
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} left out {} but declared {}",
            self.measure, self.dropped, self.declared
        )
    }
}

/// Triangles of each primitive of a mesh (0 for other topologies).
fn triangles(mesh: &Mesh) -> Vec<u64> {
    mesh.primitives()
        .iter()
        .map(|p| match p.topology {
            Topology::Triangles => (p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3) as u64,
            _ => 0,
        })
        .collect()
}

/// The world as the diagnostics see it before a pass.
#[derive(Debug)]
pub(crate) struct Before {
    placements: BTreeSet<Entity>,
    textures: BTreeSet<Entity>,
    /// Per mesh, its identity and its primitives' triangles.
    meshes: BTreeMap<Entity, (u64, Vec<u64>)>,
}

impl Before {
    pub(crate) fn take(w: &World) -> Self {
        let mut textures: BTreeSet<Entity> = w.iter::<Texture>().map(|(e, _)| e).collect();
        textures.extend(w.iter::<TextureFile>().map(|(e, _)| e));
        Before {
            placements: w.iter::<Instance>().map(|(e, _)| e).collect(),
            textures,
            meshes: w
                .iter::<Mesh>()
                .map(|(e, m)| (e, (m.id(), triangles(m))))
                .collect(),
        }
    }

    /// What the pass left out, per measure, given the world after it and its primitive rebuilds.
    pub(crate) fn dropped(&self, w: &World, rebuilds: &RebuildLog) -> BTreeMap<Measure, u64> {
        let placements = self.placements.iter().filter(|&&e| !w.has::<Instance>(e)).count() as u64;
        let textures = self
            .textures
            .iter()
            .filter(|&&e| !w.has::<Texture>(e) && !w.has::<TextureFile>(e))
            .count() as u64;
        let (mut primitives, mut tris) = (0u64, 0u64);
        // meshes changed some other way than a tracked rebuild: their primitives counted net, all together
        let (mut net_before, mut net_after) = (0u64, 0u64);
        let after: BTreeMap<Entity, (u64, Vec<u64>)> = w
            .iter::<Mesh>()
            .map(|(e, m)| (e, (m.id(), triangles(m))))
            .collect();
        for (e, (id, before)) in &self.meshes {
            match (rebuilds.get(e), after.get(e)) {
                // rebuilt from this mesh, and what the last rebuild made is still there
                (Some(Some(r)), Some((now_id, now))) if r.source == *id && r.made == *now_id => {
                    let kept: BTreeSet<usize> = r.from.iter().copied().collect();
                    primitives += (r.first - kept.len()) as u64;
                    for &j in &kept {
                        let pieces: u64 = r
                            .from
                            .iter()
                            .zip(now)
                            .filter(|(i, _)| **i == j)
                            .map(|(_, t)| t)
                            .sum();
                        tris += before[j].saturating_sub(pieces);
                    }
                }
                // the same mesh, edited in place at most
                (None, Some((now_id, now))) if now_id == id => {
                    tris += before
                        .iter()
                        .zip(now)
                        .map(|(b, a)| b.saturating_sub(*a))
                        .sum::<u64>();
                }
                (_, now) => {
                    net_before += before.len() as u64;
                    net_after += now.map_or(0, |(_, n)| n.len() as u64);
                }
            }
        }
        // meshes that appeared (a form's mesh moved onto its prototype, a loaded file) balance the net count
        for (e, (_, now)) in &after {
            if !self.meshes.contains_key(e) {
                net_after += now.len() as u64;
            }
        }
        primitives += net_before.saturating_sub(net_after);
        BTreeMap::from([
            (Measure::Placements, placements),
            (Measure::Primitives, primitives),
            (Measure::Triangles, tris),
            (Measure::Textures, textures),
        ])
    }

    /// Each measure whose count differs from what `log` declared.
    pub(crate) fn gaps(&self, w: &World, rebuilds: &RebuildLog, log: &PassLog) -> Vec<Gap> {
        self.dropped(w, rebuilds)
            .into_iter()
            .map(|(measure, dropped)| Gap {
                measure,
                dropped,
                declared: log.declared(measure),
            })
            .filter(|g| g.dropped != g.declared)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Primitive;

    use crate::components::{ColorSpace, Semantic, Stream, TexRole};
    use crate::pass::{Error, Pass, PassManager, PassRegistry, Pipeline, RunReport};
    use crate::texformat::TexFormat;
    use std::collections::BTreeMap as Map;

    /// A triangle list of `n` triangles.
    fn prim(n: u32) -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: Map::from([(Semantic::Position, Stream::F32x3(vec![[0.; 3]; 3]))]),
            indices: Some((0..n).flat_map(|_| [0, 1, 2]).collect()),
            material: None,
        }
    }

    fn texture() -> Texture {
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![0; 4]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        }
    }

    /// e0: a prototype of three primitives (1, 2, 3 triangles); e1, e2: its placements; e3: a texture file.
    fn world() -> World {
        let mut w = World::new();
        let p = w.spawn();
        w.insert(p, Mesh::new(vec![prim(1), prim(2), prim(3)]));
        for _ in 0..2 {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: p,
                    variant: 0,
                },
            );
        }
        let t = w.spawn();
        w.insert(t, TextureFile { path: "a.png".into() });
        w
    }

    /// A pass that runs `f` and declares `drops`.
    struct Do {
        f: fn(&mut World),
        drops: Vec<(Measure, u64)>,
    }
    impl Pass for Do {
        fn name(&self) -> &'static str {
            "do"
        }
        fn removes(&self) -> &[&'static str] {
            &["core.instance", "core.texture_file", "core.mesh"]
        }
        fn provides(&self) -> &[&'static str] {
            &["core.texture", "core.mesh"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["core.mesh"]
        }
        fn despawns_entities(&self) -> bool {
            true
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
            (self.f)(w);
            for &(m, n) in &self.drops {
                log.dropped(m, "on purpose", n);
            }
            Ok(())
        }
    }

    fn run_with(c: Conservation, f: fn(&mut World), drops: Vec<(Measure, u64)>) -> Result<RunReport, Error> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new()).with_conservation(c).run(
            &mut world(),
            &Pipeline {
                passes: vec![Box::new(Do { f, drops })],
            },
            &mut report,
        )?;
        Ok(report)
    }

    fn run(f: fn(&mut World), drops: Vec<(Measure, u64)>) -> Result<RunReport, Error> {
        run_with(Conservation::Enforce, f, drops)
    }

    fn left_out(f: fn(&mut World), drops: Vec<(Measure, u64)>) -> (Measure, u64, u64) {
        match run(f, drops) {
            Err(Error::Conservation {
                measure,
                dropped,
                declared,
                ..
            }) => (measure, dropped, declared),
            other => panic!("{other:?}"),
        }
    }

    /// Off by default: an undeclared drop passes unseen; Report records it in the step and the run goes on; Enforce
    /// fails the pass.
    #[test]
    fn the_check_is_off_unless_asked_and_reports_or_fails_as_asked() {
        let despawn = |w: &mut World| {
            w.despawn(Entity::from_index(1));
        };
        let r = run_with(Conservation::Off, despawn, vec![]).unwrap();
        assert!(r.steps[0].conservation.is_empty());
        let r = run_with(Conservation::Report, despawn, vec![]).unwrap();
        assert_eq!(
            r.steps[0].conservation,
            vec![Gap {
                measure: Measure::Placements,
                dropped: 1,
                declared: 0
            }]
        );
        assert!(
            r.to_string().contains("!placements left out 1 but declared 0"),
            "{r}"
        );
        assert_eq!(left_out(despawn, vec![]), (Measure::Placements, 1, 0));
    }

    #[test]
    fn placements_and_textures_are_counted_by_entity() {
        // a despawned placement must be declared; declaring it passes; declaring more than happened fails too
        let despawn = |w: &mut World| {
            w.despawn(Entity::from_index(1));
        };
        run(despawn, vec![(Measure::Placements, 1)]).unwrap();
        assert_eq!(
            left_out(despawn, vec![(Measure::Placements, 2)]),
            (Measure::Placements, 1, 2)
        );
        // spawning a placement doesn't excuse losing another
        let swap = |w: &mut World| {
            w.despawn(Entity::from_index(1));
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: Entity::from_index(0),
                    variant: 0,
                },
            );
        };
        assert_eq!(left_out(swap, vec![]), (Measure::Placements, 1, 0));
        // loading a texture file keeps the texture: no drop; losing both is one
        let load = |w: &mut World| {
            let t = Entity::from_index(3);
            w.remove::<TextureFile>(t);
            w.insert(t, texture());
        };
        run(load, vec![]).unwrap();
        let lose = |w: &mut World| {
            w.remove::<TextureFile>(Entity::from_index(3));
        };
        assert_eq!(left_out(lose, vec![]), (Measure::Textures, 1, 0));
    }

    #[test]
    fn a_rebuild_is_exact_so_a_split_cannot_hide_a_drop() {
        // drop primitive 0 and split primitive 2 in two: one more primitive than before, still one dropped
        let split = |w: &mut World| {
            w.rebuild_primitives(Entity::from_index(0), |_| {
                Ok((vec![prim(2), prim(1), prim(2)], vec![1, 2, 2]))
            })
            .unwrap();
        };
        // and primitive 2's pieces hold 3 of its 3 triangles: no triangle lost
        assert_eq!(left_out(split, vec![]), (Measure::Primitives, 1, 0));
        run(split, vec![(Measure::Primitives, 1)]).unwrap();
        // two rebuilds in one pass compose: the second drops a piece of primitive 2 (1 triangle of it lost)
        let twice = |w: &mut World| {
            let e = Entity::from_index(0);
            w.rebuild_primitives(e, |_| Ok((vec![prim(2), prim(1), prim(2)], vec![1, 2, 2])))
                .unwrap();
            w.rebuild_primitives(e, |_| Ok((vec![prim(2), prim(2)], vec![0, 2])))
                .unwrap();
        };
        assert_eq!(
            left_out(twice, vec![(Measure::Primitives, 1)]),
            (Measure::Triangles, 1, 0)
        );
        run(twice, vec![(Measure::Primitives, 1), (Measure::Triangles, 1)]).unwrap();
    }

    #[test]
    fn triangles_are_counted_inside_kept_primitives() {
        // decimate primitive 2 from 3 triangles to 1 in place
        let decimate = |w: &mut World| {
            w.get_mut::<Mesh>(Entity::from_index(0)).unwrap().primitives_mut()[2] = prim(1);
        };
        assert_eq!(left_out(decimate, vec![]), (Measure::Triangles, 2, 0));
        run(decimate, vec![(Measure::Triangles, 2)]).unwrap();
    }

    #[test]
    fn a_replaced_mesh_is_counted_net_whatever_its_primitive_count() {
        // replaced whole by one of the same count with fewer triangles (inserted, or assigned through get_mut): its
        // primitives are not the old ones, so nothing is compared primitive by primitive and nothing is left out
        let insert = |w: &mut World| {
            w.insert(Entity::from_index(0), Mesh::new(vec![prim(1), prim(1), prim(1)]));
        };
        run(insert, vec![]).unwrap();
        let assign = |w: &mut World| {
            *w.get_mut::<Mesh>(Entity::from_index(0)).unwrap() = Mesh::new(vec![prim(1), prim(1), prim(1)]);
        };
        run(assign, vec![]).unwrap();
        // a replacement between two rebuilds breaks their composition: what is left out is the net one primitive
        let between = |w: &mut World| {
            let e = Entity::from_index(0);
            w.rebuild_primitives(e, |_| Ok((vec![prim(2), prim(3), prim(1)], vec![1, 2, 0])))
                .unwrap();
            w.insert(e, Mesh::new(vec![prim(1), prim(1), prim(1)]));
            w.rebuild_primitives(e, |_| Ok((vec![prim(1), prim(1)], vec![1, 2])))
                .unwrap();
        };
        assert_eq!(left_out(between, vec![]), (Measure::Primitives, 1, 0));
        run(between, vec![(Measure::Primitives, 1)]).unwrap();
        // and one before the first rebuild: the rebuild starts from the replacement, not the mesh the pass was given
        let before = |w: &mut World| {
            let e = Entity::from_index(0);
            w.insert(e, Mesh::new(vec![prim(1), prim(1), prim(1)]));
            w.rebuild_primitives(e, |_| Ok((vec![prim(1), prim(1)], vec![1, 2])))
                .unwrap();
        };
        run(before, vec![(Measure::Primitives, 1)]).unwrap();
    }

    /// A mesh edited through [`World::edit_mesh`] counts as exactly as a rebuild: a split that adds a primitive cannot
    /// hide one dropped (a net count over a hand-made replacement would see none), and an edit that keeps the list is
    /// compared primitive by primitive. A failing edit puts the mesh back.
    #[test]
    fn an_edited_mesh_is_counted_exactly() {
        let split_and_drop = |w: &mut World| {
            w.edit_mesh(Entity::from_index(0), |m| {
                let p = m.primitives().to_vec();
                *m = Mesh::new(vec![p[1].clone(), prim(1), prim(2)]);
                Ok(Some(vec![1, 2, 2]))
            })
            .unwrap();
        };
        assert_eq!(left_out(split_and_drop, vec![]), (Measure::Primitives, 1, 0));
        run(split_and_drop, vec![(Measure::Primitives, 1)]).unwrap();
        let in_place = |w: &mut World| {
            w.edit_mesh(Entity::from_index(0), |m| {
                m.primitives_mut()[0] = prim(0);
                Ok(None)
            })
            .unwrap();
        };
        assert_eq!(left_out(in_place, vec![]), (Measure::Triangles, 1, 0));
        // an edit that rebuilds the mesh object but keeps its primitives in order (None) stays the same mesh, so a
        // later rebuild in the pass composes with it: a split there cannot hide a drop either
        let renewed_then_split = |w: &mut World| {
            let e = Entity::from_index(0);
            w.edit_mesh(e, |m| {
                *m = Mesh::new(m.primitives().to_vec());
                Ok(None)
            })
            .unwrap();
            w.edit_mesh(e, |m| {
                let p = m.primitives().to_vec();
                *m = Mesh::new(vec![p[1].clone(), prim(1), prim(2)]);
                Ok(Some(vec![1, 2, 2]))
            })
            .unwrap();
        };
        assert_eq!(left_out(renewed_then_split, vec![]), (Measure::Primitives, 1, 0));
        let mut w = world();
        let e = Entity::from_index(0);
        assert!(w.edit_mesh(e, |_| Err("no".into())).is_err());
        assert_eq!(w.get::<Mesh>(e).map(|m| m.primitives().len()), Some(3));
        assert!(w.edit_mesh(e, |_| Ok(Some(vec![5]))).is_err());
        assert_eq!(w.get::<Mesh>(e).map(|m| m.primitives().len()), Some(3));
        assert_eq!(w.edit_mesh(Entity::from_index(1), |_| Ok(None)), Ok(false));
    }

    /// The World APIs as the diagnostics see them: a clone keeps its mesh's identity (put back after an edit, it is
    /// compared primitive by primitive), a mesh moved between entities is no drop (the one it replaced is), a duplicate
    /// standing in for its original is no drop, a linear map keeps the mesh (no triangle lost), and a world clone or
    /// subset starts with no rebuild log of its own.
    #[test]
    fn clone_move_duplicate_and_linear_maps_count_as_they_should() {
        let clone_back = |w: &mut World| {
            let e = Entity::from_index(0);
            let mut m = w.get::<Mesh>(e).unwrap().clone();
            m.primitives_mut()[1] = prim(1);
            w.insert(e, m);
        };
        assert_eq!(left_out(clone_back, vec![]), (Measure::Triangles, 1, 0));
        let moved = |w: &mut World| {
            let form = w.spawn();
            w.insert(form, Mesh::new(vec![prim(1)]));
            w.move_components(form, Entity::from_index(0));
        };
        assert_eq!(left_out(moved, vec![]), (Measure::Primitives, 2, 0));
        run(moved, vec![(Measure::Primitives, 2)]).unwrap();
        let duplicated = |w: &mut World| {
            let p = Entity::from_index(0);
            let copy = w.duplicate(p).unwrap();
            for e in [Entity::from_index(1), Entity::from_index(2)] {
                w.get_mut::<Instance>(e).unwrap().prototype = copy;
            }
            w.despawn(p);
        };
        run(duplicated, vec![]).unwrap();
        let mirrored = |w: &mut World| {
            let l = crate::Linear::mirror_x();
            w.apply_linear(Entity::from_index(0), &l).unwrap();
        };
        run(mirrored, vec![]).unwrap();
        let mut w = world();
        w.start_rebuild_log();
        w.rebuild_primitives(Entity::from_index(0), |_| Ok((vec![prim(1)], vec![0])))
            .unwrap();
        assert!(w.clone().take_rebuild_log().is_empty());
        assert!(w.subset(&w.entities().collect()).take_rebuild_log().is_empty());
        assert_eq!(w.take_rebuild_log().len(), 1);
    }

    /// An entity's drop names what it held: a placement and its mesh's primitives; nothing for one gone already.
    #[test]
    fn despawn_declares_what_the_entity_held() {
        let mut w = World::new();
        let proto = w.spawn();
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let prim = Primitive {
            topology: Topology::Triangles,
            streams: Default::default(),
            indices: None,
            material: None,
        };
        w.insert(e, Mesh::new(vec![prim.clone(), prim]));
        let mut log = PassLog::default();
        assert!(despawn(&mut w, e, &mut log, "left out"));
        assert!(!despawn(&mut w, e, &mut log, "left out"));
        assert_eq!(log.declared(Measure::Placements), 1);
        assert_eq!(log.declared(Measure::Primitives), 2);
        assert_eq!(log.declared(Measure::Textures), 0);
        assert_eq!(
            log.drops.get(&(Measure::Primitives, "left out".to_owned())),
            Some(&2)
        );
    }
}
