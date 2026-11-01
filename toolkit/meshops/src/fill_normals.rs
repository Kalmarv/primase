//! `fill-normals{method=smooth,no-direction=up|leave}`: the program's choice to give a normal to every vertex of a
//! triangle list that has none, for a destination that lights by vertex normals.
//!
//! **Which vertices.** In every loaded `core.mesh`, each primitive with a triangle-list topology: all its vertices when
//! it has no normal stream, else the vertices whose stored normal has no direction (no longer than 1e-10, the
//! threshold of [`crate::smooth_normals`]). Every other stored normal is kept as it is, and a primitive that is not a
//! triangle list is left. A normal stream that is not three floats is refused, naming the primitive.
//!
//! **Both options are required.** `method=smooth`: the area-weighted smooth normal of the primitive's own triangles
//! ([`crate::smooth_normals`], positions in the mesh's space). `no-direction`: what a vertex takes whose smooth normal
//! has no direction too (no triangle uses it, its triangles have no area, or their normals cancel): `up`, +Y of the
//! mesh's space; `leave`, its stored normal as it is (a zero normal where the primitive had no stream). Geometry the
//! smooth rule refuses (a position that is not finite, an index past the positions) refuses the pass, naming it.
//!
//! The file link of a changed mesh goes. Reported: vertices given a smooth normal and vertices given the up stand-in
//! (each an approximation, on the mesh's entity), vertices left with no direction, primitives given a normal stream,
//! and meshes still in a file (not filled).

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Mesh, MeshFile, Name, Semantic, Stream, Topology, World};

use crate::{NoDirection, smooth_normals};

/// A stored normal no longer than this has no direction (the threshold of [`crate::smooth_normals`]).
const SHORTEST_NORMAL: f64 = 1e-10;

/// See the module documentation. Made by [`FillNormals::new`]; from options ([`FILL_NORMALS`]) by the same values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillNormals {
    method: Method,
    no_direction: Undirected,
}

/// Option `method`: how a missing normal is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// The area-weighted smooth normal of the primitive's triangles ([`crate::smooth_normals`]).
    Smooth,
}

impl Choice for Method {
    const VALUES: &'static [(&'static str, Self)] = &[("smooth", Method::Smooth)];
}

/// Option `no-direction`: what a vertex takes whose made normal has no direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undirected {
    /// +Y of the mesh's space.
    Up,
    /// Its stored normal as it is (zero where the primitive had no normal stream).
    Leave,
}

impl Choice for Undirected {
    const VALUES: &'static [(&'static str, Self)] = &[("up", Undirected::Up), ("leave", Undirected::Leave)];
}

impl FillNormals {
    pub fn new(method: Method, no_direction: Undirected) -> FillNormals {
        FillNormals { method, no_direction }
    }

    fn from_given(g: &Given) -> FillNormals {
        FillNormals::new(g.choice("method"), g.choice("no-direction"))
    }
}

/// `fill-normals`' declaration ([`primase_core::pass::PassRegistry::add`]): both options required.
pub const FILL_NORMALS: PassSpec = PassSpec {
    name: "fill-normals",
    options: &[
        OptionSpec::choice::<Method>("method", "how a vertex with no normal gets one"),
        OptionSpec::choice::<Undirected>(
            "no-direction",
            "what a vertex takes whose made normal has no direction",
        ),
    ],
    make: |g| Ok(Box::new(FillNormals::from_given(g))),
};

/// What a primitive is: its entity (by name when it has one) and index.
fn describe(w: &World, e: Entity, i: usize) -> String {
    match w.get::<Name>(e) {
        Some(n) => format!("{e:?} {:?} primitive {i}", n.0),
        None => format!("{e:?} primitive {i}"),
    }
}

fn undirected(n: &[f32; 3]) -> bool {
    n.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() <= SHORTEST_NORMAL * SHORTEST_NORMAL
}

impl Pass for FillNormals {
    fn name(&self) -> &'static str {
        "fill-normals"
    }
    fn options(&self) -> Options {
        Options::from([
            ("method".to_owned(), self.method.text().to_owned()),
            ("no-direction".to_owned(), self.no_direction.text().to_owned()),
        ])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    /// The file links of the meshes it changed, which no longer match them.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn approximates(&self) -> &'static str {
        "normals of vertices that had none: area-weighted over the primitive's triangles, or straight up"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Method::Smooth = self.method;
        const UP: [f32; 3] = [0.0, 1.0, 0.0];
        let in_files = w.iter::<MeshFile>().filter(|(e, _)| !w.has::<Mesh>(*e)).count();
        log.count("meshes still in a file, not filled", in_files as u64);
        let meshes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let mut changed = false;
            let count = w.get::<Mesh>(e).expect("listed").primitives().len();
            for i in 0..count {
                let p = &w.get::<Mesh>(e).expect("listed").primitives()[i];
                if p.topology != Topology::Triangles {
                    continue;
                }
                let stored = match p.streams.get(&Semantic::Normal) {
                    None => None,
                    Some(Stream::F32x3(v)) => Some(v),
                    Some(_) => {
                        return Err(format!(
                            "fill-normals: {}: its normal stream is not three floats",
                            describe(w, e, i)
                        ));
                    }
                };
                let need: Vec<usize> = match stored {
                    None => (0..p.vertex_count()).collect(),
                    Some(v) => (0..v.len()).filter(|&k| undirected(&v[k])).collect(),
                };
                if need.is_empty() {
                    continue;
                }
                let positions = p.positions().ok_or_else(|| {
                    format!(
                        "fill-normals: {}: positions are not three floats",
                        describe(w, e, i)
                    )
                })?;
                let corners: Vec<u32> = match &p.indices {
                    Some(ix) => ix.clone(),
                    None => (0..p.vertex_count() as u32).collect(),
                };
                let (triangles, _) = corners.as_chunks::<3>();
                let smooth = smooth_normals(positions, triangles, NoDirection::StandIn(UP))
                    .map_err(|err| format!("fill-normals: {}: {err}", describe(w, e, i)))?;
                let mut normals = stored
                    .cloned()
                    .unwrap_or_else(|| vec![[0.0; 3]; p.vertex_count()]);
                let had_stream = stored.is_some();
                let mut stand_ins = smooth.stand_ins.iter().copied().peekable();
                let (mut made, mut up, mut left) = (0u64, 0u64, 0u64);
                for k in need {
                    while stand_ins.next_if(|&s| (s as usize) < k).is_some() {}
                    if stand_ins.next_if(|&s| s as usize == k).is_some() {
                        match self.no_direction {
                            Undirected::Up => {
                                normals[k] = UP;
                                up += 1;
                            }
                            Undirected::Leave => left += 1,
                        }
                    } else {
                        normals[k] = smooth.normals[k];
                        made += 1;
                    }
                }
                let mesh = w.get_mut::<Mesh>(e).expect("listed");
                let mut primitive = mesh.primitive_mut(i).expect("a listed primitive");
                primitive
                    .streams_mut()
                    .insert(Semantic::Normal, Stream::F32x3(normals));
                changed = true;
                if !had_stream {
                    log.count("primitives given a normal stream", 1);
                }
                if made > 0 {
                    log.approximate_on(e, "vertices given an area-weighted smooth normal", made);
                }
                if up > 0 {
                    log.approximate_on(e, "vertices with no direction given the up stand-in", up);
                }
                log.count("vertices left with no normal direction", left);
            }
            if changed {
                w.remove::<MeshFile>(e);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Primitive;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use std::collections::BTreeMap;

    /// A unit square in z = 0 (two triangles) and a fifth vertex no triangle uses.
    fn square(normals: Option<Vec<[f32; 3]>>) -> Primitive {
        let mut streams = BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [5.0, 5.0, 5.0],
            ]),
        )]);
        if let Some(n) = normals {
            streams.insert(Semantic::Normal, Stream::F32x3(n));
        }
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2, 0, 2, 3]),
            material: None,
        }
    }

    fn run(world: &mut World, options: &str) -> RunReport {
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        let pipeline = reg.parse(&format!("fill-normals{{{options}}}")).unwrap();
        let mut report = RunReport::default();
        PassManager::new(reg).run(world, &pipeline, &mut report).unwrap();
        report
    }

    fn normals(w: &World, e: Entity) -> Vec<[f32; 3]> {
        match &w.get::<Mesh>(e).unwrap().primitives()[0].streams[&Semantic::Normal] {
            Stream::F32x3(v) => v.clone(),
            _ => unreachable!(),
        }
    }

    /// Worked by hand: the square's corners face +z; the unused vertex has no direction and takes +y under `up`.
    /// A primitive with no normal stream gets one.
    #[test]
    fn a_primitive_without_normals_gets_smooth_ones_and_the_up_stand_in() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![square(None)]));
        let report = run(&mut w, "method=smooth,no-direction=up");
        assert_eq!(
            normals(&w, e),
            [
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0]
            ]
        );
        let log = &report.steps[0].log;
        assert_eq!(log.counts["primitives given a normal stream"], 1);
        assert_eq!(
            log.approximations["vertices given an area-weighted smooth normal"],
            4
        );
        assert_eq!(
            log.approximations["vertices with no direction given the up stand-in"],
            1
        );
    }

    /// Only stored zero normals are filled; a stored direction, however far from the smooth one, is kept; under
    /// `leave` a vertex with no direction keeps its stored zero and is counted.
    #[test]
    fn only_zero_normals_are_filled_and_leave_keeps_what_has_no_direction() {
        let mut w = World::new();
        let e = w.spawn();
        let stored = vec![[1.0, 0.0, 0.0], [0.0; 3], [0.0; 3], [0.0, 0.0, -1.0], [0.0; 3]];
        w.insert(e, Mesh::new(vec![square(Some(stored))]));
        w.insert(
            e,
            MeshFile {
                path: "m.glb".into(),
                mesh: 0,
                materials: vec![None],
            },
        );
        let report = run(&mut w, "method=smooth,no-direction=leave");
        assert_eq!(
            normals(&w, e),
            [
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, -1.0],
                [0.0; 3]
            ]
        );
        assert!(!w.has::<MeshFile>(e), "the file link of a changed mesh goes");
        let log = &report.steps[0].log;
        assert_eq!(
            log.approximations["vertices given an area-weighted smooth normal"],
            2
        );
        assert_eq!(log.counts["vertices left with no normal direction"], 1);
        assert!(!log.counts.contains_key("primitives given a normal stream"));
    }

    /// Both options are required: a pipeline that states neither, or only one, is refused.
    #[test]
    fn both_options_are_required() {
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        let given = |pairs: &[(&str, &str)]| -> Options {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        for pairs in [&[][..], &[("method", "smooth")], &[("no-direction", "up")]] {
            assert!(reg.make("fill-normals", &given(pairs)).is_err(), "{pairs:?}");
        }
        assert!(
            reg.make(
                "fill-normals",
                &given(&[("method", "smooth"), ("no-direction", "up")])
            )
            .is_ok()
        );
    }
}
