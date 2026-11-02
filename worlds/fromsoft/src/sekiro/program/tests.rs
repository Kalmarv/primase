use std::collections::BTreeSet;

use super::super::passes::collision::FMeshes;
use super::super::passes::lights::{Brightness, Disabled};
use super::super::passes::object_collision::{
    ConvexBodies, ConvexSurface, MotionSelection, ResolutionChoice,
};
use super::super::passes::settings::{Factor, Fraction, Metres, PixelCap, Seconds};
use super::super::passes::weapons::{Grip, HoldGrip};
use super::super::passes::{Cards, Degenerates, Pose, SpeedTreeFrame, Unbound, Winding};
use super::*;
use primase_animation::simulation_schedule::SimulationSchedule;
use primase_animation::{Correspondence, ReferencePose, UnweightedRests, WeightedRests};
use primase_core::LodChoice;
use primase_core::pass::PassRegistry;
use primase_image::Srgba8;

fn registry() -> PassRegistry {
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_animation::register(&mut registry);
    super::super::passes::register(&mut registry);
    registry
}

/// Each text parsed and written back as its pass states itself ([`pass_text`]): the same pass gives the same text
/// whatever the order its options were written in.
fn canonical(texts: &[String]) -> Vec<String> {
    registry()
        .parse(&texts.join(", "))
        .expect("the texts parse")
        .passes
        .iter()
        .map(|pass| pass_text(pass.as_ref()))
        .collect()
}

fn parses_and_orders(texts: &[String]) {
    let pipeline = registry()
        .parse(&texts.join(", "))
        .expect("the preparation parses");
    pipeline.check().expect("the preparation's order");
}

const WATER: Srgba8 = Srgba8([48, 98, 122, 190]);

fn metres(m: f32) -> Metres {
    Metres::new(m).unwrap()
}

fn lower_materials(materials: Materials) -> LowerFromSoftMaterials {
    let layers = match materials {
        Materials::Preserve => Some(Layers::Preserve),
        Materials::Preview => Some(Layers::Preview),
        Materials::Bake(_) => None,
    };
    LowerFromSoftMaterials::new(layers, WATER)
}

fn bake_layers() -> Materials {
    Materials::Bake(BakeFromSoftLayers::new(
        PixelCap::new(512).unwrap(),
        Seconds::new(0.).unwrap(),
    ))
}

fn material_bake(cache: bool) -> BakeFromSoftMaterials {
    BakeFromSoftMaterials::new(PixelCap::new(2048).unwrap(), Seconds::new(0.).unwrap()).with_cache(cache)
}

fn resolve() -> ResolveFromSoftShaders {
    ResolveFromSoftShaders::new(Unbound::Preview)
}

/// What the read marks held: the assembly leaves it out where weapons are held.
const HELD: &str = "fromsoft.held_weapon";

fn assembly(marked: Option<&str>) -> AssembleCharacter {
    AssembleCharacter {
        correspondence: Correspondence::ByName,
        reference_pose: ReferencePose::Assembled,
        tolerance: 0.001,
        reference_tolerance: 0.0001,
        unweighted_rests: UnweightedRests::Report,
        weighted_rests: WeightedRests::Report,
        name: "assembled".into(),
        leave_out_marked: marked.map(str::to_owned),
    }
}

fn character() -> CharacterPreparation {
    CharacterPreparation {
        game: Game::Sekiro,
        materials: Materials::Preserve,
        animations: None,
        dressing: false,
        entity_names: false,
        held: None,
        geometry: LowerFromSoftGeometry {
            pose: Pose::Stored,
            uv_tiling: Tiling::Keep,
            winding: Winding::Stored,
            degenerates: Degenerates::Keep,
            leave_out: BTreeSet::new(),
            blocker_cards: Cards::Draw,
            speedtree_frame: SpeedTreeFrame::Program,
        },
        resolve_shaders: resolve(),
        lower_materials: lower_materials(Materials::Preserve),
        material_bake: material_bake(true),
        part_collision: LowerFromSoftPartCollision {
            convex: ConvexBodies::Rounded(metres(0.01)),
            motion: MotionSelection::All,
        },
        assembly: assembly(None),
        simulations: ConfigureSimulations(SimulationSchedule::new(1. / 30., 30, 8).unwrap()),
    }
}

/// A character's preparation with `materials`, and the materials' lowering stating the same choice.
fn character_with(materials: Materials) -> CharacterPreparation {
    CharacterPreparation {
        materials,
        lower_materials: lower_materials(materials),
        ..character()
    }
}

fn area_geometry(leave_out: &[SkipClass]) -> LowerFromSoftGeometry {
    LowerFromSoftGeometry {
        pose: Pose::Rigid,
        uv_tiling: Tiling::Keep,
        winding: Winding::Normals,
        degenerates: Degenerates::Drop,
        leave_out: leave_out.iter().copied().collect(),
        blocker_cards: Cards::Record,
        speedtree_frame: SpeedTreeFrame::Program,
    }
}

/// A Sekiro area's preparation: the geometry keeps the sky's meshes where the read brought the sky in.
fn area(materials: Materials, read_states: bool, sky: bool) -> AreaPreparation {
    let leave_out: &[SkipClass] = if sky {
        &[SkipClass::LightVolume, SkipClass::Shadow]
    } else {
        &[SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow]
    };
    AreaPreparation {
        game: AreaGame::Sekiro {
            materials,
            read_states,
        },
        sky,
        draw_params: None,
        environment_probes: false,
        weather: false,
        irradiance_package: None,
        irradiance_states: false,
        effects: None,
        collision: LowerFromSoftCollision {
            reach: Some(metres(50.)),
            f_meshes: FMeshes::Off,
        },
        object_collision: LowerFromSoftObjectCollision {
            resolution: ResolutionChoice::High,
            convex: ConvexBodies::Rounded(metres(0.01)),
            motion: MotionSelection::Fixed,
            convex_surface: ConvexSurface::Render,
            reach: Some(metres(50.)),
        },
        geometry: area_geometry(leave_out),
        lod: SelectLod {
            choice: LodChoice::Finest,
        },
        environment: LowerFromSoftEnvironment::new(Factor::new(1.).unwrap()),
        resolve_shaders: resolve(),
        lower_materials: lower_materials(materials),
        material_bake: material_bake(true),
        release_sources: false,
        lights: LowerFromSoftLights {
            candela_per_power: Factor::new(1.).unwrap(),
            spot_blend: Fraction::new(0.15).unwrap(),
            brightness: Brightness::Settled,
            min_range: metres(0.5),
            include_disabled: Disabled::Skip,
        },
        map_records: LowerFromSoftMapRecords {
            leave_out: BTreeSet::new(),
        },
    }
}

/// An Elden Ring area's preparation: every collision file at its best resolution, every motion type, and SpeedTree's
/// billboard cards left out too.
fn elden(sky: bool) -> AreaPreparation {
    let sekiro = area(Materials::Preserve, false, sky);
    let mut leave_out = sekiro.geometry.leave_out.clone();
    leave_out.insert(SkipClass::Billboard);
    AreaPreparation {
        game: AreaGame::EldenRing,
        object_collision: LowerFromSoftObjectCollision {
            resolution: ResolutionChoice::HighElseLow,
            motion: MotionSelection::All,
            ..sekiro.object_collision.clone()
        },
        geometry: LowerFromSoftGeometry {
            leave_out,
            ..sekiro.geometry.clone()
        },
        ..sekiro
    }
}

fn effects() -> LowerFromSoftEffects {
    LowerFromSoftEffects::new(Factor::new(1.).unwrap())
        .with_seed(0)
        .with_include_disabled(Disabled::Skip)
}

fn binding(of: &str, parents: &str, rests: &str, reparented: &str) -> BindFromSoftAnimation {
    BindFromSoftAnimation {
        from: format!("{of} havok skeleton"),
        to: format!("{of} skeleton"),
        tolerance: 0.0001,
        empty: "model-space".into(),
        absent: "neutral".into(),
        rate: 2,
        root_motion: "translation".into(),
        clips: "all".into(),
        unmapped: "drop".into(),
        parents: parents.into(),
        rests: rests.into(),
        reparented: reparented.into(),
    }
}

fn texts(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

/// The pass lists the preparations state, compared pass by pass as each pass states itself: the values the builders
/// once fixed, with a character's requested form kept first and its parts always assembled (one part is nothing to
/// relate), held weapons left out by their mark.
#[test]
fn the_stated_values_give_the_pass_lists_the_builders_fixed() {
    const CHARACTER_GEOMETRY: &str = "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}";
    const RESOLVE: &str = "resolve-fromsoft-shaders{unbound-textures=preview}";
    const PART_COLLISION: &str = "lower-fromsoft-part-collision{convex=rounded:0.01,motion=all}";
    const SIMULATIONS: &str =
        "configure-simulations{step-seconds=0.03333333333333333,pre-roll=30,max-steps-per-frame=8}";
    const COLLISION: &str = "lower-fromsoft-collision{reach=50}";
    const LIGHTS: &str = "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,brightness=settled,min-range=0.5,include-disabled=off}";
    const ENVIRONMENT: &str = "lower-fromsoft-environment{lux-per-unit=1}";
    const RECORDS: &str = "lower-fromsoft-map-records{leave-out=}";

    let one_part = texts(&[
        "select-fromsoft-requested-form",
        CHARACTER_GEOMETRY,
        RESOLVE,
        "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
        "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=on}",
        PART_COLLISION,
        "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled}",
        SIMULATIONS,
        "place-assets",
    ]);
    assert_eq!(canonical(&character().texts().unwrap()), canonical(&one_part));

    let dressed = CharacterPreparation {
        game: Game::EldenRing,
        animations: Some(binding("c0000", "any", "source", "model-space")),
        dressing: true,
        entity_names: true,
        held: Some(HoldFromSoftWeapons {
            grip: HoldGrip::Stated(Grip::OneHanded),
        }),
        assembly: assembly(Some(HELD)),
        ..character_with(bake_layers())
    };
    let dressed_was = texts(&[
        "select-fromsoft-requested-form",
        CHARACTER_GEOMETRY,
        "lower-fromsoft-selected-outfit",
        RESOLVE,
        "bake-fromsoft-layers{max-size=512,sample-time=0}",
        "lower-fromsoft-materials{water-colour=48,98,122,190}",
        "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=on}",
        PART_COLLISION,
        "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled,leave-out-marked=fromsoft.held_weapon}",
        "bind-fromsoft-animation{from=c0000 havok skeleton,to=c0000 skeleton,tolerance=0.0001,empty=model-space,absent=neutral,rate=2,root-motion=translation,clips=all,unmapped=drop,parents=any,rests=source,reparented=model-space}",
        SIMULATIONS,
        "lower-fromsoft-display-masks",
        "lower-fromsoft-material-events{game=elden}",
        "name-fromsoft-entities{game=elden}",
        "place-assets",
        "hold-fromsoft-weapons{grip=one-handed}",
    ]);
    assert_eq!(canonical(&dressed.texts().unwrap()), canonical(&dressed_was));

    let everything = AreaPreparation {
        draw_params: Some(SampleFromSoftDrawParams::new(6.5).unwrap()),
        environment_probes: true,
        weather: true,
        irradiance_package: Some(IrradiancePackage {
            area: "m11_01_00_00".into(),
            package: 3,
        }),
        irradiance_states: true,
        effects: Some(effects()),
        ..area(Materials::Preserve, true, false)
    };
    let everything_was = texts(&[
        "resolve-state{assign=fromsoft.irradiance.m11_01_00_00:package-03}",
        "resolve-state",
        "resolve-state",
        "lower-fromsoft-render-graph",
        "lower-fromsoft-environment-probes",
        "bind-fromsoft-environment-probes",
        COLLISION,
        "lower-fromsoft-object-collision{resolution=high,convex=rounded:0.01,motion=fixed,convex-surface=render,reach=50}",
        "drop-fromsoft-far",
        "fromsoft-apply-rigid-skinning",
        "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record,speedtree-frame=program}",
        "select-lod{form=finest}",
        "sample-fromsoft-draw-params{hour=6.5}",
        ENVIRONMENT,
        "lower-sekiro-reinhard",
        RESOLVE,
        "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
        "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=on}",
        LIGHTS,
        "lower-fromsoft-extras",
        RECORDS,
        "lower-fromsoft-irradiance",
        "lower-fromsoft-irradiance",
        "lower-fromsoft-effects{candela-per-power=1}",
        "name-fromsoft-entities{game=sekiro}",
    ]);
    assert_eq!(
        canonical(&everything.texts().unwrap()),
        canonical(&everything_was)
    );

    let baked_with_sky = texts(&[
        "lower-fromsoft-render-graph",
        COLLISION,
        "lower-fromsoft-object-collision{resolution=high,convex=rounded:0.01,motion=fixed,convex-surface=render,reach=50}",
        "drop-fromsoft-far",
        "fromsoft-apply-rigid-skinning",
        "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=light-volume+shadow,blocker-cards=record,speedtree-frame=program}",
        "select-lod{form=finest}",
        ENVIRONMENT,
        "lower-sekiro-reinhard",
        RESOLVE,
        "bake-fromsoft-layers{max-size=512,sample-time=0}",
        "lower-fromsoft-materials{water-colour=48,98,122,190}",
        "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=on}",
        LIGHTS,
        "lower-fromsoft-sky",
        "lower-fromsoft-extras",
        RECORDS,
        "name-fromsoft-entities{game=sekiro}",
    ]);
    assert_eq!(
        canonical(&area(bake_layers(), false, true).texts().unwrap()),
        canonical(&baked_with_sky)
    );

    // the Stormveil example's: the read's stored models and texture files released before the surface bake
    let stormveil = AreaPreparation {
        draw_params: Some(SampleFromSoftDrawParams::new(12.).unwrap()),
        material_bake: material_bake(false),
        release_sources: true,
        ..elden(true)
    };
    let stormveil_was = texts(&[
        "lower-fromsoft-render-graph",
        COLLISION,
        "lower-fromsoft-object-collision{resolution=high-else-low,convex=rounded:0.01,motion=all,convex-surface=render,reach=50}",
        "fromsoft-apply-rigid-skinning",
        "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
        "select-lod{form=finest}",
        "sample-fromsoft-draw-params{hour=12}",
        ENVIRONMENT,
        "lower-elden-ring-reinhard",
        RESOLVE,
        "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
        "release-fromsoft-sources",
        "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=off}",
        LIGHTS,
        "lower-fromsoft-sky",
        "lower-fromsoft-extras",
        RECORDS,
        "name-fromsoft-entities{game=elden}",
    ]);
    assert_eq!(canonical(&stormveil.texts().unwrap()), canonical(&stormveil_was));

    let clip = ClipPreparation {
        game: Game::EldenRing,
        binding: binding("c0000", "any", "source", "model-space"),
        effects: Some(effects()),
    };
    let clip_was = texts(&[
        "bind-fromsoft-animation{from=c0000 havok skeleton,to=c0000 skeleton,tolerance=0.0001,empty=model-space,absent=neutral,rate=2,root-motion=translation,clips=all,unmapped=drop,parents=any,rests=source,reparented=model-space}",
        "lower-fromsoft-clip-events{game=elden}",
        "lower-fromsoft-effects{candela-per-power=1}",
        "lower-fromsoft-display-masks",
        "lower-fromsoft-material-events{game=elden}",
        "name-fromsoft-entities{game=elden}",
    ]);
    assert_eq!(canonical(&clip.texts()), canonical(&clip_was));
}

#[test]
fn character_preparation_keeps_the_skinned_bind_pose_and_places_the_asset() {
    let prep = character();
    let texts = prep.texts().unwrap();
    assert_eq!(texts[0], "select-fromsoft-requested-form");
    assert_eq!(texts[1], pass_text(&prep.geometry));
    assert!(
        !texts.iter().any(|t| t == "fromsoft-apply-rigid-skinning"),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.starts_with("select-lod")), "{texts:?}");
    assert!(texts.contains(&pass_text(&prep.lower_materials)), "{texts:?}");
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    // a part's own collision is lowered onto it; the area's collision and object collision are not this read's
    assert!(
        !texts.iter().any(|t| t.starts_with("resolve-state")
            || t.starts_with("lower-fromsoft-collision")
            || t.starts_with("lower-fromsoft-object-collision")),
        "{texts:?}"
    );
    assert!(texts.contains(&pass_text(&prep.part_collision)), "{texts:?}");
    parses_and_orders(&texts);
    let baked = character_with(bake_layers()).texts().unwrap();
    assert!(baked.iter().any(|text| text.starts_with("bake-fromsoft-layers")));
    parses_and_orders(&baked);
    let preview = character_with(Materials::Preview);
    assert!(
        preview
            .texts()
            .unwrap()
            .contains(&pass_text(&preview.lower_materials))
    );
}

/// A character's parts are assembled before the asset is placed, however many the read found (one is nothing to
/// relate, which the assembly reports).
#[test]
fn the_parts_are_assembled_before_they_are_placed() {
    let prep = character();
    let texts = prep.texts().unwrap();
    let assemble = texts
        .iter()
        .position(|t| *t == pass_text(&prep.assembly))
        .expect("the parts are assembled");
    let place = texts
        .iter()
        .position(|t| t == "place-assets")
        .expect("the asset is placed");
    assert!(assemble < place, "{texts:?}");
}

/// Held weapons are left out of the body's assembly by the read's mark (the program states that leave-out) and held
/// after the assets are placed, as stated; holding weapons the assembly does not leave out is refused.
#[test]
fn held_weapons_are_left_out_of_the_assembly_and_held_after_placing() {
    let hold = |grip| HoldFromSoftWeapons { grip };
    let stated = CharacterPreparation {
        held: Some(hold(HoldGrip::Stated(Grip::OneHanded))),
        assembly: assembly(Some(HELD)),
        ..character()
    };
    let texts = stated.texts().unwrap();
    let assemble = texts
        .iter()
        .find(|t| t.starts_with("assemble-character"))
        .unwrap();
    assert!(
        assemble.contains("leave-out-marked=fromsoft.held_weapon"),
        "{assemble}"
    );
    let place = texts.iter().position(|t| t == "place-assets").unwrap();
    assert_eq!(texts[place + 1], "hold-fromsoft-weapons{grip=one-handed}");
    parses_and_orders(&texts);

    // the assembly's leave-out is the program's to state: holding weapons it does not leave out is refused
    for assembly in [assembly(None), assembly(Some("core.name"))] {
        let refusal = CharacterPreparation {
            assembly,
            ..stated.clone()
        }
        .texts()
        .unwrap_err();
        assert!(
            refusal.contains("hold-fromsoft-weapons") && refusal.contains("assemble-character"),
            "{refusal}"
        );
    }
    // the save's grip, stated as such
    let saved = CharacterPreparation {
        held: Some(hold(HoldGrip::Saved)),
        ..stated.clone()
    };
    assert_eq!(
        saved.texts().unwrap().last().unwrap(),
        "hold-fromsoft-weapons{grip=saved}"
    );
    parses_and_orders(&saved.texts().unwrap());
    // no hold: nothing is held after placing
    let none = CharacterPreparation { held: None, ..stated };
    let texts = none.texts().unwrap();
    assert!(!texts.iter().any(|t| t.starts_with("hold-fromsoft-weapons")));
}

#[test]
fn no_row_is_the_hidden_masks_form_of_the_outfit_pass() {
    let hidden = outfit_passes("c3000", None);
    assert_eq!(hidden[1], "select-fromsoft-outfit{chr=c3000,npc=none}");
    // primase reads both forms
    for texts in [&hidden, &outfit_passes("c3000", Some(30000014))] {
        parses_and_orders(texts);
    }
    assert_eq!(
        outfit_passes("c3000", Some(30000014))[1],
        "select-fromsoft-outfit{chr=c3000,npc=30000014}"
    );
}

/// The outfit passes sit right after the geometry lowering, and parse and order soundly with the rest.
#[test]
fn the_outfit_passes_follow_the_geometry_lowering() {
    let mut texts = character().texts().unwrap();
    texts.splice(2..2, outfit_passes("c3000", Some(30000000)));
    assert!(texts[1].starts_with("lower-fromsoft-geometry"), "{texts:?}");
    assert_eq!(texts[2], "raise-fromsoft-display-masks");
    assert_eq!(texts[3], "select-fromsoft-outfit{chr=c3000,npc=30000000}");
    assert_eq!(texts[4], "resolve-state");
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    parses_and_orders(&texts);
}

#[test]
fn the_animation_passes_run_after_the_model_is_lowered_and_before_it_is_placed() {
    let prep = CharacterPreparation {
        animations: Some(binding("c1010", "same", "target", "refuse")),
        ..character()
    };
    let texts = prep.texts().unwrap();
    let bind = texts
        .iter()
        .position(|t| t.starts_with("bind-fromsoft-animation"))
        .expect("the binding pass lowers and maps clips");
    assert!(
        texts[..bind]
            .iter()
            .any(|t| t.starts_with("lower-fromsoft-geometry"))
    );
    assert_eq!(texts[bind + 1], pass_text(&prep.simulations));
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    parses_and_orders(&texts);
}

/// A dressed character's preparation: the requested form kept first, the outfit resolved right after the geometry,
/// and the display masks, material events and names right before the asset is placed.
#[test]
fn a_dressed_character_states_its_outfit_after_the_geometry_and_its_events_before_placing() {
    let texts = CharacterPreparation {
        game: Game::EldenRing,
        dressing: true,
        entity_names: true,
        ..character()
    }
    .texts()
    .unwrap();
    assert_eq!(texts[0], "select-fromsoft-requested-form");
    assert!(texts[1].starts_with("lower-fromsoft-geometry"), "{texts:?}");
    assert_eq!(texts[2], "lower-fromsoft-selected-outfit");
    let n = texts.len();
    assert_eq!(texts[n - 4], "lower-fromsoft-display-masks");
    assert_eq!(texts[n - 3], "lower-fromsoft-material-events{game=elden}");
    assert_eq!(texts[n - 2], "name-fromsoft-entities{game=elden}");
    assert_eq!(texts[n - 1], "place-assets");
    parses_and_orders(&texts);
}

#[test]
fn an_area_that_keeps_its_layers_does_not_bake_them_and_resolves_state_first_where_asked() {
    let native = area(Materials::Preserve, true, false);
    let texts = native.texts().unwrap();
    assert_eq!(texts[0], "resolve-state");
    assert!(
        !texts.iter().any(|t| t.starts_with("bake-fromsoft-layers")),
        "keeping the layers does not bake them: {texts:?}"
    );
    assert!(texts.contains(&pass_text(&native.lower_materials)), "{texts:?}");
    assert!(
        !area(Materials::Preserve, false, false)
            .texts()
            .unwrap()
            .iter()
            .any(|t| t == "resolve-state")
    );
    let baked = area(bake_layers(), false, false).texts().unwrap();
    assert!(baked.iter().any(|t| t.starts_with("bake-fromsoft-layers")));
    assert!(
        baked
            .iter()
            .any(|t| t == "lower-fromsoft-materials{water-colour=48,98,122,190}")
    );
}

#[test]
fn every_area_preparation_lowers_the_render_graph_once_and_parses_and_orders() {
    for prep in [
        area(Materials::Preserve, false, false),
        area(Materials::Preview, true, true),
        area(bake_layers(), false, false),
        elden(false),
        elden(true),
    ] {
        let texts = prep.texts().unwrap();
        assert_eq!(
            texts.iter().filter(|t| *t == RENDER_GRAPH).count(),
            1,
            "{texts:?}"
        );
        parses_and_orders(&texts);
    }
}

/// With the sky read in, the sky is lowered after the lights, so after the environment (Elden Ring's after its tone
/// mapping too); a geometry lowering that would leave the sky's meshes out is refused, for either game.
#[test]
fn an_area_read_with_its_sky_lowers_it_after_the_environment_and_keeps_its_meshes() {
    for (with, without) in [
        (
            area(Materials::Preserve, true, true),
            area(Materials::Preserve, true, false),
        ),
        (elden(true), elden(false)),
    ] {
        assert!(!without.texts().unwrap().iter().any(|t| t == SKY_PASS));
        let texts = with.texts().unwrap();
        let at = |pass: &str| texts.iter().position(|t| t.starts_with(pass)).unwrap();
        let lights = at("lower-fromsoft-lights");
        assert_eq!(texts[lights + 1], SKY_PASS);
        assert!(at("lower-fromsoft-environment{") < lights, "{texts:?}");
        if with.game == AreaGame::EldenRing {
            let tone = at("lower-elden-ring-reinhard");
            assert!(
                at("lower-fromsoft-environment{") < tone && tone < lights,
                "{texts:?}"
            );
        }
        parses_and_orders(&texts);
        // the sky's meshes left out while the sky is lowered: the program states one or the other
        let refused = AreaPreparation {
            geometry: without.geometry.clone(),
            ..with.clone()
        };
        let refusal = refused.texts().unwrap_err();
        assert!(
            refusal.contains("sky") && refusal.contains("leave-out"),
            "{refusal}"
        );
    }
}

#[test]
fn the_shader_binding_is_resolved_once_before_the_first_pass_that_consumes_the_materials() {
    let position = |texts: &[String], text: &str| {
        let at: Vec<usize> = texts
            .iter()
            .enumerate()
            .filter(|(_, t)| t.starts_with(text))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(at.len(), 1, "{text} in {texts:?}");
        at[0]
    };
    let kept = area(Materials::Preserve, true, false).texts().unwrap();
    assert!(position(&kept, "resolve-fromsoft-shaders") < position(&kept, "lower-fromsoft-materials"));
    let baked = area(bake_layers(), true, false).texts().unwrap();
    assert!(position(&baked, "resolve-fromsoft-shaders") < position(&baked, "bake-fromsoft-layers"));
    let character = character_with(bake_layers()).texts().unwrap();
    assert!(position(&character, "resolve-fromsoft-shaders") < position(&character, "bake-fromsoft-layers"));
    for texts in [&kept, &baked, &character] {
        parses_and_orders(texts);
    }
}

/// An area's preparation states the hour of the day before the first pass that reads it: the environment's lowering
/// (and the draw-param sample, where an hour is asked for) before the shader resolution and every bake, for either
/// game, with or without the layer bake and the sky. A bake refuses a program that reads the hour where none is
/// stated, so a bake before the environment would refuse Elden Ring's sky.
#[test]
fn an_area_preparation_states_the_hour_before_the_first_bake() {
    for sekiro in [true, false] {
        for (bake, sky, hour) in [
            (false, false, None),
            (true, true, None),
            (false, true, Some(6.5)),
            (true, false, Some(21.0)),
        ] {
            let base = if sekiro {
                area(if bake { bake_layers() } else { Materials::Preserve }, false, sky)
            } else {
                elden(sky)
            };
            let texts = AreaPreparation {
                draw_params: hour.map(|h| SampleFromSoftDrawParams::new(h).unwrap()),
                ..base
            }
            .texts()
            .unwrap();
            let at = |pass: &str| texts.iter().position(|t| t.starts_with(pass));
            let environment = at("lower-fromsoft-environment{").expect("the environment is lowered");
            let readers: Vec<usize> = texts
                .iter()
                .enumerate()
                .filter(|(_, t)| t.starts_with("bake-fromsoft-") || t.starts_with("resolve-fromsoft-shaders"))
                .map(|(i, _)| i)
                .collect();
            assert!(readers.iter().all(|&i| environment < i), "{texts:?}");
            if hour.is_some() {
                assert_eq!(
                    at("sample-fromsoft-draw-params"),
                    Some(environment - 1),
                    "{texts:?}"
                );
            }
            parses_and_orders(&texts);
        }
    }
}

/// An area names its entities for its game.
#[test]
fn an_area_names_its_entities_for_its_game() {
    assert_eq!(
        area(Materials::Preserve, false, false)
            .texts()
            .unwrap()
            .last()
            .unwrap(),
        "name-fromsoft-entities{game=sekiro}"
    );
    assert_eq!(
        elden(false).texts().unwrap().last().unwrap(),
        "name-fromsoft-entities{game=elden}"
    );
}

/// The bake's memo store is the program's, stated in the bake it states.
#[test]
fn the_bake_cache_is_a_stated_choice() {
    let on = elden(false).texts().unwrap();
    assert!(on.contains(&"bake-fromsoft-materials{cache=on,max-size=2048,sample-time=0}".to_owned()));
    let off = AreaPreparation {
        material_bake: material_bake(false),
        ..elden(false)
    }
    .texts()
    .unwrap();
    assert!(off.contains(&"bake-fromsoft-materials{cache=off,max-size=2048,sample-time=0}".to_owned()));
    parses_and_orders(&off);
}

/// Two choices that state one thing differently are refused, by name, for the program to state the consistent
/// value: the materials' `layers` and the materials choice, and a geometry that applies the UV tiling the surface
/// bake reads as authored.
#[test]
fn conflicting_choices_are_refused_by_name() {
    for (materials, layers) in [
        (Materials::Preserve, None),
        (Materials::Preserve, Some(Layers::Preview)),
        (Materials::Preview, Some(Layers::Preserve)),
        (bake_layers(), Some(Layers::Preserve)),
    ] {
        let lower = LowerFromSoftMaterials::new(layers, WATER);
        let refusal = AreaPreparation {
            lower_materials: lower,
            ..area(materials, false, false)
        }
        .texts()
        .unwrap_err();
        assert!(
            refusal.contains("lower-fromsoft-materials") && refusal.starts_with("materials "),
            "{refusal}"
        );
        let refusal = CharacterPreparation {
            lower_materials: lower,
            ..character_with(materials)
        }
        .texts()
        .unwrap_err();
        assert!(refusal.contains("lower-fromsoft-materials"), "{refusal}");
    }
    let tiled = |geometry: &LowerFromSoftGeometry| LowerFromSoftGeometry {
        uv_tiling: Tiling::Apply,
        ..geometry.clone()
    };
    let sekiro = area(Materials::Preserve, false, false);
    let refusal = AreaPreparation {
        geometry: tiled(&sekiro.geometry),
        ..sekiro
    }
    .texts()
    .unwrap_err();
    assert!(
        refusal.contains("uv-tiling=apply") && refusal.contains("bake-fromsoft-materials"),
        "{refusal}"
    );
    let character = character();
    assert!(
        CharacterPreparation {
            geometry: tiled(&character.geometry),
            ..character
        }
        .texts()
        .is_err()
    );
    // Elden Ring's materials' lowering states its own layers: no materials choice to agree with
    let elden_preview = AreaPreparation {
        lower_materials: lower_materials(Materials::Preview),
        ..elden(false)
    };
    assert!(elden_preview.texts().is_ok());
}

#[test]
fn a_clip_preparation_binds_the_animations_then_lowers_the_events() {
    let preparation = ClipPreparation {
        game: Game::EldenRing,
        binding: binding("c0000", "any", "source", "model-space"),
        effects: Some(effects()),
    };
    let texts = preparation.texts();
    assert_eq!(texts[0], pass_text(&preparation.binding));
    assert_eq!(texts[1], "lower-fromsoft-clip-events{game=elden}");
    assert_eq!(texts[2], "lower-fromsoft-effects{candela-per-power=1}");
    assert_eq!(texts[texts.len() - 3], "lower-fromsoft-display-masks");
    assert_eq!(texts.last().unwrap(), "name-fromsoft-entities{game=elden}");
    parses_and_orders(&texts);
    let without = ClipPreparation {
        effects: None,
        ..preparation
    }
    .texts();
    assert!(!without.iter().any(|t| t.starts_with("lower-fromsoft-effects")));
}
