//! Reader I/O and assembly of exactly the PAKs supplied by the program.
use super::{
    pak::Pak,
    scene::{self, Scene, Unread},
    scp_locators,
    scp_tables::{self, Chain},
};
use crate::{
    Result,
    components::{
        Locator, MaterialId, MaterialNamespace, MeshId, RefusedMesh, SceneLight, SourceSkeleton, SourceSkin,
        Spline, Spot, SpotCamera, VertexInput, VertexInputs,
    },
    invalid,
};
use primase_core::{
    Curve, Entity, Instance, Marker, Mesh, Primitive, Semantic, Source, Stream, Topology, World,
    frame::SourceFrame,
};
use std::collections::BTreeMap;

/// Coverage counts are occurrences, including repeated copies packaged in different PAKs.
#[derive(Clone, Debug, Default)]
pub struct LevelReport {
    pub quartets: usize,
    pub meshes: usize,
    pub rigid: usize,
    pub processed: usize,
    pub processed_vertices: usize,
    pub processed_batches: usize,
    pub source_skeletons: usize,
    pub source_bones: usize,
    pub nonfinite_normals: usize,
    pub omitted_normal_streams: usize,
    pub nonfinite_normal_exceptions: Vec<(MeshId, usize)>,
    pub refused: usize,
    pub submeshes: usize,
    pub placements: usize,
    /// Nodes read from the SCP node tables (kept in `scene::Scene::tables`, not in the World).
    pub nodes: usize,
    /// Spot chunks, as `shred.spot` entities, and how many of them are `Teleport` markers.
    pub spots: usize,
    pub teleports: usize,
    /// Spot chunks whose class hash matches no proven name, by hash.
    pub unnamed_classes: BTreeMap<u32, usize>,
    /// Splines read, by class (`SplineKind::class`: rail candidates, guides, AI paths, navigation paths, short
    /// paths), and those too short for a core curve.
    pub splines: usize,
    pub spline_classes: [usize; 5],
    pub short_splines: usize,
    /// Light records, as `shred.light` entities, by kind code.
    pub lights: usize,
    pub light_kinds: BTreeMap<u32, usize>,
    /// Spot camera records (`shred.spot_camera`), and locator records (`shred.locator`) of which waypoints are core
    /// markers.
    pub spot_cameras: usize,
    pub locators: usize,
    pub waypoints: usize,
    /// SCPs whose blob section chain does not end in the end record.
    pub chain_stops: usize,
    pub unread: Vec<(String, Vec<u8>, Unread)>,
    pub refusals: Vec<MeshRefusal>,
    /// Counts stored source vertices, once per library resource (not expanded per draw or placement).
    pub zero_normals: usize,
    pub normal_exceptions: Vec<(MeshId, usize)>,
    pub raw_vertex_streams: usize,
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
                "{} quartets; {} meshes ({} rigid, {} processed, {} refused); {} submeshes; {} placements",
                self.quartets,
                self.meshes,
                self.rigid,
                self.processed,
                self.refused,
                self.submeshes,
                self.placements
            ),
        )];
        lines.push((
            "shred scene".into(),
            format!(
                "{} nodes; {} spot chunks ({} teleport markers, facing unverified: markers face local +Z by core's convention); {} section chains stopped early",
                self.nodes, self.spots, self.teleports, self.chain_stops
            ),
        ));
        lines.push((
            "shred splines".into(),
            format!(
                "{} splines ({} rail candidates: probable grind rails, not proven; {} guides, {} ai paths, \
                 {} nav paths, {} short paths); {} with fewer than two points have no curve; {} lights as data, \
                 not core lights; {} spot cameras and {} locators ({} waypoint markers) as data",
                self.splines,
                self.spline_classes[0],
                self.spline_classes[1],
                self.spline_classes[2],
                self.spline_classes[3],
                self.spline_classes[4],
                self.short_splines,
                self.lights,
                self.spot_cameras,
                self.locators,
                self.waypoints
            ),
        ));
        for (class, n) in &self.unnamed_classes {
            lines.push((
                "shred spot class".into(),
                format!("{class:08x}: {n} chunks; no class name is proven for this hash"),
            ));
        }
        lines.push((
            "shred vertex attributes".into(),
            format!(
                "{} authored zero normals; {} source input streams retained per primitive",
                self.zero_normals, self.raw_vertex_streams
            ),
        ));
        for (source, count) in &self.normal_exceptions {
            lines.push((
                "shred zero normals".into(),
                format!(
                    "{} {:?} {:08x}: {count} stored vertices",
                    source.pak,
                    String::from_utf8_lossy(&source.quartet),
                    source.id
                ),
            ));
        }
        if self.processed > 0 {
            lines.push(("shred processed skins".into(), format!(
                "{} meshes / {} batches; {} source skeletons / {} bones; palettes, weights and original matrices retained in shred provenance; runtime rest/inverse-bind/deformation binding unproved, no core.skin assigned",
                self.processed, self.processed_batches, self.source_skeletons, self.source_bones
            )));
            lines.push(("shred nonfinite normals".into(), format!(
                "{} authored nonfinite normal vectors; {} primitive normal streams omitted; exact bits retained in shred.source_skin",
                self.nonfinite_normals, self.omitted_normal_streams
            )));
        }
        for (source, count) in &self.nonfinite_normal_exceptions {
            lines.push((
                "shred source normal exception".into(),
                format!(
                    "{} {:?} {:08x}: {count} stored vertices",
                    source.pak,
                    String::from_utf8_lossy(&source.quartet),
                    source.id
                ),
            ));
        }
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
/// `frame` is required: the caller states it. Native SHRED is left-handed, Y-up (a right-handed caller mirrors Z);
/// north and physical units are not established.
pub fn read_level(paks: &[(&str, &Pak)], frame: SourceFrame) -> Result<(World, LevelReport)> {
    let mut world = World::new();
    let mut report = LevelReport::default();
    let mut materials = BTreeMap::<(u32, Vec<u8>), Entity>::new();
    for &(label, pak) in paks {
        for (name, entries) in quartets(pak)? {
            // Nonzero IDs are globally disjoint across material banks. ID zero
            // occurs in all three: retain its authored namespace rather than
            // merging actor, character and environment defaults in one world.
            let namespace = name
                .split(|&b| b == b'\\' || b == b'/')
                .next()
                .unwrap_or(&name)
                .to_vec();
            let material_key = |id| (id, if id == 0 { namespace.clone() } else { Vec::new() });
            let scene = decode_quartet(pak, entries)?;
            report.quartets += 1;
            report
                .unread
                .extend(scene.unread.into_iter().map(|u| (label.into(), name.clone(), u)));
            let source_skeleton = scene.source_bones.map(|bones| {
                report.source_skeletons += 1;
                report.source_bones += bones.len();
                let e = world.spawn();
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:source_skeleton",
                        String::from_utf8_lossy(&name)
                    )),
                );
                world.insert(e, SourceSkeleton { frame, bones });
                e
            });
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
                    materials.entry(material_key(sub.material)).or_insert_with(|| {
                        let mat = world.spawn();
                        world.insert(mat, MaterialId(sub.material));
                        if sub.material == 0 {
                            world.insert(mat, MaterialNamespace(namespace.clone()));
                        }
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
                if model.processed.is_some() {
                    report.processed += 1;
                    report.processed_vertices += model.positions.len();
                    report.processed_batches += model.processed_batches;
                } else {
                    report.rigid += 1;
                }
                let nonfinite = model
                    .attributes
                    .normals
                    .iter()
                    .filter(|n| n.iter().any(|x| !x.is_finite()))
                    .count();
                report.nonfinite_normals += nonfinite;
                if nonfinite > 0 {
                    report
                        .nonfinite_normal_exceptions
                        .push((source.clone(), nonfinite));
                }
                report.zero_normals += model.attributes.zero_normals;
                if model.attributes.zero_normals > 0 {
                    report
                        .normal_exceptions
                        .push((source.clone(), model.attributes.zero_normals));
                }
                let positions: Vec<_> = model.positions.into_iter().map(|p| frame.point(p)).collect();
                if positions.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(0, "position overflow in requested frame"));
                }
                let mut primitives = Vec::new();
                let mut raw_primitives = Vec::new();
                let mut source_primitives = Vec::new();
                for sub in model.submeshes {
                    let mut indices = model.indices[sub.indices].to_vec();
                    // Keep each submesh's contiguous vertex range and rebase its indices.
                    let low = indices.iter().copied().min().unwrap_or(0);
                    let end = indices.iter().copied().max().map_or(0, |x| x + 1);
                    for index in &mut indices {
                        *index -= low;
                    }
                    // Source render triangles are CW relative to their authored normals; core fronts are CCW.
                    // This format convention is independent of a caller's handedness choice.
                    if !frame.flips_winding() {
                        for tri in indices.as_chunks_mut::<3>().0 {
                            tri.swap(1, 2);
                        }
                    }
                    let range = low as usize..end as usize;
                    if let Some(vertices) = &model.processed {
                        source_primitives.push(vertices[range.clone()].to_vec());
                    }
                    let mut streams = BTreeMap::from([(
                        Semantic::Position,
                        Stream::F32x3(positions[range.clone()].to_vec()),
                    )]);
                    let normals = &model.attributes.normals[range.clone()];
                    if normals.iter().flatten().all(|v| v.is_finite()) {
                        streams.insert(
                            Semantic::Normal,
                            Stream::F32x3(normals.iter().map(|&v| frame.direction(v)).collect()),
                        );
                    } else {
                        report.omitted_normal_streams += 1;
                    }
                    if let Some(color) = &model.attributes.colors {
                        streams.insert(Semantic::Color(0), Stream::F32x4(color[range.clone()].to_vec()));
                    }
                    for (logical, &selector) in sub.uv_remap.iter().enumerate() {
                        let stored = if selector == 255 {
                            logical
                        } else {
                            selector as usize
                        };
                        if let Some(uv) = &model.attributes.uvs[stored] {
                            streams.insert(
                                Semantic::TexCoord(logical as u8),
                                Stream::F32x2(uv[range.clone()].to_vec()),
                            );
                        }
                    }
                    let raw: BTreeMap<_, _> = model
                        .attributes
                        .raw
                        .iter()
                        .map(|(name, a)| {
                            (
                                name.clone(),
                                VertexInput {
                                    lanes: a.lanes,
                                    values: a.values[range.clone()].to_vec(),
                                },
                            )
                        })
                        .collect();
                    report.raw_vertex_streams += raw.len();
                    raw_primitives.push(raw);
                    primitives.push(Primitive {
                        topology: Topology::Triangles,
                        streams,
                        indices: Some(indices),
                        material: Some(materials[&material_key(sub.material)]),
                    });
                }
                world.insert(e, Mesh::new(primitives));
                if model.processed.is_some() {
                    world.insert(
                        e,
                        SourceSkin {
                            frame,
                            skeleton: source_skeleton.expect("decoded source skeleton"),
                            primitives: source_primitives,
                        },
                    );
                }
                if raw_primitives.iter().any(|p| !p.is_empty()) {
                    world.insert(
                        e,
                        VertexInputs {
                            frame,
                            primitives: raw_primitives,
                        },
                    );
                }
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
            let tables = scene.tables;
            report.nodes += tables.nodes.len();
            report.chain_stops += usize::from(matches!(tables.chain, Chain::Stopped { .. }));
            for chunk in tables.spots {
                let transform = frame.transform(&chunk.world);
                if transform.cols.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(chunk.section, "spot matrix overflow in requested frame"));
                }
                let e = world.spawn();
                let class_name = scp_tables::class_name(chunk.class);
                world.insert(e, transform);
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:spot:{:#x}:{}",
                        String::from_utf8_lossy(&name),
                        chunk.section,
                        chunk.index
                    )),
                );
                world.insert(
                    e,
                    Spot {
                        pak: label.into(),
                        quartet: name.clone(),
                        index: chunk.index,
                        class: chunk.class,
                        class_name: class_name.map(String::from),
                        shape: chunk.shape,
                        shape_name: scp_tables::shape_name(chunk.shape).map(String::from),
                        name: chunk.name,
                        id: chunk.id,
                        parent: chunk.parent,
                        pair: chunk.pair,
                        node: chunk.node,
                    },
                );
                report.spots += 1;
                match class_name {
                    Some("Teleport") => {
                        report.teleports += 1;
                        world.insert(
                            e,
                            Marker {
                                kind: "teleport".into(),
                                ground: None,
                                yaw: None,
                            },
                        );
                    }
                    Some(_) => {}
                    None => *report.unnamed_classes.entry(chunk.class).or_default() += 1,
                }
            }
            for spline in tables.splines {
                let e = world.spawn();
                let points: Vec<[f32; 3]> = spline.points.iter().map(|&p| frame.point(p)).collect();
                if points.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(
                        spline.section,
                        "spline point overflow in requested frame",
                    ));
                }
                report.splines += 1;
                report.spline_classes[spline.kind.class()] += 1;
                let kind = spline.kind.curve_kind();
                if points.len() >= 2 {
                    world.insert(
                        e,
                        Curve {
                            kind: kind.into(),
                            points,
                            closed: false,
                            tangents: None,
                            up: None,
                        },
                    );
                } else {
                    report.short_splines += 1;
                }
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:spline:{:#x}:{}",
                        String::from_utf8_lossy(&name),
                        spline.section,
                        spline.index
                    )),
                );
                world.insert(
                    e,
                    Spline {
                        pak: label.into(),
                        quartet: name.clone(),
                        flags: spline.flags,
                        section: spline.section,
                        index: spline.index,
                        id: spline.id,
                        parent: spline.parent,
                        counts: spline.counts,
                        words: spline.words,
                        surface: spline.surface,
                        matrix: spline.matrix,
                        params: spline.params,
                    },
                );
            }
            for light in tables.lights {
                let transform = frame.transform(&light.world);
                if transform.cols.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(light.section, "light matrix overflow in requested frame"));
                }
                let e = world.spawn();
                world.insert(e, transform);
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:light:{:#x}:{}",
                        String::from_utf8_lossy(&name),
                        light.section,
                        light.index
                    )),
                );
                report.lights += 1;
                *report.light_kinds.entry(light.kind).or_default() += 1;
                world.insert(
                    e,
                    SceneLight {
                        pak: label.into(),
                        quartet: name.clone(),
                        section: light.section,
                        index: light.index,
                        kind: light.kind,
                        rgba: light.rgba,
                        name: light.name,
                        params: light.params,
                        links: light.links,
                        tail_words: light.tail_words,
                        buffer: light.buffer,
                    },
                );
            }
            for cam in tables.cameras {
                let (camera, target) = (frame.transform(&cam.camera), frame.transform(&cam.target));
                if camera
                    .cols
                    .iter()
                    .chain(&target.cols)
                    .flatten()
                    .any(|x| !x.is_finite())
                {
                    return Err(invalid(
                        cam.section,
                        "spot camera matrix overflow in requested frame",
                    ));
                }
                let e = world.spawn();
                world.insert(e, camera);
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:spotcam:{:#x}:{}",
                        String::from_utf8_lossy(&name),
                        cam.section,
                        cam.index
                    )),
                );
                report.spot_cameras += 1;
                world.insert(
                    e,
                    SpotCamera {
                        pak: label.into(),
                        quartet: name.clone(),
                        section: cam.section,
                        index: cam.index,
                        id: cam.id,
                        name: cam.name,
                        parent: cam.parent,
                        value: cam.value,
                        flag: cam.flag,
                        target,
                    },
                );
            }
            for loc in tables.locators {
                let transform = frame.transform(&loc.world);
                if transform.cols.iter().flatten().any(|x| !x.is_finite()) {
                    return Err(invalid(loc.section, "locator matrix overflow in requested frame"));
                }
                let e = world.spawn();
                world.insert(e, transform);
                world.insert(
                    e,
                    Source(format!(
                        "{label}:{:?}:locator:{:#x}:{}",
                        String::from_utf8_lossy(&name),
                        loc.section,
                        loc.index
                    )),
                );
                report.locators += 1;
                // The class and kind hashes are sdbm of the names the record carries; a waypoint is a marker.
                if loc.class == primase_hash::sdbm(scp_locators::CLASS_LOCATOR.as_bytes())
                    && loc.kind == primase_hash::sdbm(scp_locators::KIND_WAYPOINT.as_bytes())
                {
                    report.waypoints += 1;
                    world.insert(
                        e,
                        Marker {
                            kind: "waypoint".into(),
                            ground: None,
                            yaw: None,
                        },
                    );
                }
                world.insert(
                    e,
                    Locator {
                        pak: label.into(),
                        quartet: name.clone(),
                        section: loc.section,
                        index: loc.index,
                        class: loc.class,
                        name: loc.name,
                        kind: loc.kind,
                    },
                );
            }
        }
    }
    Ok((world, report))
}
