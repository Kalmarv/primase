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
//!
//! The expected values were measured on the same files with an independent script (a minimal TAG0 reader and a FLVER
//! bone reader in Python, over the part binders as the archives hold them and the corpus' copy of their cloth members).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::{Asset, Component, JointId, Role, Skeleton, World};
use primase_fromsoft::sekiro::{CharacterOptions, CharacterReport, read_parts_with};
use primase_havok::cloth::HavokCloth;

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
    assert_eq!(
        cloth.verify(part, w),
        Ok(()),
        "{}: havok.cloth's own invariants",
        want.part
    );
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
        },
    );
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
        },
    );
    Ok(())
}
