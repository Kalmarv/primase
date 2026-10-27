//! The Wolf's parts (`am_m_9120_l`, `bd_m_9120_l`, `lg_m_9120_l`, `hd_m_9120_l`, `fc_m_0220_l`) read each as an asset
//! of its own against SoulsFormats' reading of the same files (`oracles.soulsformats.sekiro/parts`, one folder per
//! part): every bone's name, parent and local transform, every mesh's dynamic flag, palette and vertex count, and every
//! vertex's position, bone indices and weights as stored. Then the parts assembled (`assemble-character`): the
//! assembled skeleton against a hierarchy and rests rebuilt from the oracle's own bone records, and every part's
//! vertices at the assembled rest against their own rest.
use std::collections::{BTreeMap, BTreeSet};

use fromsoft_formats::dcx::Decoder;
use primase_animation::deform::{Vertices, skin};
use primase_animation::pose::{Affine, locals, model_transforms, transform};
use primase_core::frame::{Axis, SourceFrame};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Asset, Entity, JointMap, Mesh, Name, Role, Skeleton, Skin, WeightRule, World};
use primase_fromsoft::rigid::SourceModel;
use primase_testdata::{Game, OracleSet, Tool, install, oodle, oracle};
mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The Wolf's parts, in the order the old extractor composed them.
const PARTS: [&str; 5] = [
    "am_m_9120_l",
    "bd_m_9120_l",
    "lg_m_9120_l",
    "hd_m_9120_l",
    "fc_m_0220_l",
];

const LOWER: &str = "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}";

fn manager() -> PassManager {
    let mut reg = PassRegistry::new();
    primase_core::register(&mut reg);
    primase_fromsoft::sekiro::passes::register(&mut reg);
    primase_animation::register(&mut reg);
    PassManager::new(reg).with_pass_checks(true)
}

fn run(w: &mut World, pipeline: &str) -> std::result::Result<RunReport, String> {
    let m = manager();
    let mut report = RunReport::default();
    m.run(
        w,
        &m.registry.parse(pipeline).map_err(|e| e.to_string())?,
        &mut report,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}

/// One bone of the oracle's skeleton.json.
struct Bone {
    name: String,
    parent: i64,
    local: [f64; 16],
}

fn oracle_bones(dir: &std::path::Path) -> Result<Vec<Bone>> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("skeleton.json"))?)?;
    rows.iter()
        .map(|b| {
            let local: Vec<f64> = b["localTransform"]
                .as_array()
                .ok_or("localTransform")?
                .iter()
                .map(|x| x.as_f64().unwrap_or(f64::NAN))
                .collect();
            Ok(Bone {
                name: b["name"].as_str().ok_or("name")?.to_owned(),
                parent: b["parent"].as_i64().ok_or("parent")?,
                local: local.try_into().map_err(|_| "sixteen numbers")?,
            })
        })
        .collect()
}

/// `a * b` for System.Numerics matrices (row-major, row vectors: the transform `a` first).
fn row_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    std::array::from_fn(|i| (0..4).map(|k| a[i / 4 * 4 + k] * b[k * 4 + i % 4]).sum())
}

/// The parts of the install read into a world, with the oracle folder.
#[allow(unsafe_code)]
fn read(
    world: &mut World,
) -> Result<Option<(std::path::PathBuf, Vec<primase_fromsoft::sekiro::CharacterReport>)>> {
    let Some(install) = install(Game::Sekiro) else {
        return Ok(None);
    };
    let Some(sekiro) = common::sekiro(&install)? else {
        return Ok(None);
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(None);
    };
    let Some(oracle) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "parts") else {
        return Ok(None);
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll =
        unsafe { primase_foundations::compress::oodle::Oodle::open(dll.path(), primase_foundations::compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let reports =
        primase_fromsoft::sekiro::read_parts_with(&sekiro, &PARTS, &decoder, &Default::default(), world)?.0;
    Ok(Some((oracle, reports)))
}

/// The assets of the world by part name: (skeleton, prototype).
fn assets(world: &World) -> BTreeMap<String, (Entity, Entity)> {
    world
        .iter::<Asset>()
        .map(|(root, a)| {
            let of = |role| {
                a.members
                    .iter()
                    .find(|m| m.1 == role)
                    .map(|m| m.0)
                    .expect("member")
            };
            (
                world.get::<Name>(root).expect("asset name").0.clone(),
                (of(Role::Rig), of(Role::Part)),
            )
        })
        .collect()
}

#[test]
fn parts_read_as_soulsformats_reads_them() -> Result<()> {
    let mut world = World::new();
    let Some((oracle, reports)) = read(&mut world)? else {
        return Ok(());
    };
    let file_frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
    let by_part = assets(&world);
    assert_eq!(by_part.len(), PARTS.len(), "one asset a part");
    assert_eq!(
        by_part.values().map(|a| a.0).collect::<BTreeSet<_>>().len(),
        PARTS.len(),
        "a skeleton of its own each"
    );
    for (report, id) in reports.iter().zip(PARTS) {
        assert_eq!(report.id, id);
        let dir = oracle.join(id);
        let (rig, proto) = by_part[id];
        let skeleton = world.get::<Skeleton>(rig).ok_or("skeleton")?;
        let model = &world.get::<SourceModel>(proto).ok_or("model")?.0;
        let bones: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(dir.join("skeleton.json"))?)?;
        assert_eq!(bones.len(), skeleton.joints.len(), "{id}: bone count");
        assert_eq!(report.bones, bones.len());
        for (b, j) in bones.iter().zip(&skeleton.joints) {
            assert_eq!(b["name"].as_str(), Some(j.name.as_str()), "{id}: bone name");
            assert_eq!(
                b["parent"].as_i64(),
                Some(j.parent.map_or(-1, |p| i64::from(p.0))),
                "{id}: {}",
                j.name
            );
            let m = file_frame.transform_to_source(&j.rest.matrix());
            let sf: Vec<f64> = b["localTransform"]
                .as_array()
                .ok_or("localTransform")?
                .iter()
                .map(|x| x.as_f64().unwrap_or(f64::NAN))
                .collect();
            for (c, col) in m.cols.iter().enumerate() {
                for (r, &v) in col.iter().enumerate() {
                    let want = sf[c * 4 + r];
                    assert!(
                        (f64::from(v) - want).abs() <= 2e-5,
                        "{id}: {} [{c}][{r}] {v} vs {want}",
                        j.name
                    );
                }
            }
        }
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("meshes.jsonl"))?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        assert_eq!(rows.len(), model.meshes.len(), "{id}: mesh count");
        for (i, (row, mesh)) in rows.iter().zip(&model.meshes).enumerate() {
            assert_eq!(
                row["dynamic"].as_i64() == Some(1),
                mesh.dynamic,
                "{id} mesh {i}: dynamic"
            );
            let palette: Vec<u32> = row["boneIndices"]
                .as_array()
                .ok_or("boneIndices")?
                .iter()
                .map(|x| x.as_u64().unwrap_or(u64::MAX) as u32)
                .collect();
            assert_eq!(palette, mesh.palette, "{id} mesh {i}: palette");
            let bin = std::fs::read(dir.join("meshes").join(format!("{i}.bin")))?;
            let n = u32::from_le_bytes(bin[..4].try_into()?) as usize;
            assert_eq!(n, mesh.vertices.len(), "{id} mesh {i}: vertex count");
            let f = |o: usize| f32::from_le_bytes(bin[o..o + 4].try_into().expect("4 bytes"));
            let int = |o: usize| i32::from_le_bytes(bin[o..o + 4].try_into().expect("4 bytes"));
            for (k, v) in mesh.vertices.iter().enumerate() {
                let o = 4 + k * 60;
                assert_eq!(
                    [f(o), f(o + 4), f(o + 8)],
                    v.position,
                    "{id} mesh {i} vertex {k}: position"
                );
                let idx: [i32; 4] = std::array::from_fn(|c| int(o + 28 + c * 4));
                assert_eq!(
                    idx,
                    v.bone_indices.map(i32::from),
                    "{id} mesh {i} vertex {k}: bone indices"
                );
                let wt: [f32; 4] = std::array::from_fn(|c| f(o + 44 + c * 4));
                for (a, b) in wt.iter().zip(v.bone_weights) {
                    assert!(
                        (a - b).abs() <= 1e-6,
                        "{id} mesh {i} vertex {k}: weights {wt:?} vs {:?}",
                        v.bone_weights
                    );
                }
            }
        }
    }
    // the materials and textures the parts bind are shared in one world, every one read
    let payloads = world
        .resource::<primase_fromsoft::sekiro::source::TexturePayloads>()
        .ok_or("no texture payloads")?;
    let unbound: BTreeSet<&str> = world
        .iter::<primase_fromsoft::materials::Material>()
        .flat_map(|(_, m)| m.bindings.values())
        .filter(|b| !payloads.0.contains_key(&b.texture))
        .map(|b| b.texture.as_str())
        .collect();
    assert!(unbound.is_empty(), "textures bound but not read: {unbound:?}");
    Ok(())
}

/// What the oracle's bone records say the assembly should be: the joint count, each uniquely named joint's parent name
/// and its stored local transform (System.Numerics rows).
type Expected = (
    usize,
    BTreeMap<String, Option<String>>,
    BTreeMap<String, [f64; 16]>,
);

/// The joint count and hierarchy `by-name` should give, from the oracle's bone records alone: one joint a name, except
/// that a name repeated inside a part which no other part holds gives one joint each; its parent the (single) parent
/// name the parts that give one agree on.
fn expected(oracle: &std::path::Path) -> Result<Expected> {
    let skeletons: Vec<(String, Vec<Bone>)> = PARTS
        .iter()
        .map(|p| Ok((p.to_string(), oracle_bones(&oracle.join(p))?)))
        .collect::<Result<_>>()?;
    let mut holders: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (p, (_, bones)) in skeletons.iter().enumerate() {
        for (j, b) in bones.iter().enumerate() {
            holders.entry(&b.name).or_default().push((p, j));
        }
    }
    let mut count = 0;
    let mut parent = BTreeMap::new();
    let mut local = BTreeMap::new();
    for (name, h) in &holders {
        let parts: BTreeSet<usize> = h.iter().map(|x| x.0).collect();
        if parts.len() < h.len() {
            assert_eq!(parts.len(), 1, "{name}: a repeated name another part holds");
            count += h.len();
            continue;
        }
        count += 1;
        let stated: Vec<(usize, usize)> = h
            .iter()
            .copied()
            .filter(|&(p, j)| skeletons[p].1[j].parent >= 0)
            .collect();
        let names: BTreeSet<&str> = stated
            .iter()
            .map(|&(p, j)| skeletons[p].1[skeletons[p].1[j].parent as usize].name.as_str())
            .collect();
        assert!(names.len() <= 1, "{name}: parents {names:?}");
        let (p, j) = stated.first().copied().unwrap_or(h[0]);
        parent.insert((*name).to_owned(), names.iter().next().map(|s| (*s).to_owned()));
        local.insert((*name).to_owned(), skeletons[p].1[j].local);
    }
    Ok((count, parent, local))
}

#[test]
fn the_wolf_assembles_and_its_parts_stay_their_own() -> Result<()> {
    let mut world = World::new();
    let Some((oracle, _)) = read(&mut world)? else {
        return Ok(());
    };
    let file_frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
    run(&mut world, LOWER)?;
    let before: BTreeMap<Entity, Skin> = world
        .iter::<Skin>()
        .map(|(e, s)| (e.to_owned(), s.clone()))
        .collect();
    let by_part = assets(&world);
    for (_, proto) in by_part.values() {
        assert!(before.contains_key(proto), "each part's prototype keeps a skin");
    }
    // the prototype's and its level-of-detail forms' skins each bind to their part's own skeleton
    for s in before.values() {
        assert!(
            by_part
                .values()
                .any(|(rig, _)| s.bindings.iter().all(|b| b.skeleton == *rig))
        );
    }

    // a tolerance of single-precision noise refuses, every disagreeing joint named
    let mut strict = world.clone();
    let err = run(
        &mut strict,
        "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.00001,unweighted-rests=refuse,weighted-rests=refuse,name=wolf}",
    )
    .unwrap_err();
    println!("tolerance 1e-5: {err}");
    assert!(err.contains("beyond tolerance 0.00001"), "{err}");
    assert!(err.contains("joint Head: the rest in "), "{err}");
    assert_eq!(
        strict.iter::<Asset>().count(),
        PARTS.len(),
        "a refusal changes nothing"
    );

    // the face part's Head and the joints under it stand 4.7e-3 from the other parts' (a 0.27 degree turn): with
    // weighted-rests=report the Wolf with his face is assembled at the default tolerance, the disagreements named and
    // counted, the face's displacement reported, every part bound as stored
    let mut faced = world.clone();
    let report = run(
        &mut faced,
        "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=wolf}",
    )?;
    let log = &report.steps[0].log;
    assert!(
        log.counts["weighted joint rests beyond the tolerance, accepted by weighted-rests=report"] > 0,
        "{:?}",
        log.counts
    );
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("joint Head: the rest in fc_m_0220_l differs")
                && k.contains("beyond tolerance 0.001")),
        "{:?}",
        log.approximations
    );
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("vertices of fc_m_0220_l sit up to ")),
        "{:?}",
        log.approximations
    );
    for (e, s) in &before {
        assert_eq!(faced.get::<Skin>(*e), Some(s));
    }
    assert_eq!(faced.verify_components(), vec![]);
    // rebound to the assembled skeleton the face keeps its own inverse binds beyond the tolerance, as the game poses it
    let mut rebound = world.clone();
    let report = run(
        &mut rebound,
        "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=wolf}",
    )?;
    let log = &report.steps[0].log;
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("joint Head: the inverse binds of fc_m_0220_l kept as the part's own")),
        "{:?}",
        log.approximations
    );
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("vertices of fc_m_0220_l sit up to ")
                && k.ends_with("where the part's own inverse binds are kept")),
        "{:?}",
        log.approximations
    );
    assert_eq!(rebound.verify_components(), vec![]);
    // and weighted-rests=refuse refuses the same joint at that tolerance, as before
    let mut refused = world.clone();
    let err = run(
        &mut refused,
        "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,weighted-rests=refuse,name=wolf}",
    )
    .unwrap_err();
    assert!(
        err.contains("joint Head: the rest in fc_m_0220_l differs"),
        "{err}"
    );

    // the same name is not the same joint for a rest 5 mm apart in a rotation entry: refused at 1e-3, accepted at 1e-2
    let mut own = world.clone();
    let report = run(
        &mut own,
        "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.01,unweighted-rests=report,weighted-rests=refuse,name=wolf}",
    )?;
    let log = &report.steps[0].log;
    println!("own counts: {:?}", log.counts);
    for (what, n) in &log.approximations {
        println!("own approximation ({n}): {what}");
    }
    assert_eq!(own.verify_components(), vec![]);
    let (count, parent, local) = expected(&oracle)?;
    assert_eq!(
        log.counts["assembled joints"], count as u64,
        "joint count against the oracle's bone records"
    );
    let (root, asset) = own.iter::<Asset>().next().ok_or("asset")?;
    assert_eq!(own.iter::<Asset>().count(), 1);
    assert_eq!(own.get::<Name>(root), Some(&Name("wolf".into())));
    let rig = asset.members[0].0;
    assert_eq!(asset.members[0].1, Role::Rig);
    let assembled = own.get::<Skeleton>(rig).ok_or("assembled skeleton")?;
    // parts keep their skeletons and bindings exactly
    for (e, s) in &before {
        assert_eq!(own.get::<Skin>(*e), Some(s));
    }
    for (rig_part, _) in by_part.values() {
        assert!(asset.members.contains(&(*rig_part, Role::Rig)));
    }
    assert_eq!(own.iter::<JointMap>().count(), PARTS.len());
    // the hierarchy against the oracle: every uniquely named joint has the parent name the oracle's parted records state
    let names: BTreeMap<u32, &str> = assembled
        .joints
        .iter()
        .map(|j| (j.id.0, j.name.as_str()))
        .collect();
    for j in &assembled.joints {
        if let Some(want) = parent.get(&j.name) {
            assert_eq!(
                j.parent.map(|p| names[&p.0].to_owned()),
                *want,
                "parent of {}",
                j.name
            );
        }
    }
    // and the rests against the oracle's local transforms composed along that hierarchy, in the file's frame
    let rests = model_transforms(assembled, &locals(assembled, None))?;
    let mut oracle_world: BTreeMap<String, [f64; 16]> = BTreeMap::new();
    fn world_of(
        name: &str,
        parent: &BTreeMap<String, Option<String>>,
        local: &BTreeMap<String, [f64; 16]>,
        out: &mut BTreeMap<String, [f64; 16]>,
    ) -> [f64; 16] {
        if let Some(m) = out.get(name) {
            return *m;
        }
        let m = match &parent[name] {
            Some(p) => row_mul(&local[name], &world_of(p, parent, local, out)),
            None => local[name],
        };
        out.insert(name.to_owned(), m);
        m
    }
    let mut worst = 0.0f64;
    let mut compared = 0;
    for j in &assembled.joints {
        if !local.contains_key(&j.name) {
            continue;
        }
        let want = world_of(&j.name, &parent, &local, &mut oracle_world);
        let have = file_frame.transform_to_source(&transform(&rests[&j.id]));
        for (c, col) in have.cols.iter().enumerate() {
            for (r, &v) in col.iter().enumerate() {
                worst = worst.max((f64::from(v) - want[c * 4 + r]).abs());
            }
        }
        compared += 1;
    }
    println!("assembled model rests against the oracle's: {compared} joints, max difference {worst:.3e}");
    assert!(
        worst <= 1e-3,
        "assembled rests differ from the oracle's by {worst}"
    );

    // every part's vertices at the assembled rest against their own rest, with the part rebound to the assembled skeleton
    let mut assembled_world = world.clone();
    let report = run(
        &mut assembled_world,
        "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.01,unweighted-rests=report,weighted-rests=refuse,name=wolf}",
    )?;
    for (what, n) in &report.steps[0].log.approximations {
        println!("assembled approximation ({n}): {what}");
    }
    assert_eq!(assembled_world.verify_components(), vec![]);
    let (_, asset) = assembled_world.iter::<Asset>().next().ok_or("asset")?;
    let new_rig = asset.members[0].0;
    let new_skeleton = assembled_world.get::<Skeleton>(new_rig).ok_or("skeleton")?;
    let new_rests = model_transforms(new_skeleton, &locals(new_skeleton, None))?;
    let (mut max_all, mut max_stored) = (0.0f64, 0.0f64);
    let rule = WeightRule::EngineNormalizes { floor: 0.001 };
    for (id, (rig_part, _)) in &by_part {
        let own_skeleton = world.get::<Skeleton>(*rig_part).ok_or("part skeleton")?;
        let own_rests: BTreeMap<_, Affine> = model_transforms(own_skeleton, &locals(own_skeleton, None))?;
        let (mut max, mut vertices, mut skins) = (0.0f64, 0usize, 0usize);
        // the part's prototype and its level-of-detail forms
        for proto in before
            .iter()
            .filter(|(_, s)| s.bindings[0].skeleton == *rig_part)
            .map(|(e, _)| e)
        {
            let was = world.get::<Skin>(*proto).ok_or("skin")?;
            let now = assembled_world.get::<Skin>(*proto).ok_or("skin")?;
            assert_eq!(now.bindings[0].skeleton, new_rig, "{id}");
            let mesh_was = world.get::<Mesh>(*proto).ok_or("mesh")?;
            let mesh_now = assembled_world.get::<Mesh>(*proto).ok_or("mesh")?;
            assert_eq!(mesh_was, mesh_now, "{id}: vertices untouched");
            skins += 1;
            for (i, prim) in mesh_now.primitives().iter().enumerate() {
                let Some(how) = now.influences[i] else { continue };
                // the stored weights sum to just under 1: compare under the engine's rule so the sums cancel
                let v = Vertices::of(prim)?;
                let at_own = skin(
                    &v,
                    prim,
                    how,
                    rule,
                    was.bindings[0].deformation,
                    was.bindings[0].palettes[i].as_ref().ok_or("palette")?,
                    &own_rests,
                )?;
                let at_assembled = skin(
                    &v,
                    prim,
                    how,
                    rule,
                    now.bindings[0].deformation,
                    now.bindings[0].palettes[i].as_ref().ok_or("palette")?,
                    &new_rests,
                )?;
                for (a, b) in at_own.positions.iter().zip(&at_assembled.positions) {
                    for k in 0..3 {
                        max = max.max(f64::from(a[k] - b[k]).abs());
                    }
                }
                // and the part's own rest puts each vertex where the file stores it (the reader's and the lowering's
                // skeleton, palette and inverse binds agree)
                for (a, b) in at_own.positions.iter().zip(&v.positions) {
                    for k in 0..3 {
                        max_stored = max_stored.max(f64::from(a[k] - b[k]).abs());
                    }
                }
                vertices += v.positions.len();
            }
        }
        println!(
            "{id}: {skins} skins, {vertices} vertices, max distance from their own rest at the assembled rest {max:.3e}"
        );
        assert!(max <= 1e-5, "{id}: {max}");
        max_all = max_all.max(max);
    }
    println!("max distance of a vertex at its own rest from the position the file stores {max_stored:.3e}");
    assert!(max_stored <= 1e-5, "{max_stored}");
    println!("max over all parts {max_all:.3e}");
    Ok(())
}
