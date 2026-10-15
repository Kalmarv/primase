//! A Unity scene into the world: one entity per drawn mesh renderer (its world [`Transform`], an [`Instance`] of a
//! prototype, its GameObject path as [`Name`], the `unity.*` details), Unity's frame converted. A prototype is a
//! mesh, or the slice of a combined mesh a static-batched renderer draws, with its geometry as a [`Mesh`].

use std::collections::BTreeMap;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Entity, Instance, Mesh, Name, Transform, World};

use crate::Result;
use crate::components::{UnityMesh, UnityObject, UnityRenderer};
use crate::mesh::UnityMeshData;
use crate::player::{ObjRef, Player};
use crate::scene::{Matrix, Scene, Skipped};

/// Unity's frame: left-handed, +Y up, +Z forward, metres. Into the world (right-handed, +Y up, -Z north): x, y, -z,
/// so Unity's forward is north; a mirroring frame (the mesh reader reverses triangle winding).
pub const UNITY: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

/// A Unity world matrix as a transform in Unity's frame (each entry rounded once to f32).
pub fn transform_of(m: &Matrix) -> Transform {
    let mut cols = [[0.0f32; 3]; 4];
    for (j, c) in cols.iter_mut().enumerate() {
        for (i, x) in c.iter_mut().enumerate() {
            *x = m[i][j] as f32;
        }
    }
    Transform { cols }
}

/// What reading a level did.
#[derive(Clone, Debug, Default)]
pub struct LevelReport {
    pub renderers: usize,
    pub prototypes: usize,
    /// Primitives of all prototypes (one per drawn submesh).
    pub primitives: usize,
    pub skipped: Skipped,
}

/// Read the scene file `level` of `player` into `w`.
pub fn read_level(player: &Player, level: &str, w: &mut World) -> Result<LevelReport> {
    let scene = Scene::read(player, level)?;
    let (renderers, skipped) = scene.renderers(player)?;
    let mut meshes: BTreeMap<ObjRef, UnityMeshData> = BTreeMap::new();
    let mut prototypes: BTreeMap<(ObjRef, Option<(u32, u32)>), Entity> = BTreeMap::new();
    let mut primitives = 0;
    for r in &renderers {
        let key = (r.mesh.clone(), r.submeshes);
        let proto = match prototypes.get(&key) {
            Some(&e) => e,
            None => {
                if !meshes.contains_key(&r.mesh) {
                    meshes.insert(r.mesh.clone(), UnityMeshData::read(player, &r.mesh)?);
                }
                let data = &meshes[&r.mesh];
                let range = match r.submeshes {
                    Some((first, count)) => first as usize..(first + count) as usize,
                    None => 0..data.submeshes.len(),
                };
                let mesh = Mesh {
                    primitives: data.primitives(range)?.into_iter().map(|(_, p)| p).collect(),
                };
                primitives += mesh.primitives.len();
                let e = w.spawn();
                w.insert(e, mesh);
                w.insert(
                    e,
                    UnityMesh {
                        mesh: UnityObject::from(&r.mesh),
                        submeshes: r.submeshes,
                    },
                );
                prototypes.insert(key, e);
                e
            }
        };
        let e = w.spawn();
        w.insert(e, UNITY.transform(&transform_of(&r.world)));
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(e, Name(r.path.clone()));
        w.insert(
            e,
            UnityRenderer {
                renderer: UnityObject::from(&r.renderer),
                layer: r.layer,
                materials: r
                    .materials
                    .iter()
                    .map(|m| m.as_ref().map(UnityObject::from))
                    .collect(),
                submeshes: r.submeshes,
                cast_shadows: r.cast_shadows,
            },
        );
    }
    Ok(LevelReport {
        renderers: renderers.len(),
        prototypes: prototypes.len(),
        primitives,
        skipped,
    })
}
