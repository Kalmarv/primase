//! A Sekiro character read by itself against SoulsFormats' reading of the same files
//! (`oracles.soulsformats.sekiro/skin`, one folder per character id): every bone's name, parent and local transform
//! (ours converted back to the file's frame, against SoulsFormats' `ComputeLocalTransform`, within 2e-5 for f32 Euler
//! arithmetic), every mesh's dynamic flag, palette and vertex count, and every vertex's position, bone indices and
//! weights as stored (exact). Every texture its materials bind is read (some are kept in other characters' and shared
//! packages). Then the character lowered with its skin verifies: `core.skin` and `core.mesh` hold together.
use std::collections::BTreeSet;

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Skeleton, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::rigid::SourceModel;
use primase_testdata::{Game, OracleSet, Tool, install, oracle, tool};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
#[allow(unsafe_code)]
fn characters_read_as_soulsformats_reads_them() -> Result<()> {
    let Some(install) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(dll) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    let Some(oracle) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "skin") else {
        return Ok(());
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let file_frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
    let mut ids: Vec<String> = std::fs::read_dir(&oracle)?
        .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    ids.sort();
    assert!(!ids.is_empty(), "no character folders in the oracle");
    for id in ids {
        let dir = oracle.join(&id);
        let mut world = World::new();
        let report = primase_fromsoft::sekiro::read_character(&install, &id, &decoder, &mut world)?;
        // every texture its materials bind is read, wherever the game keeps it
        let payloads = world
            .resource::<primase_fromsoft::sekiro::source::TexturePayloads>()
            .ok_or("no texture payloads")?;
        let unbound: BTreeSet<&str> = world
            .iter::<primase_fromsoft::materials::Material>()
            .flat_map(|(_, m)| m.bindings.values())
            .filter(|b| !payloads.0.contains_key(&b.texture))
            .map(|b| b.texture.as_str())
            .collect();
        assert!(
            unbound.is_empty(),
            "{id}: textures bound but not read: {unbound:?}"
        );
        let (_, skeleton) = world.iter::<Skeleton>().next().ok_or("no skeleton")?;
        let model = &world.iter::<SourceModel>().next().ok_or("no model")?.1.0;
        // bones
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
        // meshes and vertices
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
        // lowered with its skin, the world holds together
        let mut r = primase_core::pass::RunReport::default();
        let mut reg = primase_core::pass::PassRegistry::new();
        primase_fromsoft::sekiro::passes::register(&mut reg);
        let manager = primase_core::pass::PassManager::new(reg).with_pass_checks(true);
        let pipeline = manager.registry.parse(
            "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
        )?;
        manager.run(&mut world, &pipeline, &mut r)?;
        assert_eq!(
            r.steps[0].log.counts.get("prototypes lowered with their skin"),
            Some(&1),
            "{id}"
        );
        assert_eq!(world.verify_components(), vec![], "{id}");
    }
    Ok(())
}

/// Each part a writer was given: its asset roots' sources, and whether it holds a skin.
type Parts = Vec<(Vec<String>, bool)>;

/// A writer that keeps what it is given ([`Parts`]).
struct Kept(std::rc::Rc<std::cell::RefCell<Parts>>);

impl primase_core::convert::Writer for Kept {
    fn target(&self) -> primase_core::pass::Target {
        primase_core::pass::Target::new("kept").carry_unknown()
    }
    fn write(&mut self, part: &World) -> primase_core::convert::Result<()> {
        let sources = part
            .iter::<primase_core::Asset>()
            .filter_map(|(e, _)| part.get::<primase_core::Source>(e).map(|s| s.0.clone()))
            .collect();
        let skinned = part.iter::<primase_core::Skin>().next().is_some();
        self.0.borrow_mut().push((sources, skinned));
        Ok(())
    }
    fn finish(self: Box<Self>) -> primase_core::convert::Result<primase_core::convert::Finished> {
        Ok(Vec::new().into())
    }
}

/// The character through a conversion, as a program writing one file per object runs it: read (the asset verifies
/// with its part still the source's model), divided by asset (one part), lowered with its skin and placed in the
/// part's passes.
#[test]
#[allow(unsafe_code)]
fn a_character_converts_as_one_part_per_asset() -> Result<()> {
    let Some(install) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(dll) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut reg = primase_core::pass::PassRegistry::new();
    primase_core::register(&mut reg);
    primase_fromsoft::sekiro::passes::register(&mut reg);
    let manager = primase_core::pass::PassManager::new(reg).with_pass_checks(true);
    let kept = std::rc::Rc::default();
    primase_core::convert::convert_parts(
        &mut primase_fromsoft::sekiro::CharacterReader::new(primase_fromsoft::Install::sekiro(install), "c1010", decoder),
        &mut primase_core::convert::PlannedParts::default(),
        Box::new(Kept(std::rc::Rc::clone(&kept))),
        &manager,
        &manager.registry.parse("partition-by-asset")?,
        &manager.registry.parse(
            "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}, place-assets",
        )?,
    )
    .map_err(|e| e as Box<dyn std::error::Error>)?;
    assert_eq!(*kept.borrow(), [(vec!["chr/c1010".to_owned()], true)]);
    Ok(())
}

/// The angle between two directions, in radians (0 when either has no length).
fn angle(a: [f32; 3], b: [f32; 3]) -> f64 {
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    let dot: f64 = (0..3).map(|i| a[i] * b[i]).sum();
    let la = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let lb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if la == 0.0 || lb == 0.0 {
        return 0.0;
    }
    (dot / (la * lb)).clamp(-1.0, 1.0).acos()
}

/// A character's finest geometry against SoulsFormats' reading (`oracles.soulsformats.sekiro/skin`): the face set each
/// mesh is lowered from is the one SoulsFormats flags as level 0 without motion blur, with as many triangles as its
/// index list holds, and every lowered vertex keeps its stored normal (mirrored to the world frame) to within f32
/// rounding, through `lower-fromsoft-geometry` and `select-lod{form=finest}`.
#[test]
#[allow(unsafe_code)]
fn the_finest_face_set_and_the_stored_normals_survive_lowering() -> Result<()> {
    let Some(install) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(dll) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    let Some(oracle) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "skin") else {
        return Ok(());
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut ids: Vec<String> = std::fs::read_dir(&oracle)?
        .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    ids.sort();
    for id in ids {
        let dir = oracle.join(&id);
        let mut world = World::new();
        primase_fromsoft::sekiro::read_character(&install, &id, &decoder, &mut world)?;
        let model = world.iter::<SourceModel>().next().ok_or("no model")?.1.0.clone();
        let mut r = primase_core::pass::RunReport::default();
        let mut reg = primase_core::pass::PassRegistry::new();
        primase_core::register(&mut reg);
        primase_fromsoft::sekiro::passes::register(&mut reg);
        let manager = primase_core::pass::PassManager::new(reg).with_pass_checks(true);
        let pipeline = manager.registry.parse(
            "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}, select-lod{form=finest}",
        )?;
        manager.run(&mut world, &pipeline, &mut r)?;
        let (_, mesh) = world
            .iter::<primase_core::Mesh>()
            .next()
            .ok_or("no lowered mesh")?;
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("meshes.jsonl"))?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        assert_eq!(
            mesh.primitives().len(),
            rows.len(),
            "{id}: one primitive per mesh"
        );
        let (mut stored_triangles, mut lowered_triangles) = (0usize, 0usize);
        let (mut worst, mut worst_at) = (0f64, String::new());
        for (i, (row, p)) in rows.iter().zip(mesh.primitives()).enumerate() {
            // SoulsFormats' level-0 face sets: no level bits, no motion blur
            let level0: Vec<&serde_json::Value> = row["faceSets"]
                .as_array()
                .ok_or("faceSets")?
                .iter()
                .filter(|f| f["flags"].as_u64().is_some_and(|f| f & 0x8300_0000 == 0))
                .collect();
            assert_eq!(level0.len(), 1, "{id} mesh {i}: level 0 face sets");
            let indices = level0[0]["indexCount"].as_u64().ok_or("indexCount")? as usize;
            assert_eq!(
                level0[0]["triangleStrip"].as_bool(),
                Some(false),
                "{id} mesh {i}: a strip needs its own count"
            );
            let triangles = p.indices.as_ref().ok_or("indices")?.len() / 3;
            assert_eq!(triangles, indices / 3, "{id} mesh {i}: triangles");
            stored_triangles += indices / 3;
            lowered_triangles += triangles;
            // normals: stored (file frame) mirrored in Z against lowered, vertex by vertex
            let face_set = model.meshes[i]
                .faces
                .iter()
                .position(|f| f.is_lod0())
                .ok_or("no level 0")?;
            let part = model.decode(i, face_set)?.ok_or("empty part")?;
            let Some(primase_core::Stream::F32x3(lowered)) = p.streams.get(&primase_core::Semantic::Normal)
            else {
                return Err(format!("{id} mesh {i}: no normals").into());
            };
            assert_eq!(
                lowered.len(),
                part.source_vertices.len(),
                "{id} mesh {i}: vertices"
            );
            let bin = std::fs::read(dir.join("meshes").join(format!("{i}.bin")))?;
            let f = |o: usize| f32::from_le_bytes(bin[o..o + 4].try_into().expect("4 bytes"));
            for (k, n) in lowered.iter().enumerate() {
                let o = 4 + part.source_vertices[k] as usize * 60 + 12;
                let a = angle([f(o), f(o + 4), -f(o + 8)], *n);
                if a > worst {
                    (worst, worst_at) = (a, format!("mesh {i} vertex {k}"));
                }
            }
        }
        eprintln!(
            "{id}: {lowered_triangles} of {stored_triangles} level-0 triangles; largest normal angle {worst:.3e} rad at {worst_at}"
        );
        assert_eq!(lowered_triangles, stored_triangles, "{id}");
        assert!(worst < 1e-3, "{id}: {worst_at}: {worst} rad");
    }
    Ok(())
}
