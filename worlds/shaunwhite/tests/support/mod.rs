//! Shared raw-source fixtures; no lowered material snapshot.
#![allow(clippy::unwrap_used)]
use crate::inputs::{Game, install_file};
use anvil_formats::shaunwhite::shaders::{Database, Payload};
use iridia_interface::Stage;
use primase_core::shader::{KeyPart, VariantKey};
use primase_core::{World, shader::StageBinding};
/// An SM3 program lifted and taken through the passes every analysis and executor of the IR expects.
#[allow(dead_code, reason = "each test crate uses the fixtures it needs")]
pub fn lifted(bytes: &[u8]) -> iridia_ir::Program {
    let mut program = iridia_d3d9_ir::lift(bytes).unwrap();
    iridia_passes::prepare(&mut program).unwrap();
    program
}
/// The number of an input register (`vN` or `tN`), the index its value takes in a run's inputs.
#[allow(dead_code, reason = "each test crate uses the fixtures it needs")]
pub fn register_number(register: iridia_d3d9_ir::Register) -> usize {
    match register {
        iridia_d3d9_ir::Register::Input(n) | iridia_d3d9_ir::Register::Texture(n) => n as usize,
        other => panic!("{other:?} is not an input register"),
    }
}
pub fn binding(db: &Database, record: u32) -> StageBinding {
    let shader = World::new().spawn();
    StageBinding {
        variant: Some(VariantKey {
            field: "shaunwhite.shader.record".into(),
            value: record,
            parts: vec![
                KeyPart {
                    name: "SDB2 permutation".into(),
                    value: db.records[record as usize].key.permutation,
                },
                KeyPart {
                    name: "native pixel cache key".into(),
                    value: match &db.records[record as usize].payload {
                        Payload::Effect(e) => e.pixel_key,
                        _ => 0,
                    },
                },
            ],
        }),
        ..StageBinding::new(Stage::Pixel, shader)
    }
}
pub fn material(
    cell: &str,
    name: &str,
) -> Option<(
    primase_shaunwhite::components::MaterialRef,
    primase_shaunwhite::components::MaterialShader,
)> {
    use primase_shaunwhite::components::{MaterialRef, MaterialState, SlotRef};
    let path = install_file(Game::ShaunWhite, "DataPC_01.forge")?;
    let forge = anvil_formats::forge::Forge::open(&path).unwrap();
    let index = forge.names.iter().position(|n| n.name == cell).unwrap();
    let mut file = std::fs::File::open(path).unwrap();
    let entry = anvil_formats::data::read_entry(&forge.payload(&mut file, index).unwrap(), 1 << 30).unwrap();
    let objects = anvil_formats::objects::read_objects(&entry.metadata.data, &entry.resource.data).unwrap();
    let object = objects
        .iter()
        .find(|o| o.class == 0xb71f6261 && o.name == name)
        .unwrap();
    let body = &entry.resource.data[object.body.clone()];
    let m = anvil_formats::shaunwhite::material::Material::parse(body).unwrap();
    let fields = anvil_formats::shaunwhite::shader_material::read(body).unwrap();
    let material = MaterialRef {
        id: m.id,
        name: object.name.clone(),
        state: Some(MaterialState {
            blend: m.blend,
            blended: m.blended,
            alpha_test: m.alpha_test,
            alpha_ref: m.alpha_ref,
            water: m.water,
            two_sided: m.two_sided,
            invert_culling: m.invert_culling,
            color: m.color,
            illumination: m.illumination,
            specular_level: m.specular_level,
            specular_power: m.specular_power,
            normal_scale: m.normal_scale,
            layering: m.layering,
        }),
        slots: m
            .slots
            .iter()
            .map(|s| {
                (
                    s.name.into(),
                    SlotRef {
                        texture: World::new().spawn(),
                        scale: s.scale,
                        offset: s.offset,
                        rotation: s.rotation,
                        texcoord: s.texcoord,
                        address: s.address,
                    },
                )
            })
            .collect(),
        unread: vec![],
    };
    Some((material, primase_shaunwhite::components::Stored(fields)))
}
