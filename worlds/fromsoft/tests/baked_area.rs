//! Real selected-area assembly, including split geometry and authoritative PBR.
use primase_core::pass::RunReport;
use primase_core::pbr::PbrMaterial;
use primase_core::{DrawLayer, DrawOrder, Entity, MaterialUsage, Mesh, Name, Texture, World};
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::source::{FaceCull, MaterialSlots, PrototypeSource};
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_testdata::real_data;
use std::collections::BTreeMap;
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
    let read = |reverse: bool| -> Result<(World, sekiro::Report, RunReport), Box<dyn std::error::Error>> {
        let mut world = World::default();
        let report = sekiro::read_area(
            &install,
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
        if reverse {
            respawn_materials_reversed(&mut world);
        }
        let run = common::run(
            &mut world,
            &format!(
                "{}, bake-fromsoft-layers{{max-size=512,sample-time=0}}, lower-fromsoft-materials{{layers=preview}}",
                common::LEGACY_GEOMETRY
            ),
        )?;
        Ok((world, report, run))
    };
    let (world, report, run) = read(false)?;
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
    // every drawn primitive's material is lowered, and as sided as its face set: two-sided exactly where the source
    // face set does not cull back faces
    let mut sided = [0u64; 2];
    for (e, mesh) in world.iter::<Mesh>() {
        let cull = world
            .get::<FaceCull>(e)
            .ok_or("a mesh without fromsoft.face_cull")?;
        for (primitive, &cull) in mesh.primitives().iter().zip(&cull.primitives) {
            if let Some(material) = primitive.material {
                let pbr = world
                    .get::<PbrMaterial>(material)
                    .ok_or("a drawn material not lowered")?;
                assert_eq!(pbr.double_sided, !cull);
                sided[usize::from(pbr.double_sided)] += 1;
            }
        }
    }
    println!(
        "primitives drawing single-sided {}, two-sided {}; split {:?}, single {:?}, two {:?}",
        sided[0],
        sided[1],
        pbr.counts.get("materials split by face-set cull"),
        pbr.counts
            .get("materials single-sided: their face sets cull back faces"),
        pbr.counts.get("materials two-sided: their face sets do not cull")
    );
    // what the source classes state about the surfaces, on the materials drawn: the reader's class, counted by the
    // lowering, and nothing else carries it
    let class = |c: &str| {
        world
            .iter::<Material>()
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
    let (water, foliage, decal_count) = (usage(MaterialUsage::Water), usage(MaterialUsage::Foliage), decals);
    // the lowering counts what it lowered; a material it split by face-set cull has a copy that carries the same
    let copies = |c: &str| {
        world
            .iter::<Material>()
            .filter(|(e, m)| {
                m.class == c
                    && world.has::<PbrMaterial>(*e)
                    && world
                        .get::<Name>(*e)
                        .is_some_and(|n| n.0.ends_with("_two_sided") && !n.0.starts_with("bake_"))
            })
            .count() as u64
    };
    assert_eq!(usage(MaterialUsage::Water), class("water"));
    assert_eq!(usage(MaterialUsage::Foliage), class("leaves"));
    assert_eq!(decals, class("decal"));
    assert_eq!(
        world.iter::<MaterialUsage>().count() as u64,
        class("water") + class("leaves")
    );
    assert_eq!(world.iter::<DrawOrder>().count() as u64, class("decal"));
    assert_eq!(
        pbr.counts["materials of class water given their usage"] + copies("water"),
        class("water")
    );
    assert_eq!(
        pbr.counts["materials of class leaves given their usage"] + copies("leaves"),
        class("leaves")
    );
    assert_eq!(
        pbr.counts["materials of class decal given their draw order"] + copies("decal"),
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

    // names follow the source, not the order entities were spawned in: the source materials spawned in reverse give
    // the same names, the same textures under them, the same materials and the same parts drawing them
    let first = Output::of(&world);
    let alike = first.alike_variants();
    println!(
        "bake variants: {}; sharing their content with another variant: {} (in {} sets)",
        first.materials.keys().filter(|n| n.contains("_source_")).count(),
        alike.iter().map(Vec::len).sum::<usize>(),
        alike.len()
    );
    drop(world);
    let (reversed, _, _) = read(true)?;
    let second = Output::of(&reversed);
    assert_eq!(first.textures.len(), second.textures.len());
    for (name, textures) in &first.textures {
        assert_eq!(Some(textures), second.textures.get(name), "textures named {name}");
    }
    assert_eq!(first.materials, second.materials);
    assert_eq!(first.parts, second.parts);
    assert_eq!((water, foliage, decal_count), (1, 79, 4));
    Ok(())
}

/// Every source material moved to a new entity, the last first, and every reference to it with it: the same world,
/// its source materials spawned in the opposite order.
fn respawn_materials_reversed(world: &mut World) {
    let sources: Vec<_> = world.iter::<Material>().map(|(e, _)| e).collect();
    let mut moved = BTreeMap::new();
    for &old in sources.iter().rev() {
        let new = world.spawn();
        assert!(world.move_components(old, new).unwrap());
        moved.insert(old, new);
    }
    let meshes: Vec<_> = world.iter::<Mesh>().map(|(e, _)| e).collect();
    for e in meshes {
        let mesh = world.get_mut::<Mesh>(e).expect("listed");
        for mut primitive in mesh.primitives_mut() {
            if let Some(m) = primitive.material_mut()
                && let Some(&new) = moved.get(m)
            {
                *m = new;
            }
        }
    }
    let slotted: Vec<_> = world.iter::<MaterialSlots>().map(|(e, _)| e).collect();
    for e in slotted {
        for m in &mut world.get_mut::<MaterialSlots>(e).expect("listed").0 {
            if let Some(&new) = moved.get(m) {
                *m = new;
            }
        }
    }
    // nothing refers to a source material's old entity
    for e in world.entities().collect::<Vec<_>>() {
        for r in world.references(e) {
            assert!(world.is_alive(r), "{e:?} refers to {r:?}");
        }
    }
}

/// What a run produced, by name: the bytes of the textures of each name, the materials of each name with their
/// textures by name, and each prototype's parts by the names of the materials they draw with and a digest of their
/// geometry. A name may stand for several: the reader names a source material for its base texture or definition,
/// which source materials share; what a pass derives is named uniquely (checked for the bake's variants).
#[derive(Debug, PartialEq)]
struct Output {
    textures: BTreeMap<String, Vec<String>>,
    materials: BTreeMap<String, Vec<String>>,
    parts: BTreeMap<String, Vec<(Option<String>, String)>>,
}
impl Output {
    fn of(world: &World) -> Output {
        let digest = |s: String| primase_fromsoft::materials::identity_digest(&s);
        let name = |e: Entity| {
            world
                .get::<Name>(e)
                .map_or_else(|| format!("unnamed {e:?}"), |n| n.0.clone())
        };
        let mut textures = BTreeMap::<String, Vec<String>>::new();
        for (e, t) in world.iter::<Texture>() {
            textures
                .entry(name(e))
                .or_default()
                .push(digest(format!("{t:?}")));
        }
        let mut materials = BTreeMap::<String, Vec<String>>::new();
        for (e, m) in world.iter::<PbrMaterial>() {
            let mut m = m.clone();
            let mut bound = Vec::new();
            let refs = [
                &mut m.base_color_texture,
                &mut m.metallic_roughness_texture,
                &mut m.occlusion_texture,
                &mut m.normal_texture,
                &mut m.emissive_texture,
            ];
            for r in refs.into_iter().flatten() {
                bound.push(name(r.texture));
                r.texture = Entity::from_index(0);
            }
            if let Some(s) = &mut m.specular {
                for r in [&mut s.texture, &mut s.color_texture].into_iter().flatten() {
                    bound.push(name(r.texture));
                    r.texture = Entity::from_index(0);
                }
            }
            materials
                .entry(name(e))
                .or_default()
                .push(format!("{m:?} {bound:?}"));
        }
        for (name, contents) in textures.iter_mut().chain(materials.iter_mut()) {
            assert!(
                !name.starts_with("bake_") || contents.len() == 1,
                "{} derived entities named {name}",
                contents.len()
            );
            contents.sort();
        }
        let mut parts = BTreeMap::new();
        for (e, source) in world.iter::<PrototypeSource>() {
            let Some(mesh) = world.get::<Mesh>(e) else {
                continue;
            };
            let drawn = mesh
                .primitives()
                .iter()
                .map(|p| {
                    let mut geometry = p.clone();
                    geometry.material = None;
                    (p.material.map(name), digest(format!("{geometry:?}")))
                })
                .collect();
            parts.insert(source.id.clone(), drawn);
        }
        Output {
            textures,
            materials,
            parts,
        }
    }
    /// The bake variants (`bake_<group>_<level>_source_<digest>`) whose material is another variant's, in sets.
    fn alike_variants(&self) -> Vec<Vec<String>> {
        let mut by_content = BTreeMap::<&String, Vec<String>>::new();
        for (name, m) in &self.materials {
            if name.contains("_source_") {
                by_content.entry(&m[0]).or_default().push(name.clone());
            }
        }
        by_content.into_values().filter(|v| v.len() > 1).collect()
    }
}
