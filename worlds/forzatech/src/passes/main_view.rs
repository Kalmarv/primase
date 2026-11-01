//! `select-forzatech-main-view`: the primitives the game draws in its main view kept, by their mesh records' bucket
//! flags; the others (occluders, bucket bit `0x40`, and meshes in no colour bucket, `bucket & 0x27 == 0`) removed,
//! counted. A mesh left with no primitive is removed with its per-primitive data.
use crate::components::MeshRecords;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Mesh, World};

/// The occluder bucket bit.
pub const OCCLUDER: u16 = 0x40;
/// The colour buckets: opaque, decal, transparent, alpha-to-coverage.
pub const COLOUR: u16 = 0x27;

/// Whether a mesh record's bucket flags draw it in the main view.
pub fn drawn(bucket_flags: u16) -> bool {
    bucket_flags & OCCLUDER == 0 && bucket_flags & COLOUR != 0
}

#[derive(Clone, Copy, Debug)]
pub struct SelectForzaMainView;

pub const SELECT_FORZATECH_MAIN_VIEW: PassSpec = PassSpec {
    name: "select-forzatech-main-view",
    options: &[],
    make: |_| Ok(Box::new(SelectForzaMainView)),
};

impl Pass for SelectForzaMainView {
    fn name(&self) -> &'static str {
        "select-forzatech-main-view"
    }
    fn requires(&self) -> &[&'static str] {
        &[MeshRecords::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, MeshRecords::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<(Entity, Vec<usize>, usize)> = world
            .iter::<MeshRecords>()
            .filter(|(e, _)| world.has::<Mesh>(*e))
            .map(|(e, r)| {
                let keep: Vec<usize> =
                    r.0.iter()
                        .enumerate()
                        .filter(|(_, m)| drawn(m.bucket_flags))
                        .map(|(k, _)| k)
                        .collect();
                (e, keep, r.0.len())
            })
            .filter(|(_, keep, n)| keep.len() != *n)
            .collect();
        for (e, keep, n) in meshes {
            let occluders = world.get::<MeshRecords>(e).map_or(0, |r| {
                r.0.iter().filter(|m| m.bucket_flags & OCCLUDER != 0).count()
            });
            log.count("occluder primitives removed", occluders as u64);
            log.count(
                "primitives in no colour bucket removed",
                (n - keep.len() - occluders) as u64,
            );
            if keep.is_empty() {
                world.remove_mesh(e)?;
                log.count("meshes left with no primitive removed", 1);
                continue;
            }
            world.rebuild_primitives(e, |mesh| {
                let primitives = keep.iter().map(|&k| mesh.primitives()[k].clone()).collect();
                Ok((primitives, keep.clone()))
            })?;
        }
        Ok(())
    }
}
