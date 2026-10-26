//! Reader I/O and assembly of exactly the PAKs supplied by the program.
use super::{
    pak::Pak,
    scene::{self, Scene, Unread},
};
use crate::{
    Result,
    components::{MaterialId, MeshId, RefusedMesh},
    invalid,
};
use primase_core::{
    Entity, Instance, Mesh, Primitive, Semantic, Source, Stream, Topology, World, frame::SourceFrame,
};
use std::collections::BTreeMap;

/// Coverage counts are occurrences, including repeated copies packaged in different PAKs.
#[derive(Clone, Debug, Default)]
pub struct LevelReport {
    pub quartets: usize,
    pub meshes: usize,
    pub rigid: usize,
    pub refused: usize,
    pub submeshes: usize,
    pub placements: usize,
    pub unread: Vec<(String, Vec<u8>, Unread)>,
    pub refusals: Vec<MeshRefusal>,
}

#[derive(Clone, Debug)]
pub struct MeshRefusal {
    pub source: MeshId,
    pub offset: usize,
    pub reason: &'static str,
}

impl LevelReport {
    pub fn lines(&self) -> primase_core::convert::Report {
        let mut lines = vec![(
            "shred geometry".into(),
            format!(
                "{} quartets; {} meshes ({} rigid, {} refused); {} submeshes; {} placements",
                self.quartets, self.meshes, self.rigid, self.refused, self.submeshes, self.placements
            ),
        )];
        for r in &self.refusals {
            lines.push((
                "shred refused mesh".into(),
                format!(
                    "{} {:?} {:08x} mlp+{:#x}: {}",
                    r.source.pak,
                    String::from_utf8_lossy(&r.source.quartet),
                    r.source.id,
                    r.offset,
                    r.reason
                ),
            ));
        }
        for (pak, name, u) in &self.unread {
            lines.push((
                "shred unread".into(),
                format!(
                    "{pak} {:?} {}+{:#x} {} bytes: {}",
                    String::from_utf8_lossy(name),
                    u.file,
                    u.offset,
                    u.bytes,
                    u.reason
                ),
            ));
        }
        lines
    }
}

/// Quartet stems are raw archive names, not host filesystem paths.
pub fn quartets(pak: &Pak) -> Result<BTreeMap<Vec<u8>, [usize; 4]>> {
    let mut groups: BTreeMap<Vec<u8>, [Option<usize>; 4]> = BTreeMap::new();
    for (i, e) in pak.entries().iter().enumerate() {
        if e.attributes == 1 {
            continue;
        }
        let Some(dot) = e.name.iter().rposition(|&c| c == b'.') else {
            continue;
        };
        let slot = match &e.name[dot + 1..] {
            b"scp" => 0,
            b"mlp" => 1,
            b"mmp" => 2,
            b"mgp" => 3,
            _ => continue,
        };
        let group = groups.entry(e.name[..dot].to_vec()).or_insert([None; 4]);
        if group[slot].replace(i).is_some() {
            return Err(invalid(e.offset, "duplicate quartet extension"));
        }
    }
    groups
        .into_iter()
        .map(|(name, g)| {
            if let [Some(a), Some(b), Some(c), Some(d)] = g {
                Ok((name, [a, b, c, d]))
            } else {
                Err(invalid(
                    0,
                    format!("incomplete quartet {:?}", String::from_utf8_lossy(&name)),
                ))
            }
        })
        .collect()
}

pub fn decode_quartet(pak: &Pak, entries: [usize; 4]) -> Result<Scene> {
    let [a, b, c, d] = entries;
    scene::decode(
        &pak.payload(a)?,
        &pak.payload(b)?,
        &pak.payload(c)?,
        &pak.payload(d)?,
    )
}

/// Builds a World from every quartet in the caller's PAK list. No base/layer or difficulty selection occurs here.
/// `frame` is required: source units, north and handedness have not yet been independently established.
pub fn read_level(paks: &[(&str, &Pak)], frame: SourceFrame) -> Result<(World, LevelReport)> {
    let mut world = World::new();
    let mut report = LevelReport::default();
    let mut materials = BTreeMap::<u32, Entity>::new();
    for &(label, pak) in paks {
        for (name, entries) in quartets(pak)? {
            let scene = decode_quartet(pak, entries)?;
            report.quartets += 1;
            report
                .unread
                .extend(scene.unread.into_iter().map(|u| (label.into(), name.clone(), u)));
            let mut prototypes = BTreeMap::new();
            for model in scene.models {
                report.meshes += 1;
                report.submeshes += model.submeshes.len();
                let e = world.spawn();
                prototypes.insert(model.id, e);
                let source = MeshId {
                    pak: label.into(),
                    quartet: name.clone(),
                    id: model.id,
                };
                world.insert(e, source.clone());
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:{:08x}",
                        String::from_utf8_lossy(&name),
                        model.id
                    )),
                );
                for sub in &model.submeshes {
                    materials.entry(sub.material).or_insert_with(|| {
                        let mat = world.spawn();
                        world.insert(mat, MaterialId(sub.material));
                        mat
                    });
                }
                if let Some((offset, reason)) = model.refusal {
                    report.refused += 1;
                    report.refusals.push(MeshRefusal {
                        source,
                        offset,
                        reason,
                    });
                    world.insert(
                        e,
                        RefusedMesh {
                            offset,
                            reason: reason.into(),
                        },
                    );
                    continue;
                }
                report.rigid += 1;
                let positions: Vec<_> = model.positions.into_iter().map(|p| frame.point(p)).collect();
                if positions.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(0, "position overflow in requested frame"));
                }
                let mut primitives = Vec::new();
                for sub in model.submeshes {
                    let mut indices = model.indices[sub.indices].to_vec();
                    // Keep each submesh's contiguous vertex range and rebase its indices.
                    let low = indices.iter().copied().min().unwrap_or(0);
                    let end = indices.iter().copied().max().map_or(0, |x| x + 1);
                    for index in &mut indices {
                        *index -= low;
                    }
                    if frame.flips_winding() {
                        for tri in indices.as_chunks_mut::<3>().0 {
                            tri.swap(1, 2);
                        }
                    }
                    primitives.push(Primitive {
                        topology: Topology::Triangles,
                        streams: BTreeMap::from([(
                            Semantic::Position,
                            Stream::F32x3(positions[low as usize..end as usize].to_vec()),
                        )]),
                        indices: Some(indices),
                        material: Some(materials[&sub.material]),
                    });
                }
                world.insert(e, Mesh::new(primitives));
            }
            for (i, placement) in scene.placements.into_iter().enumerate() {
                let transform = frame.transform(&placement.transform);
                if transform.cols.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(0, "transform overflow in requested frame"));
                }
                let e = world.spawn();
                world.insert(
                    e,
                    Instance {
                        prototype: prototypes[&placement.id],
                        variant: 0,
                    },
                );
                world.insert(e, transform);
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:placement:{i}",
                        String::from_utf8_lossy(&name)
                    )),
                );
                report.placements += 1;
            }
        }
    }
    Ok((world, report))
}
