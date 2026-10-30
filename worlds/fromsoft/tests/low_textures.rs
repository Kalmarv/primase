//! Elden Ring's low-resolution texture twins (`Options::low_textures`) on the Limgrave tile `m60_42_36_00`, read and
//! prepared as the lab prepares an Elden Ring area for its Shaded view (the shaders resolved, the materials lowered
//! with their layers kept, then the bake, here at 16 pixels).
//!
//! Without the option the read and the preparation hold nothing of the twins. With it, every read texture either has
//! its twin, is named as having none, or is the coarse form a binding binds (an asset's binding to another asset
//! group's texture, `materials::drawn_coarse`: observed, not traced); the textures made from the sources and the
//! bake's textures carry their twins as forms (`core.texture_forms`), a material baked over its twins only where every
//! texture its program reads has one; every asset and map piece placed carries its draw param's texture switch
//! (`core.texture_switch`, with the view, `core.texture_switch_view`); and keeping the finest form of every texture
//! (`select-texture-form{form=finest}`) leaves what the read without the option prepares, byte for byte, but for the
//! materials with a binding drawn coarse and the textures they alone bind.
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::convert::NoticeKind;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Entity, Instance, Mesh, Name, Resource, SelectTextureForm, Texture, TextureChoice,
    TextureForms, TextureSwitch, TextureSwitchView, World,
};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::LowTexturePayloads;
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};

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

/// What a preparation holds, by name rather than entity.
struct Held {
    /// Each named texture's bytes, with the entity it describes, sorted.
    textures: Vec<(String, Entity)>,
    /// Each material with the names of the textures it binds, with the entity it describes, sorted.
    materials: Vec<(String, Entity)>,
    /// The meshes and the placements, counted.
    counts: [usize; 2],
}

fn held(world: &World) -> Held {
    let name = |e: Entity| {
        world
            .get::<Name>(e)
            .map_or_else(|| format!("{e:?}"), |n| n.0.clone())
    };
    let mut textures: Vec<(String, Entity)> = world
        .iter::<Texture>()
        .map(|(e, t)| {
            let mut hash = sha2::Sha256::new();
            for level in &t.levels {
                hash.update(level);
            }
            let line = format!(
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
            );
            (line, e)
        })
        .collect();
    textures.sort();
    let placeholder = World::new().spawn();
    let mut materials: Vec<(String, Entity)> = world
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
            (format!("{} {bound:?} {m:?}", name(e)), e)
        })
        .collect();
    materials.sort();
    Held {
        textures,
        materials,
        counts: [world.iter::<Mesh>().count(), world.iter::<Instance>().count()],
    }
}

/// The entries of `a` that `b` does not hold.
fn only<'a>(a: &'a [(String, Entity)], b: &[(String, Entity)]) -> Vec<&'a (String, Entity)> {
    let b: BTreeSet<&str> = b.iter().map(|(s, _)| s.as_str()).collect();
    a.iter().filter(|(s, _)| !b.contains(s.as_str())).collect()
}

/// Whether the texture named `name` was made from the stem `stem` (`stem`, then nothing, `.` or `@`).
fn made_from(name: &str, stem: &str) -> bool {
    name.strip_prefix(stem)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '@']))
}

/// The source materials (`fromsoft.material`, kept beside the lowered one) of `world` with a binding drawn coarse,
/// and the stored stems those bindings draw the coarse form of.
fn coarse(world: &World) -> (BTreeSet<Entity>, BTreeSet<String>) {
    let (mut materials, mut stems) = (BTreeSet::new(), BTreeSet::new());
    for (e, m) in world.iter::<Material>() {
        for stored in m.bindings.values().filter_map(|b| b.coarse_of.as_ref()) {
            materials.insert(e);
            stems.insert(stored.clone());
        }
    }
    (materials, stems)
}

/// The textures `materials` bind.
fn bound(world: &World, materials: impl IntoIterator<Item = Entity>) -> BTreeSet<Entity> {
    materials
        .into_iter()
        .filter_map(|m| world.get::<PbrMaterial>(m))
        .flat_map(|m| m.slots().map(|(_, r)| r.texture).collect::<Vec<_>>())
        .collect()
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
    assert!(
        off.iter::<TextureSwitch>().next().is_none() && off.resource::<TextureSwitchView>().is_none(),
        "a read without twins switches nothing"
    );
    assert_eq!((off_report.coarse_bindings, off_report.coarse_textures), (0, 0));
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
    // every texture read has its twin, is named, or is a coarse form a binding binds (pinned: regulation 11730034)
    assert!(report.low_textures > 0 && report.low_texture_packages > 0);
    println!(
        "coarse bindings {}, coarse textures {}, texture switch {:?}",
        report.coarse_bindings, report.coarse_textures, report.texture_switch
    );
    assert_eq!((report.coarse_bindings, report.coarse_textures), (784, 130));
    assert_eq!(
        report.low_textures + report.missing_low_textures.len() + report.coarse_textures,
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
    // every asset and map piece placed switches its textures' form at its draw param's distances (the read's 679
    // placements), and every placement the preparation keeps keeps its switch
    let t = &report.texture_switch;
    assert_eq!(
        (
            t.unread.is_none(),
            t.switched,
            t.border_zero,
            t.without_row,
            t.pieces_unestablished,
            t.asset_rows_differ
        ),
        (true, 679, 0, 0, 0, 0)
    );
    for (e, s) in on.iter::<TextureSwitch>() {
        assert_eq!(s.verify(e, &on), Ok(()));
    }
    assert!(
        on.iter::<Instance>().all(|(e, _)| on.has::<TextureSwitch>(e)),
        "a placement kept without its switch"
    );
    // the read says what of the game's state the switches leave out, and that the coarse bindings are observed
    let approximated = |what: &str| {
        report
            .notices()
            .iter()
            .filter(|n| n.kind == NoticeKind::Approximation && n.reason.starts_with(what))
            .map(|n| n.count)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        approximated("texture switches stated in the game's steady state"),
        [679]
    );
    assert_eq!(
        approximated("bindings drawn in their texture's coarse form"),
        [784]
    );
    assert!(
        off_report
            .notices()
            .iter()
            .all(|n| !n.reason.contains("texture switch") && !n.reason.contains("coarse form")),
        "a read without twins says nothing of them"
    );
    let view = on.resource::<TextureSwitchView>().expect("the switch's view");
    assert_eq!(view.verify(&on), Ok(()));
    // the finest form of every texture is the read without twins, but for the bindings drawn coarse
    SelectTextureForm {
        choice: TextureChoice::Finest,
    }
    .run(&mut on, &mut PassLog::default())?;
    let (off_held, on_held) = (held(&off), held(&on));
    assert_eq!(off_held.counts, on_held.counts, "meshes and placements");
    let (coarse_materials, coarse_stems) = coarse(&on);
    assert_eq!(coarse_stems.len(), report.coarse_textures);
    let (off_materials, on_materials) = (
        only(&off_held.materials, &on_held.materials),
        only(&on_held.materials, &off_held.materials),
    );
    let (off_textures, on_textures) = (
        only(&off_held.textures, &on_held.textures),
        only(&on_held.textures, &off_held.textures),
    );
    println!(
        "materials with a coarse binding {}; differing from the read without twins: materials {} for {}, textures {} for {}",
        coarse_materials.len(),
        on_materials.len(),
        off_materials.len(),
        on_textures.len(),
        off_textures.len()
    );
    // a material differs only where a binding of it is drawn coarse: the read without twins binds the stored texture
    for (line, e) in &on_materials {
        assert!(coarse_materials.contains(e), "{line}");
    }
    for (line, e) in &off_materials {
        let source = off
            .get::<Material>(*e)
            .expect("a lowered material keeps its source");
        assert!(
            source
                .bindings
                .values()
                .any(|b| coarse_stems.contains(&b.texture)),
            "{line}"
        );
    }
    // and a texture only where it is a coarse form, a stored texture drawn coarse, or a differing material's bake
    let (on_bound, off_bound) = (
        bound(&on, on_materials.iter().map(|(_, e)| *e)),
        bound(&off, off_materials.iter().map(|(_, e)| *e)),
    );
    let name = |w: &World, e: Entity| w.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
    for (line, e) in &on_textures {
        let n = name(&on, *e);
        assert!(
            on_bound.contains(e) || coarse_stems.iter().any(|s| made_from(&n, &format!("{s}_l"))),
            "{line}"
        );
    }
    for (line, e) in &off_textures {
        let n = name(&off, *e);
        assert!(
            off_bound.contains(e) || coarse_stems.iter().any(|s| made_from(&n, s)),
            "{line}"
        );
    }
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
