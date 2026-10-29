//! The alpha tests `bake-fromsoft-materials` carries from a program's own compare, on Elden Ring's Limgrave tile
//! `m60_42_36_00` read and prepared as the lab prepares an Elden Ring area for its Shaded view (the shaders resolved, the
//! materials lowered with their layers kept, then the bake).
//!
//! The bake carries a compare only where it is the program's discard's own test: in a run of the program with its test
//! on (the material's reference, as the Game shader view draws it), the program discards exactly the texels whose value
//! is below the reference. `aet000_102_a` (`M[AMSN_V][Mb4][3Mask][Ov_N2][beach]_Gbuf_[A]`, `g_AlphaRef` 255) discards
//! where `FC_AlphaRef.y * (dither - 0.5) + 1 < FC_AlphaRef.x`: no texture of its surface reaches the value, which is 1
//! at every texel with the dither at its neutral grey, and the program keeps every texel there, as the Game shader view
//! does. It is drawn opaque, not as a mask at 1 that a renderer may cut whole. The foliage and ivy cards whose compare
//! reads their textures (`aet801_270_a`, `aet030_075_a`) stay masks carried from their compare.
mod common;
mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use iridia_interface::Stage;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::shader::{ConstantRole, ShaderBinding};
use primase_core::{Entity, Mesh, Name, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::passes::bake_materials::{
    CARRIED_ALPHA_TEST, UNCARRIED_ALPHA_TEST, UNCUT_ALPHA_TEST,
};
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The lab's preparation of an Elden Ring area for the Shaded view, its bake at 16 pixels: whether a compare is the
/// discard's test, and whether it is one value over the surface, do not depend on the size.
const PASSES: [&str; 6] = [
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
    "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    "bake-fromsoft-materials{max-size=16,sample-time=0}",
];

/// Whether a material's pixel program tests its alpha: its binding states a reference above 0.
fn alpha_tested(world: &World, material: Entity) -> bool {
    world
        .get::<ShaderBinding>(material)
        .and_then(|b| b.stage(Stage::Pixel))
        .is_some_and(|s| {
            s.constants
                .iter()
                .any(|c| matches!(c.role, ConstantRole::AlphaReference { value, .. } if value > 0.))
        })
}

/// The drawn materials named `name` whose program tests its alpha, and their alpha modes.
fn named(world: &World, name: &str) -> Vec<(Entity, AlphaMode)> {
    let drawn: BTreeSet<Entity> = world
        .iter::<Mesh>()
        .flat_map(|(_, m)| m.primitives().iter().filter_map(|p| p.material))
        .collect();
    world
        .iter::<PbrMaterial>()
        .filter(|&(e, _)| {
            drawn.contains(&e) && alpha_tested(world, e) && world.get::<Name>(e).is_some_and(|n| n.0 == name)
        })
        .map(|(e, p)| (e, p.alpha_mode))
        .collect()
}

#[test]
fn the_carried_alpha_test_is_the_programs_discard() -> Result<()> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::default();
    sekiro::read_game_area(
        &install,
        "m60_42_36_00",
        &decoder,
        &sekiro::Options::default(),
        &mut world,
    )?;
    let mut bake = PassLog::default();
    for pass in PASSES {
        for step in common::run(&mut world, pass)?.steps {
            if step.pass.starts_with("bake-fromsoft-materials") {
                bake = step.log;
            }
        }
    }
    let on = |reason: &str, e: Entity| bake.approximated_on.get(reason).is_some_and(|es| es.contains(&e));
    // the beach program's compare: one value, kept at the reference everywhere, so no cut
    let beach = named(&world, "aet000_102_a");
    assert!(!beach.is_empty(), "aet000_102_a is not in m60_42_36_00");
    for &(e, mode) in &beach {
        assert_eq!(mode, AlphaMode::Opaque, "aet000_102_a");
        assert!(
            on(UNCUT_ALPHA_TEST, e),
            "aet000_102_a: the bake does not find its test keeps every texel"
        );
    }
    // the cards whose compare reads their textures: masks at their reference, carried from the compare
    for name in ["aet801_270_a", "aet030_075_a"] {
        let cards = named(&world, name);
        assert!(!cards.is_empty(), "{name} is not in m60_42_36_00");
        for &(e, mode) in &cards {
            assert!(matches!(mode, AlphaMode::Mask { .. }), "{name}: {mode:?}");
            assert!(
                on(CARRIED_ALPHA_TEST, e),
                "{name}: its cut is not carried from its compare"
            );
        }
    }
    // every compare the bake met in the tile is its program's discard's test
    let untied: Vec<&String> = bake
        .approximated_on
        .keys()
        .filter(|r| r.starts_with(UNCARRIED_ALPHA_TEST) && r.contains("is not its discard's test"))
        .collect();
    assert!(untied.is_empty(), "{untied:#?}");
    Ok(())
}
