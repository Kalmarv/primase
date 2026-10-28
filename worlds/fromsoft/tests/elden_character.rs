//! An Elden Ring character read by itself (`installs.elden`, `keys.elden`, `tools.oodle6`): the one character reader of
//! the crate, given the game's install, finds the model in `chr/<id>.chrbnd`, its textures in the high and low
//! resolution texture binders (`chr/<id>_h.texbnd`, `chr/<id>_l.texbnd`) and resolves its materials against the game's
//! material binaries, as it does Sekiro's against MTDs.
//!
//! - E1: `c2010` reads: its FLVER's 356 bones, 16 meshes and 16 materials (a census of every character's FLVER read
//!   them all as version 0x2001A), the textures its materials bind found in the texture binders, an asset holding its
//!   skeleton as the rig and its prototype as the part.
//! - E2: the notices say what the read left out (the Havok skeleton and animation binder when animations are not
//!   asked for).
//! - E4: an alpha-tested card (c5840's wing feathers, `C[Fur]`) is drawn with the slot its pixel program tests
//!   (`c5840_feather_a`, sampled with a later UV pair of the mesh than the first and multiplied by the first vertex colour's alpha),
//!   not the slot the preview colours with (`c5840_feather_color_a`, whose alpha is 255 everywhere); the meshes keep
//!   their UV sets and first colour.
//! - E3: the textures a material binds from the parts folder (`AAT100_Crystal_a`, `AAT500_Hair_02_a`: the matbin's
//!   sampler paths `N:\GR\data\Model\parts\tex\...`) are in `parts/common_body.tpf`, which every character reads: none
//!   of them is unavailable on `c2200` (its backface and silhouette shells name only `AAT100_Crystal_*`) and `c2010`
//!   (the fur cards' strand map is `AAT500_Hair_02_a`).
//! - E4: the textures of the assets a character's materials bind (`aet210_112_a`: the package
//!   `asset/aet/aet210/aet210_112.tpf` the game names after the texture) are read: every `aet` binding of `c8100`, `c8101`
//!   and `c8110` that has a package in the archives is available.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::{Asset, Role, Skeleton, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::{CharacterOptions, read_character_with};

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn an_elden_ring_character_reads_through_the_one_character_reader() -> Result<()> {
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
    let report = read_character_with(&install, "c2010", &decoder, CharacterOptions::default(), &mut w)?;
    println!(
        "{}: {} bones, {} meshes, {} materials, {} textures; unread {:?}; missing shader bundles {:?}",
        report.id,
        report.bones,
        report.meshes,
        report.materials,
        report.textures,
        report.unread,
        report.missing_shader_bundles
    );
    // E1
    assert_eq!((report.bones, report.meshes), (356, 16));
    assert!(
        report.materials > 0 && report.materials <= 16,
        "{} materials",
        report.materials
    );
    assert!(report.textures > 0, "no texture was found in the texture binders");
    let (root, asset) = w
        .iter::<Asset>()
        .next()
        .map(|(e, a)| (e, a.members.clone()))
        .unwrap();
    let _ = root;
    assert_eq!(asset.iter().filter(|(_, role)| *role == Role::Rig).count(), 1);
    assert_eq!(asset.iter().filter(|(_, role)| *role == Role::Part).count(), 1);
    let (rig, _) = asset.iter().find(|(_, role)| *role == Role::Rig).unwrap();
    assert_eq!(w.get::<Skeleton>(*rig).unwrap().joints.len(), 356);
    // E2
    assert!(
        report.unread.iter().any(|f| f.ends_with(".anibnd")),
        "{:?}",
        report.unread
    );
    Ok(())
}

/// `c8002` (a creature) has two helper joints at its root scaled to nothing (`MoveSampling_...`,
/// `MasterRig_MoveSampling_UpperLimit`): their rests have no inverse. No vertex weights them, so the geometry lowers
/// with the identity held as their inverse bind, counted.
#[test]
fn joints_scaled_to_nothing_hold_the_identity_when_no_vertex_weights_them() -> Result<()> {
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
    read_character_with(&install, "c8002", &decoder, CharacterOptions::default(), &mut w)?;
    let report = common::run(
        &mut w,
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    )?;
    let counts = &report.steps[0].log.counts;
    assert_eq!(
        counts["joints whose rest has no inverse (a bone scaled to nothing), holding the identity as inverse bind: no vertex weights them"],
        2
    );
    Ok(())
}

/// `c8000`, a character with no model of its own (0 meshes, a skeleton and animations only): its geometry lowers to
/// nothing, the prototype is dropped with the asset's mention of it, and the world still verifies.
#[test]
fn a_character_with_no_model_lowers_to_nothing_and_its_asset_stays_whole() -> Result<()> {
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
    read_character_with(&install, "c8000", &decoder, CharacterOptions::default(), &mut w)?;
    let report = common::run(
        &mut w,
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    )?;
    let counts = &report.steps[0].log.counts;
    assert_eq!(counts["prototypes left drawing nothing"], 1);
    assert_eq!(counts["asset members dropped with the entity they listed"], 1);
    let (_, asset) = w.iter::<Asset>().next().unwrap();
    assert!(asset.members.iter().all(|(e, _)| w.is_alive(*e)));
    assert!(asset.members.iter().all(|(_, role)| *role != Role::Part));
    Ok(())
}

/// E3: the parts folder's textures a character's materials bind are found.
#[test]
fn the_parts_folder_textures_of_a_character_are_found_in_the_common_body_package() -> Result<()> {
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
    for id in ["c2200", "c2010"] {
        let mut w = World::new();
        read_character_with(&install, id, &decoder, CharacterOptions::default(), &mut w)?;
        let mut parts = 0;
        for (_, m) in w.iter::<Material>() {
            for (kind, b) in &m.bindings {
                if b.texture.starts_with("aat100_") || b.texture.starts_with("aat500_") {
                    parts += 1;
                    assert!(
                        b.available,
                        "{id}: {kind} binds {}, which no package held",
                        b.texture
                    );
                }
            }
        }
        assert!(parts > 0, "{id} binds no parts folder texture");
    }
    Ok(())
}

/// E4: a texture named for an asset is read from that asset's package.
#[test]
fn the_asset_textures_a_character_binds_are_read_from_their_packages() -> Result<()> {
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
    let archive = install.open()?;
    for id in ["c8100", "c8101", "c8110"] {
        let mut w = World::new();
        read_character_with(&install, id, &decoder, CharacterOptions::default(), &mut w)?;
        let mut assets = 0;
        for (_, m) in w.iter::<Material>() {
            for (kind, b) in &m.bindings {
                let Some(package) = Game::EldenRing.texture_package(&b.texture) else {
                    continue;
                };
                if archive.contains(&package) {
                    assets += 1;
                    assert!(
                        b.available,
                        "{id}: {kind} binds {}, whose package {package} was not read",
                        b.texture
                    );
                }
            }
        }
        assert!(assets > 0, "{id} binds no asset texture");
    }
    Ok(())
}

/// E5: the cut-out of a fur card is in the slot its program tests, on the UV set the program samples it with.
#[test]
fn an_alpha_tested_card_is_drawn_with_the_slot_its_program_tests() -> Result<()> {
    use primase_core::pbr::{AlphaMode, PbrMaterial};
    use primase_core::{Mesh, Name, Semantic, VertexColor};
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
    read_character_with(&install, "c5840", &decoder, CharacterOptions::default(), &mut w)?;
    common::run(
        &mut w,
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program},lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    )?;
    let wing: Vec<_> = w
        .iter::<PbrMaterial>()
        .filter(|(_, p)| {
            p.base_color_texture
                .is_some_and(|t| w.get::<Name>(t.texture).is_some_and(|n| n.0 == "c5840_feather_a"))
        })
        .map(|(e, p)| (e, p.base_color_texture.unwrap().texcoord, p.alpha_mode))
        .collect();
    assert!(
        !wing.is_empty(),
        "no material is drawn with the feather coverage texture"
    );
    for (e, texcoord, alpha) in wing {
        // not the first set: the cut-out is on a later UV pair of the vertex (`C[Fur]_Arts`: `TEXCOORD0.zw`)
        assert!(texcoord > 0);
        assert!(matches!(alpha, AlphaMode::Mask { .. }));
        // the colour is multiplied by the vertex colour only where the program's alpha test reads it
        assert!(w.get::<VertexColor>(e).is_none());
        let drawn = w
            .iter::<Mesh>()
            .map(|(_, m)| m.primitives().iter().filter(|p| p.material == Some(e)).count())
            .sum::<usize>();
        assert!(drawn > 0);
        for (_, m) in w.iter::<Mesh>() {
            for p in m.primitives().iter().filter(|p| p.material == Some(e)) {
                assert!(p.streams.contains_key(&Semantic::TexCoord(texcoord)));
                assert!(p.streams.contains_key(&Semantic::Color(0)));
            }
        }
    }
    Ok(())
}
