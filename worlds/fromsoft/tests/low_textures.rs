//! Elden Ring's low-resolution texture twins (`Options::low_textures`) on the Limgrave tile `m60_42_36_00`, read and
//! prepared as the lab prepares an Elden Ring area for its Shaded view (the shaders resolved, the materials lowered
//! with their layers kept, then the bake, here at 16 pixels).
//!
//! Without the option the read and the preparation hold nothing of the twins. With it, every read texture either has
//! its twin or is named as having none; the textures made from the sources and the bake's textures carry their twins
//! as forms (`core.texture_forms`), a material baked over its twins only where every texture its program reads has
//! one; and keeping the finest form of every texture (`select-texture-form{form=finest}`) leaves exactly what the read
//! without the option prepares: the same textures, byte for byte, and the same materials binding them.
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Entity, Instance, Mesh, Name, SelectTextureForm, Texture, TextureChoice, TextureForms, World,
};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::LowTexturePayloads;
use sha2::Digest;
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The lab's preparation of an Elden Ring area for the Shaded view, its bake at 16 pixels.
const PASSES: [&str; 6] = [
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
    "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    "bake-fromsoft-materials{max-size=16,sample-time=0}",
];

/// The tile read with or without the twins and prepared: the world, the read's report and the bake's log.
fn prepared(
    install: &fromsoft_formats::Install,
    decoder: &Decoder<'_>,
    low: bool,
) -> Result<(World, sekiro::Report, PassLog)> {
    let mut world = World::default();
    let report = sekiro::read_game_area(
        install,
        "m60_42_36_00",
        decoder,
        &sekiro::Options {
            low_textures: low,
            ..Default::default()
        },
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
    Ok((world, report, bake))
}

/// What a preparation holds, by name rather than entity: each named texture's bytes, each material with the names of
/// the textures it binds, and the meshes and placements counted.
fn held(world: &World) -> (Vec<String>, Vec<String>, [usize; 2]) {
    let name = |e: Entity| {
        world
            .get::<Name>(e)
            .map_or_else(|| format!("{e:?}"), |n| n.0.clone())
    };
    let mut textures: Vec<String> = world
        .iter::<Texture>()
        .map(|(e, t)| {
            let mut hash = sha2::Sha256::new();
            for level in &t.levels {
                hash.update(level);
            }
            format!(
                "{} {}x{} {:?} {:?} {:?} {:?} {} levels {:02x?}",
                name(e),
                t.width,
                t.height,
                t.format,
                t.color_space,
                t.role,
                t.normal,
                t.levels.len(),
                hash.finalize().as_slice()
            )
        })
        .collect();
    textures.sort();
    let placeholder = World::new().spawn();
    let mut materials: Vec<String> = world
        .iter::<PbrMaterial>()
        .map(|(e, m)| {
            let bound: Vec<String> = m
                .slots()
                .map(|(slot, r)| format!("{}={}", slot.name(), name(r.texture)))
                .collect();
            let mut m = m.clone();
            for (_, r) in m.slots_mut() {
                r.texture = placeholder;
            }
            format!("{} {bound:?} {m:?}", name(e))
        })
        .collect();
    materials.sort();
    (
        textures,
        materials,
        [world.iter::<Mesh>().count(), world.iter::<Instance>().count()],
    )
}

#[test]
fn low_twins_are_forms_and_change_nothing_else() -> Result<()> {
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
    let (off, off_report, off_bake) = prepared(&install, &decoder, false)?;
    assert!(
        off.iter::<TextureForms>().next().is_none(),
        "a read without twins makes no form"
    );
    assert!(off.resource::<LowTexturePayloads>().is_none());
    assert_eq!(
        (
            off_report.low_textures,
            off_report.low_texture_packages,
            off_report.missing_low_textures.len()
        ),
        (0, 0, 0)
    );
    assert!(
        !off_bake.counts.keys().any(|k| k.contains("low-resolution")),
        "{:?}",
        off_bake.counts
    );

    let (mut on, report, bake) = prepared(&install, &decoder, true)?;
    println!(
        "textures {}, twins {}, _l packages {} of {} packages, without a twin {:?}; bake {:?}",
        report.textures,
        report.low_textures,
        report.low_texture_packages,
        report.texture_packages,
        report.missing_low_textures,
        bake.counts
            .iter()
            .filter(|(k, _)| k.contains("low-resolution"))
            .collect::<BTreeMap<_, _>>()
    );
    // every texture read has its twin or is named
    assert!(report.low_textures > 0 && report.low_texture_packages > 0);
    assert_eq!(
        report.low_textures + report.missing_low_textures.len(),
        report.textures
    );
    assert_eq!(
        on.resource::<LowTexturePayloads>().map(|p| p.0.len()),
        Some(report.low_textures)
    );
    // each texture's forms hold a texture of their own, and a baked material's textures all have theirs
    let baked = bake
        .counts
        .get("materials baked over their low-resolution twins too")
        .copied()
        .unwrap_or(0);
    assert!(
        baked > 0,
        "no material was baked over its twins: {:?}",
        bake.counts
    );
    let mut forms = 0;
    for (e, f) in on.iter::<TextureForms>() {
        assert_eq!(f.verify(e, &on), Ok(()));
        forms += 1;
    }
    let baked_forms = on
        .iter::<TextureForms>()
        .filter(|(e, _)| on.get::<Name>(*e).is_some_and(|n| n.0.starts_with("baked_")))
        .count();
    assert!(
        baked_forms as u64 >= 3 * baked,
        "{baked} materials baked low, {baked_forms} baked textures with a form"
    );
    println!("textures with a form: {forms}, of them baked: {baked_forms}");
    // the finest form of every texture is the read without twins
    SelectTextureForm {
        choice: TextureChoice::Finest,
    }
    .run(&mut on, &mut PassLog::default())?;
    let (off_held, on_held) = (held(&off), held(&on));
    assert_eq!(off_held.2, on_held.2, "meshes and placements");
    assert!(
        off_held.0 == on_held.0,
        "the textures differ from the read without twins"
    );
    assert!(
        off_held.1 == on_held.1,
        "the materials differ from the read without twins"
    );
    Ok(())
}

/// Sekiro stores a map texture's twin beside it in the area's binders (`<stem>_l`): a read of the baseline region's
/// first cell of `m11_01` with the twins finds them there, and every texture read has its twin or is named.
#[test]
fn sekiro_twins_are_read_from_the_area_binders() -> Result<()> {
    let Some(root) = inputs::install(inputs::Game::Sekiro) else {
        return Ok(());
    };
    let Some(install) = common::sekiro(&root)? else {
        return Ok(());
    };
    let (Some(dll), Some(reference)) = (
        inputs::oodle(inputs::Tool::Oodle6),
        inputs::oracle(inputs::OracleSet::Baseline, inputs::Game::Sekiro, "scene"),
    ) else {
        return Ok(());
    };
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let region: [f32; 4] = serde_json::from_value(manifest["region"].clone())?;
    let dll = common::oodle(dll)?;
    let mut world = World::default();
    let report = sekiro::read_game_area(
        &install,
        "m11_01",
        &Decoder {
            oodle: Some(&dll),
            max_output: 1 << 30,
        },
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: common::cells(region)[0],
                margin: 30.0,
            }),
            low_textures: true,
            ..Default::default()
        },
        &mut world,
    )?;
    println!(
        "textures {}, twins {}, without a twin {:?}",
        report.textures, report.low_textures, report.missing_low_textures
    );
    assert!(report.low_textures > 0);
    assert_eq!(
        report.low_texture_packages, 0,
        "Sekiro's twins are in the area binders, not packages"
    );
    assert_eq!(
        report.low_textures + report.missing_low_textures.len(),
        report.textures
    );
    Ok(())
}
