//! Packing the map's collision: each leaf of the layout ([`crate::collision`]) written as GLB pieces and map objects,
//! as `prepare-reskate-collision` planned it (`skate.collision_plan`, checked against the layout; without one, exactly:
//! every surface its own, only bit-identical corners merged), and each convex hull written whole. The writer's
//! encoding, beside the writer ([`crate::map`]): both move to `write/` together.

use std::collections::BTreeMap;

use primase_meshops::vector::Vec3;

use crate::collision::{CollisionResult, Leaf, MAX_PIECE_VERTS, surfaces_of};
use crate::components::{CollisionSurfaces, CollisionWeld, PlannedLeaf, SkateCollisionPlan};
use crate::glb::{self, GlbPrim};
use crate::model::{MapObject, cells_met};
use crate::scene::SurfaceTable;
use crate::{Error, Result};

/// Files written by the collision build, by relative name.
pub type Files = BTreeMap<String, Vec<u8>>;

/// Where a piece centred on `o` (the layout's frame) is placed in the map, the writer's offset `off` added.
fn at(off: Vec3, o: Vec3) -> [f32; 12] {
    MapObject::translation(
        o.x + off.x,
        o.y + off.y,
        if off.z != 0.0 { o.z + off.z } else { o.z },
    )
}

/// The error for a plan made for another layout than the writer's.
fn replan(why: String) -> Error {
    Error::Scene(format!(
        "the collision was planned for another layout ({why}): run prepare-reskate-collision with the writer's \
         settings, after every pass that changes the map's collision"
    ))
}

/// Done with the layout, `packed` leaves packed (by every packer of the world the plan was made for): an error when the
/// plan has leaves the layout did not give.
pub(crate) fn check_packed(plan: Option<&SkateCollisionPlan>, packed: usize) -> Result<()> {
    match plan {
        Some(p) if p.leaves.len() != packed => Err(replan(format!(
            "the plan has {} leaves, the layout {packed}",
            p.leaves.len()
        ))),
        _ => Ok(()),
    }
}

/// How the leaf `leaf` is written under `surfaces` and `weld`, over the surface table `surf`: its triangles per
/// surface, the surfaces written as another, the triangles its weld collapses. `prepare-reskate-collision`'s rule, for
/// every leaf of the world it plans (the whole map, or one collision cell of a map in parts).
pub(crate) fn plan_leaf(
    surf: &SurfaceTable,
    leaf: &Leaf<'_>,
    surfaces: CollisionSurfaces,
    weld: CollisionWeld,
) -> Result<PlannedLeaf> {
    let count = leaf.counts(surf.list.len());
    let into = surfaces_of(surf, &count, surfaces);
    let collapsed = leaf.pieces(&into, weld)?.iter().map(|p| p.collapsed).sum();
    let mut planned = PlannedLeaf {
        counts: BTreeMap::new(),
        into: BTreeMap::new(),
        collapsed,
    };
    for (s, &n) in count.iter().enumerate().filter(|&(_, &n)| n > 0) {
        planned.counts.insert(s, n);
        if into[s] != s {
            planned.into.insert(s, into[s]);
        }
    }
    Ok(planned)
}

/// The collision's packer: each leaf of the layout, as the plan says, into GLB files and map objects.
pub(crate) struct Pack<'a> {
    surf: &'a SurfaceTable,
    off: Vec3,
    plan: Option<&'a SkateCollisionPlan>,
    files: &'a mut Files,
    /// The leaves packed, to tell a plan with leaves the layout no longer has.
    seen: usize,
    /// Whether it encodes the pieces' GLB files; without, it makes their objects only (a pass's count).
    encode: bool,
}

impl<'a> Pack<'a> {
    /// A packer for the surface table `surf` and the writer's offset `off`, replaying `plan` (exact without one); an
    /// error for a plan made for another surface table.
    pub(crate) fn new(
        surf: &'a SurfaceTable,
        off: Vec3,
        plan: Option<&'a SkateCollisionPlan>,
        files: &'a mut Files,
    ) -> Result<Pack<'a>> {
        if let Some(p) = plan {
            let table: Vec<&str> = surf.list.iter().map(|(n, _)| n.as_str()).collect();
            if p.table != table {
                return Err(replan(format!(
                    "the surfaces {:?}, the writer's {table:?}",
                    p.table
                )));
            }
        }
        Ok(Pack {
            surf,
            off,
            plan,
            files,
            seen: 0,
            encode: true,
        })
    }

    /// The packer encoding its pieces' files (`encode`), or making their objects only.
    pub(crate) fn encoding(mut self, encode: bool) -> Pack<'a> {
        self.encode = encode;
        self
    }

    /// One leaf of the layout: its pieces written, each as GLB files of at most [`MAX_PIECE_VERTS`] vertices and a
    /// collision object per file. An error for a leaf the plan does not have, or has with other triangles.
    pub(crate) fn leaf(&mut self, leaf: Leaf<'_>, res: &mut CollisionResult) -> Result<()> {
        let ns = self.surf.list.len();
        let mut into: Vec<usize> = (0..ns).collect();
        let (weld, collapsed) = match self.plan {
            None => (CollisionWeld::Exact, None),
            Some(p) => {
                let planned = p
                    .leaves
                    .get(leaf.name)
                    .ok_or_else(|| replan(format!("its leaf {} is not in the plan", leaf.name)))?;
                let mine: BTreeMap<usize, usize> = leaf
                    .counts(ns)
                    .into_iter()
                    .enumerate()
                    .filter(|&(_, n)| n > 0)
                    .collect();
                if mine != planned.counts {
                    return Err(replan(format!(
                        "leaf {} has the triangles per surface {mine:?}, the plan {:?}",
                        leaf.name, planned.counts
                    )));
                }
                for (&s, &t) in &planned.into {
                    match into.get_mut(s) {
                        Some(i) if t < ns => *i = t,
                        _ => return Err(replan(format!("leaf {} rewrites surface {s} as {t}", leaf.name))),
                    }
                }
                (p.weld, Some(planned.collapsed))
            }
        };
        self.seen += 1;
        let pieces = leaf.pieces(&into, weld)?;
        let found: usize = pieces.iter().map(|p| p.collapsed).sum();
        if let Some(c) = collapsed
            && c != found
        {
            return Err(replan(format!(
                "leaf {}'s weld collapses {found} triangles, the plan {c}",
                leaf.name
            )));
        }
        let origin = leaf.origin;
        for piece in pieces {
            if piece.idx.is_empty() {
                continue;
            }
            let s = piece.surface;
            let stem = if leaf.water {
                format!("{}_water", leaf.name)
            } else {
                format!("{}_{}", leaf.name, self.surf.list[s].0)
            };
            let (pos, idx) = (piece.pos, piece.idx);
            let chunks: Vec<(Vec<f32>, Vec<u32>)> = if pos.len() / 3 <= MAX_PIECE_VERTS {
                vec![(pos, idx)]
            } else {
                primase_vertex::split_by_vertex_budget(&idx, pos.len() / 3, MAX_PIECE_VERTS)
                    .expect("valid indices")
                    .into_iter()
                    .map(|c| {
                        let p: Vec<f32> = c
                            .vertices
                            .iter()
                            .flat_map(|&v| pos[v as usize * 3..v as usize * 3 + 3].to_vec())
                            .collect();
                        (p, c.indices)
                    })
                    .collect()
            };
            let n = chunks.len();
            for (c, (p, i)) in chunks.into_iter().enumerate() {
                let stem_c = if n == 1 {
                    stem.clone()
                } else {
                    format!("{stem}_{c}")
                };
                let file = format!("{stem_c}.glb");
                let tris = (i.len() / 3) as i64;
                let cells = cells_met(&p, Vec3::new(origin.x + self.off.x, origin.y + self.off.y, 0.0));
                if self.encode {
                    self.files.insert(
                        file.clone(),
                        glb::write(
                            &stem_c,
                            &[GlbPrim {
                                blend: None,
                                material: None,
                                pos: p,
                                nrm: None,
                                uv: None,
                                idx: i,
                            }],
                        )?,
                    );
                }
                let mut ob = MapObject::new(stem_c, at(self.off, origin));
                ob.cells = cells;
                ob.collision_mesh = Some(format!("meshes/{file}"));
                ob.render = false;
                ob.collision_mode = if leaf.water { "water" } else { "triangle_mesh" };
                ob.collision_packed = if leaf.water {
                    MapObject::WATER_PACKED
                } else {
                    self.surf.list[s].1
                };
                res.objects.push(ob);
                res.triangles += tris;
                res.pieces += 1;
            }
        }
        Ok(())
    }

    /// The leaves it packed.
    pub(crate) fn seen(&self) -> usize {
        self.seen
    }

    /// One convex hull of surface `surface`, written whole.
    pub(crate) fn hull(
        &mut self,
        res: &mut CollisionResult,
        name: &str,
        pos: &[f32],
        idx: &[u32],
        surface: usize,
    ) -> Result<()> {
        if idx.is_empty() {
            return Ok(());
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in (0..pos.len().saturating_sub(2)).step_by(3) {
            let p = Vec3::new(pos[v], pos[v + 1], pos[v + 2]);
            mn = mn.min(p);
            mx = mx.max(p);
        }
        let origin = (mn + mx) * 0.5;
        let mut local = vec![0f32; pos.len()];
        for v in (0..pos.len().saturating_sub(2)).step_by(3) {
            local[v] = pos[v] - origin.x;
            local[v + 1] = pos[v + 1] - origin.y;
            local[v + 2] = pos[v + 2] - origin.z;
        }
        let cells = cells_met(
            &local,
            Vec3::new(origin.x + self.off.x, origin.y + self.off.y, 0.0),
        );
        let file = format!("{name}.glb");
        if self.encode {
            self.files.insert(
                file.clone(),
                glb::write(
                    name,
                    &[GlbPrim {
                        blend: None,
                        material: None,
                        pos: local,
                        nrm: None,
                        uv: None,
                        idx: idx.to_vec(),
                    }],
                )?,
            );
        }
        let mut ob = MapObject::new(name.to_owned(), at(self.off, origin));
        ob.cells = cells;
        ob.collision_mesh = Some(format!("meshes/{file}"));
        ob.render = false;
        ob.collision_mode = "hull";
        ob.collision_packed = self.surf.list[surface].1;
        res.objects.push(ob);
        res.hulls += 1;
        res.triangles += (idx.len() / 3) as i64;
        Ok(())
    }
}
