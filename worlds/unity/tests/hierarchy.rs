//! Gate for a scene's hierarchy against an independent reader's scan (`PRIMASE_UNITY_SCAN`, JSON: `level`, `instances`
//! with `src` "scene" for the drawn mesh renderers in scene order, each `path`, `layer`, `mesh` and `mats` as
//! [file, path id], `M` the world matrix rounded to 6 decimals, `shadows`, `submeshes`; `stats` with
//! `renderer_inactive`, `renderer_lod1plus`, `renderer_kept`), for the player `PRIMASE_UNITY_DATA` read with the type
//! tree package `PRIMASE_UNITY_TPK`.
//!
//! Every drawn renderer: path, layer, mesh, materials, static-batch slice and shadow mode exactly; the world matrix
//! within the scan's rounding (5e-7) plus 1e-12 relative (the two readers sum the products in another order). Then the
//! level in the world: one entity per renderer, its transform converted back to Unity's frame the f32 matrix bit for
//! bit, one prototype per mesh or batch slice; each prototype's primitives, converted back to Unity's frame corner by
//! corner, the mesh's own channels bit for bit (texture v as 1 - v), and the world's components valid.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance, real_data};
use primase_core::{Instance, Mesh, Semantic, Stream, Transform, World};
use primase_unity::TypeTreePackage;
use primase_unity::components::{UnityBehaviours, UnityLeftOut, UnityMesh, UnityRenderer, UnityTreeInstance};
use primase_unity::level::{UNITY, read_level, transform_of};
use primase_unity::mesh::UnityMeshData;
use primase_unity::player::{ObjRef, Player};
use primase_unity::scene::Scene;
use serde_json::{Value as Json, json};

fn obj(r: &ObjRef) -> Json {
    json!([r.file, r.path_id])
}

#[test]
fn renderers_match_the_scan() {
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let level = scan["level"].as_str().unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let scene = Scene::read(&player, level).unwrap();
    let (ours, skipped) = scene.renderers(&player).unwrap();
    let theirs: Vec<&Json> = scan["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["src"] == "scene")
        .collect();
    let mut records = GateReport::new(
        "renderers: path, layer, mesh, materials, batch slice, shadows",
        Tolerance::Exact,
    );
    let mut matrices = GateReport::new("renderers: world matrix", Tolerance::Abs(5e-7));
    assert_eq!(ours.len(), theirs.len(), "drawn renderers: ours vs the scan");
    for (r, t) in ours.iter().zip(&theirs) {
        let item = format!("{} ({}/{})", r.path, r.renderer.file, r.renderer.path_id);
        let mats: Vec<Json> = r
            .materials
            .iter()
            .map(|m| m.as_ref().map_or(Json::Null, obj))
            .collect();
        let sub = r.submeshes.map_or(Json::Null, |(a, b)| json!([a, b]));
        let mine = json!({"path": r.path, "layer": r.layer, "mesh": obj(&r.mesh), "mats": mats, "submeshes": sub,
            "shadows": r.cast_shadows});
        let want = json!({"path": t["path"], "layer": t["layer"], "mesh": t["mesh"], "mats": t["mats"],
            "submeshes": t.get("submeshes").cloned().unwrap_or(Json::Null), "shadows": t["shadows"]});
        if mine == want {
            records.record(&item, 0.0);
        } else {
            records.fail(&item, format!("ours {mine} vs {want}"));
        }
        let mut worst = 0f64;
        for i in 0..4 {
            for j in 0..4 {
                let (a, b) = (r.world[i][j], t["M"][i][j].as_f64().unwrap());
                let d = (a - b).abs() - 1e-12 * b.abs();
                worst = worst.max(d.max(0.0));
            }
        }
        matrices.record(&item, worst);
    }
    let stats = &scan["stats"];
    println!("{records}\n{matrices}");
    println!(
        "ours skipped: {skipped:?}; the scan's stats: inactive {}, LOD 1+ {}",
        stats["renderer_inactive"], stats["renderer_lod1plus"]
    );
    assert_eq!(
        skipped.get("inactive or disabled").copied().unwrap_or(0),
        stats["renderer_inactive"].as_u64().unwrap()
    );
    assert_eq!(
        skipped.get("LOD 1+").copied().unwrap_or(0),
        stats["renderer_lod1plus"].as_u64().unwrap()
    );
    records.assert_passed();
    matrices.assert_passed();

    // into the world: each renderer an entity, its transform Unity's matrix (rounded once to f32) converted exactly
    let mut w = World::new();
    let rep = read_level(&player, level, &mut w).unwrap();
    println!(
        "world: {} materials, {} textures, not decoded {:?}; not reproduced {:?}",
        rep.materials, rep.textures, rep.textures_not_decoded, rep.not_reproduced
    );
    let mut placed = GateReport::new(
        "world: renderer transforms, back in Unity's frame, the f32 matrix bit for bit",
        Tolerance::Exact,
    );
    // the scene's renderers (terrain trees are gated with the terrains)
    let entities: Vec<_> = w
        .iter::<UnityRenderer>()
        .filter(|(e, _)| !w.has::<UnityTreeInstance>(*e))
        .collect();
    let mut left_out = std::collections::BTreeSet::new();
    let mut left_out_scripts = std::collections::BTreeMap::new();
    assert_eq!(entities.len(), ours.len());
    for ((e, u), r) in entities.iter().zip(&ours) {
        assert_eq!(u.renderer.path_id, r.renderer.path_id);
        let t: &Transform = w.get(*e).unwrap();
        let back = UNITY.transform_to_source(t);
        let want = transform_of(&r.world);
        let same = back
            .cols
            .iter()
            .flatten()
            .zip(want.cols.iter().flatten())
            .all(|(a, b)| a.to_bits() == b.to_bits());
        if same {
            placed.record(&r.path, 0.0);
        } else {
            placed.fail(&r.path, format!("{:?} vs {:?}", back.cols, want.cols));
        }
        if let Some(why) = w.get::<UnityLeftOut>(*e) {
            assert!(
                w.get::<Instance>(*e).is_none(),
                "{}: left out ({}) but placed",
                r.path,
                why.0
            );
            left_out.insert(r.path.clone());
            let behaviours = w.get::<UnityBehaviours>(*e).map_or(&[][..], |b| &b.0);
            for b in behaviours {
                *left_out_scripts.entry(b.script.clone()).or_insert(0u64) += 1;
            }
            // the component a later lowering needs: a Shapes behaviour with its fields
            assert!(
                behaviours
                    .iter()
                    .any(|b| b.script.starts_with("Shapes.") && !b.data.is_empty()),
                "{}: no Shapes component data",
                r.path
            );
            continue;
        }
        let proto = w.get::<Instance>(*e).unwrap().prototype;
        let m = w.get::<UnityMesh>(proto).unwrap();
        assert_eq!((m.mesh.path_id, m.submeshes), (r.mesh.path_id, r.submeshes));
    }
    assert_eq!(left_out.len(), rep.left_out);
    println!("left out, their MonoBehaviours by script: {left_out_scripts:?}");
    // the scan's meshes or slices of the renderers placed
    // and the terrain trees' (gated with the terrains)
    let trees = scan["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["src"] == "tree");
    let slices: std::collections::BTreeSet<String> = theirs
        .iter()
        .copied()
        .filter(|t| !left_out.contains(t["path"].as_str().unwrap()))
        .chain(trees)
        .map(|t| format!("{} {}", t["mesh"], t.get("submeshes").unwrap_or(&Json::Null)))
        .collect();
    println!(
        "{placed}
world: {} renderer entities ({} left out), {} prototypes ({} meshes or batch slices in the scan's scene renderers placed), {} primitives",
        rep.renderers,
        rep.left_out,
        rep.prototypes,
        slices.len(),
        rep.primitives
    );
    assert_eq!(rep.prototypes, slices.len());
    assert!(UNITY.flips_winding(), "Unity's frame mirrors");
    let geometry = world_meshes(&player, &w);
    println!("{geometry}");
    assert_eq!(w.verify_components(), Vec::new());
    placed.assert_passed();
    geometry.assert_passed();
}

/// Each prototype's primitives against its Unity mesh: every corner of every primitive (winding reversed) back in
/// Unity's frame, each stream's value bit for bit with the channel's value at the corner's Unity vertex; the primitive's
/// vertices exactly those its submesh uses; one primitive per submesh with indices.
fn world_meshes(player: &Player, w: &World) -> GateReport {
    let mut gate = GateReport::new(
        "world meshes: primitives back in Unity's frame, corner by corner bit for bit",
        Tolerance::Exact,
    );
    let mut corners = 0u64;
    for (_, mesh, u) in w.iter2::<Mesh, UnityMesh>() {
        let r = ObjRef {
            file: u.mesh.file.clone(),
            path_id: u.mesh.path_id,
        };
        let item = format!("{}/{} {:?}", r.file, r.path_id, u.submeshes);
        let data = UnityMeshData::read(player, &r).unwrap();
        let first = u.submeshes.map_or(0, |s| s.0 as usize);
        let count = u.submeshes.map_or(data.submeshes.len(), |s| s.1 as usize);
        let drawn: Vec<usize> = (first..first + count)
            .filter(|&k| data.submeshes[k].index_count > 0)
            .collect();
        let mut problems = Vec::new();
        if mesh.primitives.len() != drawn.len() {
            problems.push(format!(
                "{} primitives for {} submeshes with indices",
                mesh.primitives.len(),
                drawn.len()
            ));
        }
        for (&k, p) in drawn.iter().zip(&mesh.primitives) {
            let s = data.submeshes[k];
            let at = (s.first_byte / data.index_size) as usize;
            let theirs: Vec<u32> = data.indices[at..at + s.index_count as usize]
                .iter()
                .map(|&i| i + s.base_vertex)
                .collect();
            let ours = p.indices.as_ref().unwrap();
            let used: std::collections::BTreeSet<u32> = theirs.iter().copied().collect();
            if ours.len() != theirs.len() || p.vertex_count() != used.len() {
                problems.push(format!(
                    "submesh {}: {} indices and {} vertices for {} and {}",
                    k,
                    ours.len(),
                    p.vertex_count(),
                    theirs.len(),
                    used.len()
                ));
                continue;
            }
            let channels: Vec<usize> = (0..12).filter(|&c| data.channels[c].is_some()).collect();
            let semantics: Vec<Semantic> = p.streams.keys().copied().collect();
            let want: Vec<Semantic> = {
                let mut v: Vec<Semantic> = channels
                    .iter()
                    .map(|&c| match c {
                        0 => Semantic::Position,
                        1 => Semantic::Normal,
                        2 => Semantic::Tangent,
                        3 => Semantic::Color(0),
                        c => Semantic::TexCoord(c as u8 - 4),
                    })
                    .collect();
                v.sort();
                v
            };
            if semantics != want {
                problems.push(format!("submesh {}: streams {semantics:?} for {want:?}", k));
                continue;
            }
            for (j, &v) in ours.iter().enumerate() {
                // world corner j of its triangle is Unity's corner with the last two swapped
                let c = match j % 3 {
                    1 => j + 1,
                    2 => j - 1,
                    _ => j,
                };
                let src = theirs[c] as usize;
                let v = v as usize;
                corners += 1;
                for (sem, st) in &p.streams {
                    let (ch, back): (usize, Vec<f32>) = match (sem, st) {
                        (Semantic::Position, Stream::F32x3(x)) => (0, UNITY.point_to_source(x[v]).to_vec()),
                        (Semantic::Normal, Stream::F32x3(x)) => (1, UNITY.direction_to_source(x[v]).to_vec()),
                        (Semantic::Tangent, Stream::F32x4(x)) => {
                            let [a, b, c, d] = x[v];
                            let [a, b, c] = UNITY.direction_to_source([a, b, c]);
                            (2, vec![a, b, c, -d])
                        }
                        (Semantic::Color(0), Stream::F32x4(x)) => (3, x[v].to_vec()),
                        (Semantic::TexCoord(n), Stream::F32x2(x)) => (4 + *n as usize, x[v].to_vec()),
                        other => panic!("unexpected stream {other:?}"),
                    };
                    let unity = data.channels[ch].as_ref().unwrap().vertex(src);
                    let unity: Vec<f32> = if ch >= 4 {
                        vec![unity[0], 1.0 - unity[1]]
                    } else {
                        unity[..back.len()].to_vec()
                    };
                    let same = back.iter().zip(&unity).all(|(a, b)| a.to_bits() == b.to_bits());
                    if !same && problems.len() < 4 {
                        problems.push(format!("submesh {} corner {j} {sem:?}: {back:?} vs {unity:?}", k));
                    }
                }
            }
        }
        if problems.is_empty() {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, problems.join("; "));
        }
    }
    println!("world meshes: {corners} corners compared");
    gate
}
