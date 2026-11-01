use super::*;
use primase_core::pass::PassRegistry;

fn registry() -> PassRegistry {
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_animation::register(&mut registry);
    super::super::passes::register(&mut registry);
    registry
}

const WATER: [u8; 4] = [48, 98, 122, 190];

fn character() -> CharacterPreparation {
    CharacterPreparation {
        game: Game::Sekiro,
        materials: Materials::Preserve,
        game_shaders: false,
        water_colour: WATER,
        parts: 1,
        select_assets: Vec::new(),
        animations: None,
        dressing: false,
        entity_names: false,
        held: None,
        bake_cache: Cache::On,
    }
}

fn area(materials: Materials, game_shaders: bool, read_states: bool, sky: bool) -> AreaPreparation {
    AreaPreparation {
        game: AreaGame::Sekiro {
            materials,
            game_shaders,
            read_states,
        },
        sky,
        water_colour: WATER,
        hour: None,
        environment_probes: false,
        weather: false,
        irradiance_package: None,
        irradiance_states: false,
        effects: false,
        bake_cache: Cache::On,
    }
}

fn elden(sky: bool) -> AreaPreparation {
    AreaPreparation {
        game: AreaGame::EldenRing,
        ..area(Materials::Preserve, false, false, sky)
    }
}

#[test]
fn character_preparation_keeps_the_skinned_bind_pose_and_places_the_asset() {
    let texts = character().texts();
    let joined = texts.join(", ");
    assert!(
        joined.contains("lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}"),
        "{joined}"
    );
    assert!(!joined.contains("fromsoft-apply-rigid-skinning"), "{joined}");
    assert!(!joined.contains("select-lod"), "{joined}");
    assert!(joined.contains("lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}"));
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    // a part's own collision is lowered onto it; the area's collision and object collision are not this read's
    assert!(
        !joined.contains("resolve-state")
            && !joined.contains("lower-fromsoft-collision")
            && !joined.contains("lower-fromsoft-object-collision"),
        "{joined}"
    );
    assert!(joined.contains(PART_COLLISION), "{joined}");
    let pipeline = registry().parse(&joined).expect("character preparation parses");
    pipeline.check().expect("character preparation order");
    assert!(
        CharacterPreparation {
            materials: Materials::Bake,
            ..character()
        }
        .texts()
        .iter()
        .any(|text| text.starts_with("bake-fromsoft-layers"))
    );
    assert!(
        CharacterPreparation {
            materials: Materials::Preview,
            ..character()
        }
        .texts()
        .iter()
        .any(|text| text == "lower-fromsoft-materials{layers=preview,water-colour=48,98,122,190}")
    );
}

/// A character of several parts is assembled before the asset is placed; a character of one part is not assembled.
#[test]
fn several_parts_are_assembled_before_they_are_placed() {
    let texts = CharacterPreparation {
        parts: 2,
        ..character()
    }
    .texts();
    let assemble = texts
        .iter()
        .position(|t| t == ASSEMBLE_PARTS)
        .expect("the parts are assembled");
    let place = texts
        .iter()
        .position(|t| t == "place-assets")
        .expect("the asset is placed");
    assert!(assemble < place, "{texts:?}");
    assert!(
        !character()
            .texts()
            .iter()
            .any(|t| t.starts_with("assemble-character"))
    );
}

/// Held weapons leave the body's assembly and are held after the assets are placed, at the grip given.
#[test]
fn held_weapons_are_left_out_of_the_assembly_and_held_after_placing() {
    let mut texts = CharacterPreparation {
        parts: 2,
        ..character()
    }
    .texts();
    held_weapon_passes(&mut texts, &[], "one-handed");
    assert!(!texts.iter().any(|t| t.contains("hold-fromsoft-weapons")));
    assert!(texts.iter().any(|t| t == ASSEMBLE_PARTS));
    let held = [
        "parts/wp_a_0300_l".to_owned(),
        "parts/wp_a_0300_l/wp_a_0300_1_l".to_owned(),
    ];
    held_weapon_passes(&mut texts, &held, "one-handed");
    let assemble = texts
        .iter()
        .find(|t| t.starts_with("assemble-character"))
        .unwrap();
    assert!(
        assemble.ends_with(",leave-out=parts/wp_a_0300_l+parts/wp_a_0300_l/wp_a_0300_1_l}"),
        "{assemble}"
    );
    let place = texts.iter().position(|t| t == "place-assets").unwrap();
    assert_eq!(texts[place + 1], "hold-fromsoft-weapons{grip=one-handed}");
    let mut texts = character().texts();
    held_weapon_passes(&mut texts, &held, "two-handed-right");
    assert_eq!(
        texts.last().unwrap(),
        "hold-fromsoft-weapons{grip=two-handed-right}"
    );
    // the program states them as a field
    let stated = CharacterPreparation {
        held: Some(HeldWeapons {
            held: held.to_vec(),
            grip: "one-handed".into(),
        }),
        ..character()
    };
    assert_eq!(
        stated.texts().last().unwrap(),
        "hold-fromsoft-weapons{grip=one-handed}"
    );
}

#[test]
fn no_row_is_the_hidden_masks_form_of_the_outfit_pass() {
    let hidden = outfit_passes("c3000", None);
    assert_eq!(hidden[1], "select-fromsoft-outfit{chr=c3000,npc=none}");
    // primase reads both forms
    for texts in [&hidden, &outfit_passes("c3000", Some(30000014))] {
        let pipeline = registry().parse(&texts.join(", ")).expect("it parses");
        pipeline.check().expect("its order");
    }
    assert_eq!(
        outfit_passes("c3000", Some(30000014))[1],
        "select-fromsoft-outfit{chr=c3000,npc=30000014}"
    );
}

/// The outfit passes sit right after the geometry lowering, and parse and order soundly with the rest.
#[test]
fn the_outfit_passes_follow_the_geometry_lowering() {
    let mut texts = character().texts();
    texts.splice(1..1, outfit_passes("c3000", Some(30000000)));
    assert!(texts[0].starts_with("lower-fromsoft-geometry"), "{texts:?}");
    assert_eq!(texts[1], "raise-fromsoft-display-masks");
    assert_eq!(texts[2], "select-fromsoft-outfit{chr=c3000,npc=30000000}");
    assert_eq!(texts[3], "resolve-state");
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    let pipeline = registry().parse(&texts.join(", ")).expect("it parses");
    pipeline.check().expect("its order");
}

#[test]
fn the_animation_passes_run_after_the_model_is_lowered_and_before_it_is_placed() {
    let binding = ClipBinding {
        of: "c1010".into(),
        skeleton: "c1010 skeleton".into(),
        tolerance: "0.0001".into(),
        absent: "neutral".into(),
        rate: 2,
        clips: "all".into(),
        root_motion: "translation".into(),
        unmapped: "drop".into(),
        parents: "same".into(),
        rests: "target".into(),
        reparented: "refuse".into(),
    };
    let texts = CharacterPreparation {
        animations: Some(binding),
        ..character()
    }
    .texts();
    let lower = texts
        .iter()
        .position(|t| {
            t == "bind-fromsoft-animation{from=c1010 havok skeleton,to=c1010 skeleton,tolerance=0.0001,empty=model-space,absent=neutral,rate=2,root-motion=translation,clips=all,unmapped=drop,parents=same,rests=target,reparented=refuse}"
        })
        .expect("the binding pass lowers and maps clips");
    assert_eq!(texts[lower + 1], CONFIGURE_SIMULATIONS);
    assert_eq!(texts.last().map(String::as_str), Some("place-assets"));
    let pipeline = registry().parse(&texts.join(", ")).expect("it parses");
    pipeline.check().expect("its order");
}

/// A dressed character's preparation: the outfit resolved right after the geometry, and the display masks, material
/// events and names right before the asset is placed; a form of a part keeps only its names.
#[test]
fn a_dressed_character_states_its_outfit_after_the_geometry_and_its_events_before_placing() {
    let texts = CharacterPreparation {
        game: Game::EldenRing,
        dressing: true,
        entity_names: true,
        select_assets: vec!["wp_a_0761_l".into(), "c0000 animations".into()],
        ..character()
    }
    .texts();
    assert_eq!(texts[0], "select-assets{names=wp_a_0761_l;c0000 animations}");
    assert!(texts[1].starts_with("lower-fromsoft-geometry"), "{texts:?}");
    assert_eq!(texts[2], "lower-fromsoft-selected-outfit");
    let n = texts.len();
    assert_eq!(texts[n - 4], "lower-fromsoft-display-masks");
    assert_eq!(texts[n - 3], "lower-fromsoft-material-events{game=elden}");
    assert_eq!(texts[n - 2], "name-fromsoft-entities{game=elden}");
    assert_eq!(texts[n - 1], "place-assets");
    let pipeline = registry().parse(&texts.join(", ")).expect("it parses");
    pipeline.check().expect("its order");
}

#[test]
fn the_default_area_preparation_preserves_layers_and_resolves_state_first() {
    let native = area(Materials::Preserve, false, true, false).texts();
    assert_eq!(native[0], "resolve-state");
    assert!(
        !native.iter().any(|t| t.starts_with("bake-fromsoft-layers")),
        "the default must not bake: {native:?}"
    );
    assert!(
        native
            .iter()
            .any(|t| t == "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}"),
        "{native:?}"
    );
    assert!(
        !area(Materials::Preserve, false, false, false)
            .texts()
            .iter()
            .any(|t| t == "resolve-state")
    );
    let baked = area(Materials::Bake, false, false, false).texts();
    assert!(baked.iter().any(|t| t.starts_with("bake-fromsoft-layers")));
    assert!(
        baked
            .iter()
            .any(|t| t == "lower-fromsoft-materials{water-colour=48,98,122,190}")
    );
}

#[test]
fn both_area_recipes_select_the_registered_render_graph_lowering() {
    for recipe in [AREA_PREP, ELDEN_AREA_PREP] {
        assert_eq!(
            recipe
                .iter()
                .filter(|text| **text == "lower-fromsoft-render-graph")
                .count(),
            1
        );
    }
}

#[test]
fn the_area_preparation_parses_and_checks() {
    let registry = registry();
    let texts: Vec<String> = AREA_PREP
        .iter()
        .map(|text| match *text {
            MATERIALS_PASS => "lower-fromsoft-materials{water-colour=48,98,122,190}".to_owned(),
            text => text.to_owned(),
        })
        .collect();
    let pipeline = registry
        .parse(&texts.join(", "))
        .expect("the preparation lowering's text parses");
    pipeline.check().expect("its passes are ordered soundly");
    assert_eq!(pipeline.passes.len(), AREA_PREP.len());
}

#[test]
fn an_area_read_with_its_sky_keeps_the_sky_geometry_and_lowers_it_after_the_environment() {
    let geometry = |texts: &[String]| {
        texts
            .iter()
            .find(|t| t.starts_with("lower-fromsoft-geometry"))
            .cloned()
            .unwrap()
    };
    let without = area(Materials::Preserve, true, true, false).texts();
    assert!(
        geometry(&without).contains("leave-out=sky+light-volume+shadow"),
        "{without:?}"
    );
    assert!(!without.iter().any(|t| t == SKY_PASS));
    let with = area(Materials::Preserve, true, true, true).texts();
    assert!(
        geometry(&with).contains("leave-out=light-volume+shadow,"),
        "{with:?}"
    );
    let environment = with.iter().position(|t| t == ENVIRONMENT_PASS).unwrap();
    let lights = with
        .iter()
        .position(|t| t.starts_with("lower-fromsoft-lights"))
        .unwrap();
    assert_eq!(with[lights + 1], SKY_PASS);
    assert!(environment < lights, "{with:?}");
    let pipeline = registry()
        .parse(&with.join(", "))
        .expect("the preparation parses");
    pipeline.check().expect("the preparation's order");
}

/// Elden Ring's sky (its sky map's weather assets) is read as Sekiro's is: the sky's meshes kept, what else the
/// geometry lowering leaves out still out, and the sky lowered after the environment.
#[test]
fn an_elden_ring_area_read_with_its_sky_keeps_the_sky_geometry_and_lowers_it_after_the_environment() {
    let geometry = |texts: &[String]| {
        texts
            .iter()
            .find(|t| t.starts_with("lower-fromsoft-geometry"))
            .cloned()
            .unwrap()
    };
    let without = elden(false).texts();
    assert!(
        geometry(&without).contains("leave-out=sky+light-volume+shadow+billboard"),
        "{without:?}"
    );
    assert!(!without.iter().any(|t| t == SKY_PASS));
    let with = elden(true).texts();
    assert!(
        geometry(&with).contains("leave-out=light-volume+shadow+billboard,"),
        "{with:?}"
    );
    let environment = with.iter().position(|t| t == ENVIRONMENT_PASS).unwrap();
    let tone = with
        .iter()
        .position(|t| t == "lower-elden-ring-reinhard")
        .unwrap();
    let lights = with
        .iter()
        .position(|t| t.starts_with("lower-fromsoft-lights"))
        .unwrap();
    assert_eq!(with[lights + 1], SKY_PASS);
    assert!(environment < tone && tone < lights, "{with:?}");
    let pipeline = registry()
        .parse(&with.join(", "))
        .expect("the preparation parses");
    pipeline.check().expect("the preparation's order");
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
    let game = area(Materials::Preserve, true, true, false).texts();
    assert!(position(&game, "resolve-fromsoft-shaders") < position(&game, "lower-fromsoft-materials"));
    let baked = area(Materials::Bake, false, true, false).texts();
    assert!(position(&baked, "resolve-fromsoft-shaders") < position(&baked, "bake-fromsoft-layers"));
    let neither = area(Materials::Preserve, false, true, false).texts();
    assert!(neither.iter().any(|t| t.starts_with("resolve-fromsoft-shaders")));
    for texts in [&game, &baked] {
        let pipeline = registry()
            .parse(&texts.join(", "))
            .expect("the preparation parses");
        pipeline.check().expect("the preparation's order");
    }
}

/// An area's preparation states the hour of the day before the first pass that reads it: the environment's lowering
/// (and the draw-param sample, where an hour is asked for) before the shader resolution and every bake, for either
/// game, with or without the layer bake, the Game shader view and the sky. A bake refuses a program that reads the
/// hour where none is stated, so a bake before the environment would refuse Elden Ring's sky.
#[test]
fn an_area_preparation_states_the_hour_before_the_first_bake() {
    for sekiro in [true, false] {
        for (bake_layers, game_shaders, sky, hour) in [
            (false, false, false, None),
            (true, false, true, None),
            (false, true, true, Some(6.5)),
            (true, true, false, Some(21.0)),
        ] {
            let base = if sekiro {
                let materials = if bake_layers {
                    Materials::Bake
                } else {
                    Materials::Preserve
                };
                area(materials, game_shaders, false, sky)
            } else {
                elden(sky)
            };
            let texts = AreaPreparation { hour, ..base }.texts();
            let at = |pass: &str| texts.iter().position(|t| t.starts_with(pass));
            let environment = at("lower-fromsoft-environment").expect("the environment is lowered");
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
            let pipeline = registry()
                .parse(&texts.join(", "))
                .expect("the preparation parses");
            pipeline.check().expect("the preparation's order");
        }
    }
}

/// An area names its entities for its game.
#[test]
fn an_area_names_its_entities_for_its_game() {
    assert_eq!(
        area(Materials::Preserve, false, false, false)
            .texts()
            .last()
            .unwrap(),
        "name-fromsoft-entities{game=sekiro}"
    );
    assert_eq!(
        elden(false).texts().last().unwrap(),
        "name-fromsoft-entities{game=elden}"
    );
}

/// The bake's memo store is a stated choice.
#[test]
fn the_bake_cache_is_a_stated_choice() {
    let on = elden(false).texts();
    assert!(
        on.iter()
            .any(|t| t == "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=on}")
    );
    let off = AreaPreparation {
        bake_cache: Cache::Off,
        ..elden(false)
    }
    .texts();
    assert!(
        off.iter()
            .any(|t| t == "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=off}")
    );
    let pipeline = registry().parse(&off.join(", ")).expect("the preparation parses");
    pipeline.check().expect("the preparation's order");
}

#[test]
fn a_clip_preparation_binds_the_animations_then_lowers_the_events() {
    let preparation = ClipPreparation {
        game: Game::EldenRing,
        binding: ClipBinding {
            of: "c0000".into(),
            skeleton: "c0000 skeleton".into(),
            tolerance: "0.0001".into(),
            absent: "neutral".into(),
            rate: 1,
            clips: "a000_000000".into(),
            root_motion: "leave".into(),
            unmapped: "drop".into(),
            parents: "any".into(),
            rests: "source".into(),
            reparented: "model-space".into(),
        },
        effects: true,
    };
    let texts = preparation.texts();
    assert!(texts[0].starts_with("bind-fromsoft-animation{from=c0000 havok skeleton,to=c0000 skeleton"));
    assert_eq!(texts[1], "lower-fromsoft-clip-events{game=elden}");
    assert_eq!(texts[2], "lower-fromsoft-effects{candela-per-power=1}");
    assert_eq!(texts[texts.len() - 3], "lower-fromsoft-display-masks");
    assert_eq!(texts.last().unwrap(), "name-fromsoft-entities{game=elden}");
    let without = ClipPreparation {
        effects: false,
        ..preparation
    }
    .texts();
    assert!(!without.iter().any(|t| t.starts_with("lower-fromsoft-effects")));
}
