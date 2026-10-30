//! GPARAM hemisphere conversion against the independent cbLight oracle in Sekiro 1.06 frame 13604.
//! Both captured light sets have these same six AmbientSH coefficients. Expected values are captured
//! f32 words, not recomputed from the reader's output. Needs the Sekiro install, keys and Oodle 6.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{AmbientLight, Environment, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::{DefaultHour, DrawField, DrawGroup, DrawParam, FieldType, SetChosen};

#[test]
fn hemisphere_coefficients_match_the_captured_light_sets_bit_for_bit()
-> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(folder), Some(dll)) = (
        inputs::install(inputs::Game::Sekiro),
        inputs::keys(inputs::Game::Sekiro),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let keys = Keys::from_folder(folder, Game::Sekiro, Game::Sekiro.param_key_sha256())?;
    let archive = common::install(root, Game::Sekiro, keys).open()?;
    let bytes = archive.read("/param/drawparam/m11_01_0000.gparam.dcx", &decoder)?;
    let gparam = fromsoft_formats::gparam::Gparam::read(&bytes)?;
    let group = gparam
        .groups
        .iter()
        .find(|g| g.key == "LightSet ParamEditor")
        .expect("the captured light set's group");
    // The capture identifies value ID 0. Test that observed set directly; the lightweight
    // area helper selects against collision parts alone, which choose a different set here.
    let fields = group
        .fields
        .iter()
        .flat_map(|field| {
            field.indices_of(0).filter_map(move |index| {
                Some(DrawField {
                    key: field.key.clone(),
                    name: field.name.clone(),
                    value: field.values.get(index)?,
                    stored: FieldType::of(&field.values),
                    time: field.times.get(index).copied(),
                })
            })
        })
        .collect();
    let draw = DrawParam {
        game: Game::Sekiro,
        file: "m11_01_0000".into(),
        version: gparam.version,
        time: gparam.time,
        groups: vec![DrawGroup {
            key: group.key.clone(),
            name: group.name.clone(),
            id: 0,
            chosen: SetChosen::OnlySet,
            fields,
        }],
        default_hour: DefaultHour {
            hour: 18.,
            source: "the captured frame's observed key".into(),
        },
    };
    let mut world = World::new();
    world.insert_resource(draw);
    let mut registry = PassRegistry::new();
    sekiro::passes::register(&mut registry);
    let pipeline =
        registry.parse("sample-fromsoft-draw-params{hour=18},lower-fromsoft-environment{lux-per-unit=1}")?;
    PassManager::new(registry).run(&mut world, &pipeline, &mut RunReport::default())?;
    let ambient = world
        .resource::<Environment>()
        .and_then(|e| e.ambient.as_ref())
        .expect("the stored hemispheres");
    let AmbientLight::Gradient { sky, equator, ground } = ambient.light else {
        panic!("the stored hemispheres form a gradient");
    };
    let captured_mean = [0x3df2_8ce2, 0x3dd5_a885, 0x3e0c_4b06];
    let captured_slope = [0x3dde_64aa, 0x3dbc_259b, 0x3df9_172c];
    assert_eq!(equator.map(f32::to_bits), captured_mean);
    let slope = [0, 1, 2].map(|i| (sky[i] - ground[i]) * 0.5);
    assert_eq!(slope.map(f32::to_bits), captured_slope);
    Ok(())
}
