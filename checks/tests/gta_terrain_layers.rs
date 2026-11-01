//! GTA's lookup-blended terrain (Vespucci Beach, which has a `*_cm*` and a `*_2tex_blend*` one) as layered materials
//! with per-pixel weight channels, written by the generic glTF writer. glTF blends no materials and the writer chooses
//! no layer: `gta-terrain-layers` gives a terrain material no `pbr.material` of its own, so each layered terrain
//! material the written placements draw is left out, counted (`layered_materials_unwritten`), its primitives drawn
//! with no material; its layers' materials, which no primitive draws, are reported as not written. Blended terrain
//! reaches glTF through a bake pass before the writer that makes the blend a `pbr.material` with baked textures.
//!
//! Needs `installs.gta` and `keys.gta`.
mod inputs;

use std::collections::BTreeSet;

use inputs::{Game, install, keys};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, LayeredMaterial, Mesh, World};
use primase_gta::read::source::GtaSource;
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
        "select-lod{form=finest}, gta-pose-fragments, lower-gta-pbr{night=false,water-colour=48,98,122,190}, gta-winding, gta-tint, gta-terrain-layers, \
         gta-vertex-colours, normalize-normals, fill-missing-texcoords",
    )?;
    m.run(&mut w, &part, &mut RunReport::default())?;
    Ok(w)
}

/// Every key of `v`'s objects named `name`, anywhere in it.
fn keys_of(v: &Value, name: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if k == name {
                    if let Some(inner) = x.as_object() {
                        out.extend(inner.keys().cloned());
                    }
                    if let Some(list) = x.as_array() {
                        out.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
                    }
                }
                keys_of(x, name, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| keys_of(x, name, out)),
        _ => {}
    }
}

#[test]
fn layered_terrain_is_left_out_of_the_generic_gltf_and_counted() -> Result<()> {
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
    let w = vespucci(&m, &source)?;
    let layered: Vec<(Entity, &LayeredMaterial)> = w.iter::<LayeredMaterial>().collect();
    let per_pixel = layered
        .iter()
        .filter(|(_, l)| l.weights.channels.is_some())
        .count();
    assert_eq!(per_pixel, 2, "Vespucci's lookup-blended terrain materials");
    // the terrain blend is the layered material alone: no pbr.material of its own to be its host
    assert!(layered.iter().all(|&(e, _)| !w.has::<PbrMaterial>(e)));
    let drawn: BTreeSet<Entity> = w
        .iter::<Mesh>()
        .flat_map(|(_, mesh)| {
            mesh.primitives()
                .iter()
                .filter_map(|p| p.material)
                .collect::<Vec<_>>()
        })
        .collect();
    let layers: BTreeSet<Entity> = layered
        .iter()
        .flat_map(|(_, l)| l.layers.iter().flatten().map(|layer| layer.material))
        .filter(|e| !drawn.contains(e))
        .collect();

    let (bytes, report) = primase_gltf::write(&w, &primase_gltf::WriteOptions::default())?;
    let doc: Value = serde_json::from_slice(primase_glb::parse(&bytes)?.json)?;
    // standard glTF alone: Khronos extensions, and no extras but a mesh's targetNames
    let (mut extensions, mut extras) = (BTreeSet::new(), BTreeSet::new());
    keys_of(&doc, "extensionsUsed", &mut extensions);
    keys_of(&doc, "extras", &mut extras);
    assert!(extensions.iter().all(|n| n.starts_with("KHR_")), "{extensions:?}");
    assert!(extras.iter().all(|k| k == "targetNames"), "{extras:?}");
    // the layered terrain the placements draw: left out, counted, its primitives drawn with no material
    let unwritten = report.layered_materials_unwritten;
    assert!(unwritten > 0 && unwritten <= layered.len() as u64, "{report}");
    assert!(report.primitives_without_material > 0, "{report}");
    assert!(!report.components_without_form.contains_key(LayeredMaterial::NAME));
    // the layers' own materials, which nothing else draws, are reported as not written
    let left = report
        .components_unwritten
        .get(PbrMaterial::NAME)
        .copied()
        .unwrap_or(0);
    assert!(
        left >= layers.len() as u64,
        "{left} of {} layer materials",
        layers.len()
    );
    println!(
        "Vespucci: {} layered terrain materials ({per_pixel} lookup-blended), {unwritten} drawn and left out; {} \
         primitives drawn with no material; {} layer materials not written",
        layered.len(),
        report.primitives_without_material,
        layers.len()
    );
    Ok(())
}
