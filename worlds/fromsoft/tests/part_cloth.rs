//! A part's Havok cloth (`installs.sekiro`, `installs.elden`, `keys.sekiro`, `keys.elden`, `tools.oodle6`): the part
//! reader takes the part's cloth tagfile (`<part>_c_l.hkx` of Sekiro's `_l` parts, `<part>_c.hkx` of Elden Ring's) and
//! puts it on the part's prototype as `havok.cloth`, following the part's skeleton.
//!
//! - C1: Sekiro's `bd_m_9040_l` (the Wolf's scarf and robes): 7 cloth data, the transform sets' skeleton of 146 bones,
//!   every bone joined to the part skeleton's joint of the same name (the FLVER's 173 bones hold all 146 names).
//! - C2: Elden Ring's `bd_m_1360_l` (a samurai armour): 11 cloth data, 136 bones, every bone joined (the FLVER's 177).
//! - C3: the joint of a bone is the FLVER bone of its name, by index in the FLVER (Sekiro: `Spine2` is bone 44 of the
//!   cloth skeleton and joint 6, `Neck` 79 and 20, `Head` 80 and 21; Elden Ring: `Master` 0 and 82, `Neck` 86 and 29,
//!   the cloth-driven `[cloth]BD_M_1360_BD_PelvisArmor_003` 9 and 58).
//! - C4: every display buffer is bound to its FLVER mesh (Sekiro's 17, Elden Ring's 6), none left unbound: by cloth data
//!   and buffer, the FLVER mesh and its vertex count.
//! - C5: lowered (`lower-fromsoft-geometry`, the stored pose), each level-0 primitive of a bound mesh says which display
//!   buffer vertex each of its vertices is (`havok.cloth_vertices`), and the cloth's own display skin operators place
//!   those display vertices where the primitive's vertices are: their local positions (the packed bind-pose positions)
//!   are the primitive's positions in the file's frame within 1e-4 of the mesh's size.
//!
//! The expected values were measured on the same files with an independent script (a minimal TAG0 reader and a FLVER
//! reader in Python, over the part binders as the archives hold them and the corpus' copy of their cloth members); C5's
//! bound is that script's census over both installs (every display skin operator's local positions are its bound mesh's
//! vertex positions within 4.9e-5 of the mesh's size).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use havok_formats::cloth::OperatorKind;
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Asset, Component, Entity, JointId, Role, Skeleton, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{CharacterOptions, CharacterReport, read_parts_with};
use primase_havok::cloth::{ClothVertices, HavokCloth};

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// What one part's read must hold of its cloth.
struct Expected<'a> {
    part: &'a str,
    cloth_data: &'a [&'a str],
    bones: usize,
    joints: usize,
    /// (cloth skeleton bone, its name, the part skeleton's joint)
    joined: &'a [(usize, &'a str, u32)],
    /// (cloth data, its display buffer, the FLVER mesh it holds, its vertex count)
    displays: &'a [(&'a str, u32, u32, u32)],
}

fn check(w: &World, report: &CharacterReport, want: &Expected<'_>) {
    println!(
        "{}: havok cloth {} data, {} bones ({} unjoined); unread {:?}",
        report.id,
        report.havok_cloth_data,
        report.havok_cloth_bones,
        report.havok_cloth_bones_unjoined,
        report.unread
    );
    assert_eq!(
        (
            report.havok_cloth_data,
            report.havok_cloth_bones,
            report.havok_cloth_bones_unjoined
        ),
        (want.cloth_data.len(), want.bones, 0),
        "{}",
        want.part
    );
    assert!(
        !report
            .unread
            .iter()
            .any(|f| f.ends_with("_c.hkx") || f.ends_with("_c_l.hkx")),
        "{}: the cloth tagfile is named unread: {:?}",
        want.part,
        report.unread
    );
    let (_, asset) = w.iter::<Asset>().next().unwrap();
    let rig = asset.members.iter().find(|(_, r)| *r == Role::Rig).unwrap().0;
    let part = asset.members.iter().find(|(_, r)| *r == Role::Part).unwrap().0;
    let cloth = w
        .get::<HavokCloth>(part)
        .expect("havok.cloth on the part's prototype");
    assert_eq!(cloth.skeleton, rig, "the cloth follows the part's skeleton");
    assert_eq!(w.get::<Skeleton>(rig).unwrap().joints.len(), want.joints);
    let mut names: Vec<&str> = cloth.physics.cloths.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, want.cloth_data);
    assert_eq!(cloth.bones.bones.len(), want.bones);
    assert_eq!(cloth.joints.len(), want.bones);
    for &(bone, name, joint) in want.joined {
        assert_eq!(cloth.bones.bones[bone].name, name);
        assert_eq!(cloth.joints[bone], Some(JointId(joint)), "{name}");
    }
    // C4
    assert_eq!(report.havok_cloth_displays, want.displays.len(), "{}", want.part);
    assert!(
        report.havok_cloth_displays_unbound.is_empty() && cloth.unbound.is_empty(),
        "{}: {:?}",
        want.part,
        cloth.unbound
    );
    for &(data, buffer, mesh, vertices) in want.displays {
        let d = cloth
            .displays
            .iter()
            .find(|d| cloth.physics.cloths[d.data as usize].name == data && d.buffer == buffer)
            .unwrap_or_else(|| panic!("{}: no display {data} / {buffer}", want.part));
        assert_eq!(d.mesh, mesh, "{data} / {buffer}");
        assert_eq!(
            cloth.display_buffer(d).unwrap().vertices,
            vertices,
            "{data} / {buffer}"
        );
    }
    assert_eq!(
        cloth.verify(part, w),
        Ok(()),
        "{}: havok.cloth's own invariants",
        want.part
    );
}

/// FromSoft's frame in the world's, as primase's FromSoft world reads it: z negated, in metres.
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

/// An `hkPackedVector3` from its four stored words: each the high half of a 32-bit integer, scaled by the float whose
/// high half is the fourth word.
fn unpack(words: [i16; 4]) -> [f32; 3] {
    let scale = f32::from_bits(u32::from(words[3].cast_unsigned()) << 16);
    [0, 1, 2].map(|k| ((i32::from(words[k]) << 16) as f32) * scale)
}

/// Each vertex the object space skin operators writing display buffer `buffer` of cloth data `data` place, with its
/// local position (the bind-pose position, in the file's frame): the deformer's blocks in control byte order, sixteen
/// vertices each, each with the local block of the same rank.
fn display_skin_positions(cloth: &HavokCloth, data: usize, buffer: u32) -> Vec<(u16, [f32; 3])> {
    let mut out = Vec::new();
    for op in &cloth.physics.cloths[data].operators {
        let OperatorKind::ObjectSpaceSkin {
            output_buffer,
            deformer,
            local,
            local_unpacked,
            ..
        } = &op.kind
        else {
            continue;
        };
        if *output_buffer != buffer {
            continue;
        }
        let mut next = [0usize; 8];
        let mut rank = 0;
        for &control in &deformer.control_bytes {
            let bones = match control {
                0 => 4,
                1 => 3,
                2 => 2,
                3 => 1,
                5 => 8,
                6 => 7,
                7 => 6,
                8 => 5,
                _ => continue,
            };
            let block = &deformer.blocks[bones - 1][next[bones - 1]];
            next[bones - 1] += 1;
            for (i, &v) in block.vertices.iter().enumerate() {
                let p = if local.is_empty() {
                    let q = local_unpacked[rank].positions[i];
                    [q[0], q[1], q[2]]
                } else {
                    unpack(local[rank].positions[i])
                };
                out.push((v, p));
            }
            rank += 1;
        }
    }
    out
}

/// C5 on a read world: lowered, each level-0 primitive of a bound mesh holds its display buffer vertices, and the
/// display skin operators' local positions of those vertices are the primitive's positions (in the file's frame) within
/// 1e-4 of the mesh's size. The count of primitives so checked.
fn check_lowered(w: &mut World, part: Entity, displays: usize) -> Result<usize> {
    let mut reg = primase_core::pass::PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut reg);
    let manager = primase_core::pass::PassManager::new(reg).with_pass_checks(true);
    let pipeline = manager.registry.parse(
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    )?;
    let mut r = primase_core::pass::RunReport::default();
    manager.run(w, &pipeline, &mut r)?;
    assert_eq!(
        r.steps[0]
            .log
            .counts
            .get("prototypes whose Havok cloth sets vertices of their meshes"),
        Some(&1)
    );
    assert_eq!(w.verify_components(), vec![]);
    let cloth = w.get::<HavokCloth>(part).unwrap();
    let vertices = w
        .get::<ClothVertices>(part)
        .expect("havok.cloth_vertices on the lowered prototype");
    assert_eq!(vertices.cloth, part);
    let mesh = w.get::<primase_core::Mesh>(part).unwrap();
    let held: Vec<(usize, &primase_havok::cloth::ClothPrimitive)> = vertices
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.as_ref().map(|c| (i, c)))
        .collect();
    // one level-0 face set a mesh: one primitive each display holds
    assert_eq!(held.len(), displays);
    let mut checked = 0;
    for (i, c) in held {
        let d = &cloth.displays[c.display as usize];
        let placed = display_skin_positions(cloth, d.data as usize, d.buffer);
        let positions = mesh.primitives()[i].positions().unwrap();
        let size = positions
            .iter()
            .map(|p| p.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt())
            .fold(0.0, f64::max);
        let mut compared = 0;
        for (k, &v) in c.vertices.iter().enumerate() {
            let Some(&(_, local)) = placed.iter().find(|(w, _)| u32::from(*w) == v) else {
                continue;
            };
            let at = FRAME.point(local);
            let off = (0..3)
                .map(|r| (f64::from(at[r]) - f64::from(positions[k][r])).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                off <= 1e-4 * size,
                "primitive {i} vertex {k} (display vertex {v}): {off} m from the skin's local position, mesh size {size}"
            );
            compared += 1;
        }
        if !placed.is_empty() {
            assert!(compared > 0, "primitive {i}: no vertex compared");
            checked += 1;
        }
    }
    Ok(checked)
}

#[test]
fn a_sekiro_parts_cloth_is_read_onto_its_prototype() -> Result<()> {
    let (Some(dir), Some(dll)) = (install(Data::Sekiro), oodle(Tool::Oodle6)) else {
        return Ok(());
    };
    let Some(sekiro) = common::sekiro(dir)? else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut w = World::new();
    let (reports, _) = read_parts_with(
        &sekiro,
        &["bd_m_9040_l"],
        &decoder,
        &CharacterOptions::default(),
        &mut w,
    )?;
    assert_eq!(reports.len(), 1);
    // C1, C3
    check(
        &w,
        &reports[0],
        &Expected {
            part: "bd_m_9040_l",
            cloth_data: &[
                "BD_M_9040_bodybase02",
                "BD_M_9040_bodybese01",
                "BD_M_9040_bodybese03",
                "BD_M_9040_muffler",
                "BD_M_9040_muffler1",
                "BD_M_9040_muffler2",
                "BD_M_9040_muffler_neck",
            ],
            bones: 146,
            joints: 173,
            joined: &[
                (0, "Master", 0),
                (44, "Spine2", 6),
                (79, "Neck", 20),
                (80, "Head", 21),
            ],
            displays: &[
                ("BD_M_9040_muffler1", 2, 10, 407),
                ("BD_M_9040_muffler1", 3, 22, 47),
                ("BD_M_9040_muffler1", 4, 26, 112),
                ("BD_M_9040_bodybese01", 2, 7, 76),
                ("BD_M_9040_bodybese01", 3, 16, 633),
                ("BD_M_9040_bodybese01", 4, 19, 92),
                ("BD_M_9040_muffler", 2, 9, 48),
                ("BD_M_9040_muffler", 3, 13, 158),
                ("BD_M_9040_muffler", 4, 24, 24),
                ("BD_M_9040_muffler2", 2, 6, 72),
                ("BD_M_9040_muffler2", 3, 11, 383),
                ("BD_M_9040_muffler2", 4, 23, 37),
                ("BD_M_9040_muffler_neck", 0, 12, 172),
                ("BD_M_9040_bodybase02", 2, 17, 583),
                ("BD_M_9040_bodybese03", 2, 8, 70),
                ("BD_M_9040_bodybese03", 3, 18, 389),
                ("BD_M_9040_bodybese03", 4, 20, 89),
            ],
        },
    );
    // C5: every display of the scarf and robes is written by a display skin operator
    let (_, asset) = w.iter::<Asset>().next().unwrap();
    let part = asset.members.iter().find(|(_, r)| *r == Role::Part).unwrap().0;
    assert_eq!(check_lowered(&mut w, part, 17)?, 17);
    Ok(())
}

#[test]
fn an_elden_ring_parts_cloth_is_read_onto_its_prototype() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = common::install(
        dir,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let mut w = World::new();
    let (reports, _) = read_parts_with(
        &install,
        &["bd_m_1360_l"],
        &decoder,
        &CharacterOptions::default(),
        &mut w,
    )?;
    assert_eq!(reports.len(), 1);
    // C2, C3
    check(
        &w,
        &reports[0],
        &Expected {
            part: "bd_m_1360_l",
            cloth_data: &[
                "BD_ArmorRope",
                "BD_Belt",
                "BD_CloakRope",
                "BD_PelvisFrontArmor",
                "BD_PelvisRearArmor",
                "BD_RearRope",
                "BD_ShoulderArmorRope",
                "BD_SideSkirt",
                "BD_Tesset02",
                "BD_Tesset03",
                "BD_Wara",
            ],
            bones: 136,
            joints: 177,
            joined: &[
                (0, "Master", 82),
                (9, "[cloth]BD_M_1360_BD_PelvisArmor_003", 58),
                (86, "Neck", 29),
                (87, "Head", 30),
            ],
            displays: &[
                ("BD_Wara", 0, 11, 2214),
                ("BD_SideSkirt", 2, 9, 258),
                ("BD_SideSkirt", 3, 12, 28),
                ("BD_Belt", 2, 8, 172),
                ("BD_Belt", 3, 13, 32),
                ("BD_CloakRope", 2, 10, 70),
            ],
        },
    );
    // C5
    let (_, asset) = w.iter::<Asset>().next().unwrap();
    let part = asset.members.iter().find(|(_, r)| *r == Role::Part).unwrap().0;
    check_lowered(&mut w, part, 6)?;
    Ok(())
}
