//! GTA's lookup-blended terrain (Vespucci Beach, which has a `*_cm*` and a `*_2tex_blend*` one) as layered materials with per-pixel weight channels: the glTF writer
//! writes them (`PRIMASE_layered_material` version 2, the lookup a mask at TEXCOORD_1), and after `bake-layer-channels`
//! they are vertex-weighted (version 1, a colour attribute) for a destination that blends by vertex weights.
//!
//! Needs `installs.gta` and `keys.gta`.
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{LayeredMaterial, World};
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Vespucci read, selected and loaded, through the GTA passes up to the terrain layers and vertex colours, the
/// texture coordinate sets a material samples and a primitive lacks given zeros (glTF requires them).
fn vespucci(m: &PassManager, source: &GtaSource) -> Result<World> {
    let region = [-1500.0, -1650.0, -1150.0, -1330.0];
    let (mut read, _) = source.read(region, 0.0)?;
    let select = m.registry.parse(
        "resolve-state{assign=gta.map:online;gta.hour:12}, \
         gta-select-region{x0=-1500,y0=-1650,x1=-1150,y1=-1330,min_size=0.5,max_priority=3}, gta-leave-out-interiors",
    )?;
    m.run(&mut read, &select, &mut RunReport::default())?;
    let (mut w, _) = source.load_all(&read)?;
    let part = m.registry.parse(
        "select-lod{form=finest}, gta-pose-fragments, lower-gta-pbr{night=false}, gta-winding, gta-tint, gta-terrain-layers, \
         gta-vertex-colours, normalize-normals, fill-missing-texcoords",
    )?;
    m.run(&mut w, &part, &mut RunReport::default())?;
    Ok(w)
}

/// The written glTF's layered materials' `PRIMASE_layered_material` objects.
fn layered(w: &World) -> Result<Vec<Value>> {
    let (doc, _) = primase_gltf::write_document(w, &primase_gltf::WriteOptions::default())?;
    Ok(doc
        .json
        .get("materials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.pointer("/extensions/PRIMASE_layered_material").cloned())
        .collect())
}

#[test]
fn lookup_terrain_is_written_per_pixel_and_bakes_to_vertex_weights() -> Result<()> {
    let (Some(install), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return Ok(());
    };
    let source = GtaSource::open(&install, &keys)?;
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_image::register(&mut registry);
    primase_gta::register(&mut registry);
    primase_gltf::register(&mut registry);
    let m = PassManager::new(registry);
    let mut w = vespucci(&m, &source)?;
    let per_pixel = w
        .iter::<LayeredMaterial>()
        .filter(|(_, l)| l.weights.channels.is_some())
        .count();
    assert_eq!(per_pixel, 2, "Vespucci's lookup-blended terrain materials");
    // written per pixel: rgb_product_4 of COLOR_1's red and the lookup's green and blue at TEXCOORD_1
    let written: Vec<Value> = layered(&w)?
        .into_iter()
        .filter(|l| l["weights"].get("channels").is_some())
        .collect();
    assert_eq!(written.len(), per_pixel);
    for l in &written {
        assert_eq!(l["version"], 2, "{l}");
        assert_eq!(l["weights"]["mapping"], "rgb_product_4", "{l}");
        let channels = l["weights"]["channels"].as_array().ok_or("channels")?;
        assert_eq!(channels.len(), 3, "{l}");
        assert_eq!(
            channels[0]["source"]["vertex"],
            serde_json::json!({"attribute": "COLOR_1", "channel": 0})
        );
        for (c, k) in [(&channels[1], 1), (&channels[2], 2)] {
            let mask = c
                .pointer("/source/mask")
                .or_else(|| c.pointer("/lerp/a/mask"))
                .ok_or("a lookup mask")?;
            assert_eq!(mask["texture"]["texCoord"], 1, "{c}");
            let mut want = [0.0; 4];
            want[k] = 1.0;
            assert_eq!(mask["channels"], serde_json::json!(want), "{c}");
            if let Some(lerp) = c.get("lerp") {
                assert_eq!(
                    lerp["b"]["vertex"],
                    serde_json::json!({"attribute": "COLOR_1", "channel": k})
                );
                assert_eq!(
                    lerp["t"]["vertex"],
                    serde_json::json!({"attribute": "COLOR_0", "channel": 3})
                );
            }
        }
    }
    println!(
        "Vespucci: {per_pixel} lookup-blended terrain materials, {} mixed by vertex alpha",
        written.iter().filter(|l| l.to_string().contains("lerp")).count()
    );
    // baked to vertex weights: no channels left, every layered material a colour attribute
    let bake = m.registry.parse("bake-layer-channels")?;
    m.run(&mut w, &bake, &mut RunReport::default())?;
    assert!(w.iter::<LayeredMaterial>().all(|(_, l)| !l.weights.per_pixel()));
    for l in layered(&w)? {
        assert!(l["weights"].get("channels").is_none(), "{l}");
        assert!(l.get("version").is_none(), "{l}");
    }
    Ok(())
}
