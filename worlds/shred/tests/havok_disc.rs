//! The 490 Havok 7.1.0-r1 packfiles of the disc (482 `.hkw` PAK entries + 8 `PHYSICS/*.HKX`) read by `havok-formats`'s
//! packfile view: every file parses with zero failures, every class is accounted for, and the structural identities the
//! format guarantees hold. The oracle is the data's own redundancy, not a second reader (no public reader has the
//! physics classes): stored AABBs contain the vertices, cumulative triangle offsets, welding count == triangles,
//! index bounds and orthonormal rotations are checked inside the views; the AABB containment is re-checked here.
//! The counts are the ones the offline recon census produced (`HK.md`: 12,789 bodies, 1,758 meshes, 6,445 subparts,
//! 646,081 triangles, 855,428 vertices, 8,780 convex shapes), and the header words, class signatures, body filter
//! words and material records the separate read-only census of the same 490 files found.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use havok_formats::packfile::{
    CLASS_SIGNATURES, Packfile,
    physics::{self, ConvexVertices, ExtendedMesh, RigidBody, Shape},
};
use inputs::{Game, Query, expect_found, install};
use rme_formats::pak::Pak;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = e.expect("directory entry").path();
        if path.is_dir() {
            files(&path, ext, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            out.push(path);
        }
    }
}

fn inside(points: &[[f32; 3]], center: [f32; 3], half: [f32; 3], tolerance: f32) -> bool {
    points.iter().all(|p| {
        (0..3).all(|k| p[k] >= center[k] - half[k] - tolerance && p[k] <= center[k] + half[k] + tolerance)
    })
}

#[test]
fn every_packfile_parses_and_the_census_matches() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let usrdir = root.join("PS3_GAME/USRDIR");
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut paks = Vec::new();
    files(&usrdir, "pak", &mut paks);
    paks.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paks.len());
    for path in paks {
        let pak = Pak::open(&path).unwrap();
        for (i, e) in pak.entries().iter().enumerate() {
            if e.name.ends_with(b".hkw") {
                let label = format!("{}:{}", path.display(), String::from_utf8_lossy(&e.name));
                blobs.push((label, pak.payload(i).unwrap().into_owned()));
            }
        }
    }
    let mut hkx = Vec::new();
    files(&usrdir.join("PHYSICS"), "hkx", &mut hkx);
    hkx.sort();
    for path in hkx {
        blobs.push((path.display().to_string(), std::fs::read(&path).unwrap()));
    }
    assert_eq!(blobs.len(), 490);

    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut bodies, mut fixed, mut meshes, mut subparts, mut triangles, mut vertices) = (0, 0, 0, 0, 0, 0);
    let (mut convex, mut convex_vertices, mut leaf_shapes) = (0, 0, 0);
    let mut surfaces = BTreeMap::<u32, usize>::new();
    let mut filters = BTreeMap::<u32, usize>::new();
    let mut storage_indices = BTreeSet::<u16>::new();
    let (mut records, mut stored_user_data) = (0, 0);
    let mut signed = BTreeSet::new();
    for (label, bytes) in &blobs {
        let fail = |e: havok_formats::Error| -> ! { panic!("{label}: {e}") };
        let pack = Packfile::read(bytes).unwrap_or_else(|e| fail(e));
        // version 7: user tag 0, the two header words after the contents version are filler
        assert_eq!((pack.user_tag, pack.filler), (0, [0xFFFF_FFFF; 2]), "{label}");
        // every class name is one of the 24 the reader checks (it refuses another signature for them)
        for c in pack.class_names() {
            assert!(
                CLASS_SIGNATURES.contains(&(c.name.as_str(), c.signature)),
                "{label}: {}",
                c.name
            );
            signed.insert(c.name.clone());
        }
        let census = physics::census(&pack);
        assert!(
            census.unknown.is_empty(),
            "{label}: unread classes {:?}",
            census.unknown
        );
        for (name, n) in census.decoded.iter().chain(&census.recognised) {
            *classes.entry(name.clone()).or_default() += n;
        }
        for body in physics::rigid_bodies(&pack) {
            let b = RigidBody::read(&pack, body).unwrap_or_else(|e| fail(e));
            bodies += 1;
            fixed += usize::from(b.motion_type == physics::MOTION_FIXED);
            *filters.entry(b.collision_filter_info).or_default() += 1;
            storage_indices.insert(b.storage_index);
            // the shape tree resolves to shapes this module reads
            let mut stack = vec![b.shape];
            while let Some(s) = stack.pop() {
                match Shape::read(&pack, s).unwrap_or_else(|e| fail(e)) {
                    Shape::Mopp { child } | Shape::ConvexTranslate { child, .. } => stack.push(child),
                    Shape::List { children } => stack.extend(children),
                    _ => leaf_shapes += 1,
                }
            }
        }
        for o in pack.objects_of("hkpStorageExtendedMeshShape").collect::<Vec<_>>() {
            let m = ExtendedMesh::read(&pack, o).unwrap_or_else(|e| fail(e));
            meshes += 1;
            subparts += m.subparts.len();
            triangles += m.triangle_count();
            for s in &m.subparts {
                vertices += s.vertices.len();
                assert!(
                    inside(&s.vertices, m.aabb_center, m.aabb_half_extents, 0.06),
                    "{label}: mesh {o:#x} has a vertex outside its stored AABB"
                );
                for m in &s.materials {
                    records += 1;
                    stored_user_data += usize::from(m.user_data != 0);
                    assert_eq!((m.restitution, m.friction), (0.0, 1.0), "{label}");
                }
                for &i in &s.material_indices {
                    let a = s.materials[usize::from(i)].filter_info;
                    *surfaces.entry((a >> 3) & 0x3F).or_default() += 1;
                }
            }
        }
        for o in pack.objects_of("hkpConvexVerticesShape").collect::<Vec<_>>() {
            let c = ConvexVertices::read(&pack, o).unwrap_or_else(|e| fail(e));
            convex += 1;
            convex_vertices += c.vertices.len();
            assert!(
                inside(&c.vertices, c.aabb_center, c.aabb_half_extents, 0.06),
                "{label}: convex {o:#x} has a vertex outside its stored AABB"
            );
        }
    }
    eprintln!(
        "SHRED havok: {bodies} bodies ({fixed} fixed), {meshes} meshes, {subparts} subparts, {triangles} triangles, {vertices} vertices, {convex} convex ({convex_vertices} vertices), {leaf_shapes} leaf shapes reached from bodies"
    );
    assert_eq!((bodies, fixed), (12_789, 2_021));
    assert_eq!(
        (meshes, subparts, triangles, vertices),
        (1_758, 6_445, 646_081, 855_428)
    );
    assert_eq!((convex, convex_vertices), (8_780, 473_820));
    let count = |n: &str| classes.get(n).copied().unwrap_or(0);
    for (name, n) in [
        ("hkpRigidBody", 12_789),
        ("hkpMoppBvTreeShape", 1_705),
        ("hkpMoppCode", 1_705),
        ("hkpStorageExtendedMeshShape", 1_758),
        ("hkpStorageExtendedMeshShapeMeshSubpartStorage", 6_445),
        ("hkpConvexVerticesShape", 8_780),
        ("hkpConvexVerticesConnectivity", 8_751),
        ("hkpConvexTranslateShape", 1_400),
        ("hkpBoxShape", 1_971),
        ("hkpCylinderShape", 199),
        ("hkpSphereShape", 16),
        ("hkpCapsuleShape", 96),
        ("hkpListShape", 10),
        ("hkpConstraintInstance", 161),
        ("hkpRagdollConstraintData", 72),
        ("hkpLimitedHingeConstraintData", 89),
        ("hkpBreakableConstraintData", 16),
        ("hkRootLevelContainer", 490),
        ("hkpPhysicsData", 490),
        ("hkpPhysicsSystem", 972),
    ] {
        assert_eq!(count(name), n, "{name}");
    }
    assert_eq!(signed.len(), CLASS_SIGNATURES.len());
    // the bodies' filter words (the census of `+0x2C`): layers only, system group 0
    assert_eq!(
        filters,
        BTreeMap::from([(0, 148), (2, 1_933), (7, 8_741), (11, 1_953), (15, 1), (18, 13)])
    );
    assert_eq!(storage_indices.len(), 540);
    assert_eq!((records, stored_user_data), (7_909, 6_279));
    // the surface field of every triangle is a `surfaces` row id (0..=53): 36 distinct ids occur
    assert_eq!(surfaces.values().sum::<usize>(), 646_081);
    assert_eq!(surfaces.keys().max(), Some(&53));
    assert_eq!(surfaces.len(), 36);
}
