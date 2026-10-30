//! Elden Ring's vertex colours lowered as the inputs their semantic index names, on Stormveil (`m10_00_00_00`, the
//! region the lab opens around its courtyard), its geometry lowered and its shaders resolved as the lab prepares an area.
//!
//! A FLVER colour member is `COLOR0` or `COLOR1` by its semantic index, not by its position: most of Stormveil's map
//! meshes store one colour, indexed 1 (in `m002002`, 18 of its 19 meshes, each `(1, 0, 0, 1)` at every vertex). The
//! courtyard floor's dark slabs (`m002002`'s `m10_00_246` mesh) are drawn with `M[AMSN_V]`, whose vertex program reads
//! `COLOR1` and no `COLOR0`: lowered as the first colour, the stored value reached no input the program reads.
//!
//! Checked: that mesh carries its colour as the second colour (`Color(9)`), `(1, 0, 0, 1)`, and no first colour; the
//! one mesh of `m002002` that stores both (`m10_00_046`) carries both; and no drawn primitive's vertex program reads a
//! colour the primitive does not carry.
mod common;
mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use iridia_interface::Stage;
use primase_core::shader::{Attribute, InputRole, ShaderBinding};
use primase_core::{Mesh, Name, Semantic, Stream, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::SECOND_COLOUR;
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The lab's preparation of an Elden Ring area up to its shaders (the colours are lowered with the geometry and read by
/// the bindings the resolution states).
const PASSES: [&str; 4] = [
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
];

/// The colour streams of the primitives of the prototype `prototype` drawn with the material binary `matbin`.
fn colours(world: &World, prototype: &str, matbin: &str) -> Vec<BTreeMap<Semantic, Vec<[f32; 4]>>> {
    world
        .iter::<Mesh>()
        .filter(|&(e, _)| world.get::<Name>(e).is_some_and(|n| n.0 == prototype))
        .flat_map(|(_, m)| m.primitives())
        .filter(|p| {
            p.material
                .and_then(|e| world.get::<Material>(e))
                .is_some_and(|m| m.mtd == matbin)
        })
        .map(|p| {
            p.streams
                .iter()
                .filter(|(s, _)| matches!(s, Semantic::Color(_)))
                .map(|(s, v)| {
                    let Stream::F32x4(values) = v else {
                        panic!("{prototype} {matbin}: colour {s:?} is not four floats");
                    };
                    (*s, values.clone())
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_colour_is_the_input_its_semantic_index_names() -> Result<()> {
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
        "m10_00_00_00",
        &decoder,
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: [-230.0, -110.0, 30.0, 90.0],
                margin: 30.0,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    for pass in PASSES {
        common::run(&mut world, pass)?;
    }

    // the slabs: COLOR1 alone, carried as the second colour
    let slabs = colours(&world, "m002002", "m10_00_246");
    assert_eq!(slabs.len(), 1, "m002002's m10_00_246 primitives");
    let streams: Vec<Semantic> = slabs[0].keys().copied().collect();
    assert_eq!(streams, [SECOND_COLOUR], "m002002 m10_00_246");
    assert!(
        slabs[0][&SECOND_COLOUR]
            .iter()
            .all(|c| *c == [1.0, 0.0, 0.0, 1.0]),
        "m002002 m10_00_246: COLOR1 is not (1, 0, 0, 1) at every vertex"
    );
    // a mesh that stores both colours carries both
    let both = colours(&world, "m002002", "m10_00_046");
    assert_eq!(both.len(), 1, "m002002's m10_00_046 primitives");
    let streams: BTreeSet<Semantic> = both[0].keys().copied().collect();
    assert_eq!(
        streams,
        BTreeSet::from([Semantic::Color(0), SECOND_COLOUR]),
        "m002002 m10_00_046"
    );

    // no drawn primitive's vertex program reads a colour the primitive does not carry
    let read = [Attribute::Color(0), Attribute::from(SECOND_COLOUR)];
    let (mut supplied, mut variants) = (BTreeMap::<String, u64>::new(), 0u64);
    let mut lacking = BTreeMap::<String, u64>::new();
    for (_, mesh) in world.iter::<Mesh>() {
        for p in mesh.primitives() {
            let Some(binding) = p.material.and_then(|e| world.get::<ShaderBinding>(e)) else {
                continue;
            };
            let carried: BTreeSet<Attribute> = p.streams.keys().map(|&s| Attribute::from(s)).collect();
            for stage in binding.stages.iter().filter(|s| s.stage == Stage::Vertex) {
                if stage.variant.is_some() {
                    variants += 1;
                    continue;
                }
                for input in &stage.inputs {
                    let InputRole::Stream(a) = &input.role else {
                        continue;
                    };
                    if !read.contains(a) {
                        continue;
                    }
                    let key = format!("{}{} as {a:?}", input.label.name, input.label.index);
                    if carried.contains(a) {
                        *supplied.entry(key).or_default() += 1;
                    } else {
                        let name = p
                            .material
                            .and_then(|e| world.get::<Material>(e))
                            .map_or_else(String::new, |m| m.mtd.clone());
                        *lacking.entry(format!("{key}: {name}")).or_default() += 1;
                    }
                }
            }
        }
    }
    println!("colour reads supplied {supplied:?}; vertex stages of a variant not counted: {variants}");
    assert!(!supplied.is_empty(), "no primitive's program reads a colour");
    assert_eq!(lacking, BTreeMap::new(), "colour reads with no stream");
    Ok(())
}
