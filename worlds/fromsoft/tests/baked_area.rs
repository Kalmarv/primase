//! Real selected-area assembly, including split geometry and authoritative PBR.
use primase_core::pbr::PbrMaterial;
use primase_core::{DrawLayer, DrawOrder, MaterialUsage, Mesh, Name, Texture, World};
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_testdata::real_data;
mod common;
#[test]
fn baked_slice_assembly() -> Result<(), Box<dyn std::error::Error>> {
    let Some(install) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let dll = common::oodle(dll)?;
    let mut world = World::default();
    let report = sekiro::read_area(
        install,
        "m11_01",
        &Decoder {
            oodle: Some(&dll),
            max_output: 1024 * 1024 * 1024,
        },
        &sekiro::Options {
            region: Some(serde_json::from_value(manifest["region"].clone())?),
            ..Default::default()
        },
        &mut world,
    )?;
    let run = common::run(
        &mut world,
        &format!(
            "{}, bake-fromsoft-layers{{max-size=512,sample-time=0}}, lower-fromsoft-materials{{layers=preview}}",
            common::LEGACY_GEOMETRY
        ),
    )?;
    let bake = &run.steps[run.steps.len() - 2].log;
    let pbr = &run.steps[run.steps.len() - 1].log;
    println!(
        "bake {bake:?}; PBR {pbr:?}; prototypes {}, placements {}",
        report.prototypes, report.placements
    );
    assert_eq!(bake.counts["source materials baked"], 290);
    assert_eq!(bake.counts["bake groups"], 208);
    assert_eq!(
        bake.counts["bake groups with gloss"]
            + bake.counts.get("bake groups without gloss").copied().unwrap_or(0),
        208
    );
    assert_eq!(
        bake.counts["bake groups with emission"]
            + bake
                .counts
                .get("bake groups without emission")
                .copied()
                .unwrap_or(0),
        208
    );
    assert!(
        !pbr.approximations
            .contains_key("layered material lowered from its main-layer preview; layers not baked")
    );
    for (_, mesh) in world.iter::<Mesh>() {
        for primitive in mesh.primitives() {
            if let Some(material) = primitive.material {
                assert!(world.has::<PbrMaterial>(material));
            }
        }
    }
    // what the source classes state about the surfaces, on the materials drawn: the reader's class, counted by the
    // lowering, and nothing else carries it
    let class = |c: &str| {
        world
            .iter::<primase_fromsoft::materials::Material>()
            .filter(|(e, m)| m.class == c && world.has::<PbrMaterial>(*e))
            .count() as u64
    };
    let usage = |u: MaterialUsage| world.iter::<MaterialUsage>().filter(|(_, x)| **x == u).count() as u64;
    let decals = world
        .iter::<DrawOrder>()
        .filter(|(_, d)| d.layer == DrawLayer::Decal)
        .count() as u64;
    println!(
        "water {}, leaves {}, decals {}",
        usage(MaterialUsage::Water),
        usage(MaterialUsage::Foliage),
        decals
    );
    assert_eq!(
        (usage(MaterialUsage::Water), usage(MaterialUsage::Foliage), decals),
        (1, 79, 4)
    );
    assert_eq!(usage(MaterialUsage::Water), class("water"));
    assert_eq!(usage(MaterialUsage::Foliage), class("leaves"));
    assert_eq!(decals, class("decal"));
    assert_eq!(
        world.iter::<MaterialUsage>().count() as u64,
        class("water") + class("leaves")
    );
    assert_eq!(world.iter::<DrawOrder>().count() as u64, class("decal"));
    assert_eq!(
        pbr.counts["materials of class water given their usage"],
        class("water")
    );
    assert_eq!(
        pbr.counts["materials of class leaves given their usage"],
        class("leaves")
    );
    assert_eq!(
        pbr.counts["materials of class decal given their draw order"],
        class("decal")
    );
    for (entity, texture) in world.iter::<Texture>() {
        if world
            .get::<Name>(entity)
            .is_some_and(|n| n.0.starts_with("bake_"))
        {
            assert!(texture.width <= 512 && texture.height <= 512);
        }
    }
    Ok(())
}
