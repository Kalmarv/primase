//! Real selected-area assembly, including split geometry and authoritative PBR.
use primase_core::pass::RunReport;
use primase_core::pbr::PbrMaterial;
use primase_core::{DrawLayer, DrawOrder, Entity, MaterialUsage, Mesh, Name, Texture, World};
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::source::{FaceCull, MaterialSlots, PrototypeSource};
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_testdata::{Game, OracleSet, Tool, install, oodle, oracle};
use std::collections::BTreeMap;
mod common;
/// One read and bake of `region` of m11_01, its layers baked at most `max_size` pixels on an edge, its source
/// materials spawned in reverse when `reverse`.
fn bake(
    install: &std::path::Path,
    dll: &primase_compress::oodle::Oodle,
    region: [f32; 4],
    max_size: u32,
    reverse: bool,
) -> Result<(World, sekiro::Report, RunReport), Box<dyn std::error::Error>> {
    let mut world = World::default();
    let report = sekiro::read_area(
        install,
        "m11_01",
        &Decoder {
            oodle: Some(dll),
            max_output: 1024 * 1024 * 1024,
        },
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: region,
                margin: 30.0,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    // the bake's rows on the threads the configuration allows: the same bake on any count
    world.insert_resource(common::threads());
    if reverse {
        respawn_materials_reversed(&mut world);
    }
    let run = common::run(
        &mut world,
        &format!(
            "{}, resolve-fromsoft-shaders{{unbound-textures=preview}}, bake-fromsoft-layers{{max-size={max_size},sample-time=0}}, lower-fromsoft-materials{{layers=preview,water-colour=48,98,122,190}}",
            common::LEGACY_GEOMETRY
        ),
    )?;
    Ok((world, report, run))
}

/// What every region's bake holds, whatever region (`at` names it in a failure): each drawn primitive's material
/// lowered, two-sided exactly where its face set does not cull; the source classes carried as usage and draw order,
/// and counted by the lowering; baked textures within the size cap `max_size`; and the same names, textures,
/// materials and parts when the source materials are spawned in reverse. Returns (water, foliage, decal) counts.
fn region_holds(
    install: &std::path::Path,
    dll: &primase_compress::oodle::Oodle,
    region: [f32; 4],
    max_size: u32,
    at: &str,
) -> Result<(RunReport, [u64; 3]), Box<dyn std::error::Error>> {
    let (world, report, run) = bake(install, dll, region, max_size, false)?;
    let bake_log = &run.steps[run.steps.len() - 2].log;
    let pbr = &run.steps[run.steps.len() - 1].log;
    println!(
        "{at}: bake {bake_log:?}; PBR {pbr:?}; prototypes {}, placements {}",
        report.prototypes, report.placements
    );
    for (what, total) in [
        ("gloss", "bake groups with gloss"),
        ("emission", "bake groups with emission"),
    ] {
        let groups = bake_log.counts.get("bake groups").copied().unwrap_or(0);
        let with = bake_log.counts.get(total).copied().unwrap_or(0);
        let without = bake_log
            .counts
            .get(&total.replace("with", "without"))
            .copied()
            .unwrap_or(0);
        assert_eq!(
            with + without,
            groups,
            "{at}: bake groups with and without {what}"
        );
    }
    assert!(
        !pbr.approximations
            .contains_key("layered material lowered from its main-layer preview; layers not baked"),
        "{at}"
    );
    // every drawn primitive's material is lowered, and as sided as its face set: two-sided exactly where the source
    // face set does not cull back faces
    let mut sided = [0u64; 2];
    for (e, mesh) in world.iter::<Mesh>() {
        let cull = world
            .get::<FaceCull>(e)
            .ok_or_else(|| format!("{at}: a mesh without fromsoft.face_cull"))?;
        for (primitive, &cull) in mesh.primitives().iter().zip(&cull.primitives) {
            if let Some(material) = primitive.material {
                let pbr = world
                    .get::<PbrMaterial>(material)
                    .ok_or_else(|| format!("{at}: a drawn material not lowered"))?;
                assert_eq!(pbr.double_sided, !cull, "{at}");
                sided[usize::from(pbr.double_sided)] += 1;
            }
        }
    }
    println!(
        "{at}: primitives drawing single-sided {}, two-sided {}; split {:?}, single {:?}, two {:?}",
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
    let counted = |k: &str| pbr.counts.get(k).copied().unwrap_or(0);
    assert_eq!(usage(MaterialUsage::Water), class("water"), "{at}");
    assert_eq!(usage(MaterialUsage::Foliage), class("leaves"), "{at}");
    assert_eq!(decals, class("decal"), "{at}");
    assert_eq!(
        world.iter::<MaterialUsage>().count() as u64,
        class("water") + class("leaves"),
        "{at}"
    );
    assert_eq!(world.iter::<DrawOrder>().count() as u64, class("decal"), "{at}");
    assert_eq!(
        counted("materials of class water given their usage") + copies("water"),
        class("water"),
        "{at}"
    );
    assert_eq!(
        counted("materials of class leaves given their usage") + copies("leaves"),
        class("leaves"),
        "{at}"
    );
    assert_eq!(
        counted("materials of class decal given their draw order") + copies("decal"),
        class("decal"),
        "{at}"
    );
    for (entity, texture) in world.iter::<Texture>() {
        if world
            .get::<Name>(entity)
            .is_some_and(|n| n.0.starts_with("bake_"))
        {
            assert!(texture.width <= max_size && texture.height <= max_size, "{at}");
        }
    }
    let counts = [usage(MaterialUsage::Water), usage(MaterialUsage::Foliage), decals];

    // names follow the source, not the order entities were spawned in: the source materials spawned in reverse give
    // the same names, the same textures under them, the same materials and the same parts drawing them
    let first = Output::of(&world);
    let alike = first.alike_variants();
    println!(
        "{at}: bake variants: {}; sharing their content with another variant: {} (in {} sets)",
        first.materials.keys().filter(|n| n.contains("_source_")).count(),
        alike.iter().map(Vec::len).sum::<usize>(),
        alike.len()
    );
    drop(world);
    let (reversed, _, _) = bake(install, dll, region, max_size, true)?;
    let second = Output::of(&reversed);
    assert_eq!(first.textures.len(), second.textures.len(), "{at}");
    for (name, textures) in &first.textures {
        assert_eq!(
            Some(textures),
            second.textures.get(name),
            "{at}: textures named {name}"
        );
    }
    assert_eq!(first.materials, second.materials, "{at}");
    assert_eq!(first.parts, second.parts, "{at}");
    Ok((run, counts))
}

/// The baseline scene's region of m11_01, baked as the conversion bakes it, holds what any region's bake holds
/// ([`region_holds`]). The smoke gate ([`primase_testdata::depth`]) bakes a sample of the region's cells
/// ([`common::cells`]: [`SMOKE_CELLS`] of nine, drawn with [`primase_testdata::seed`], printed and named in every
/// failure) at [`SMOKE_MAX_SIZE`], so it checks nothing that depends on the resolution (the baked pixels, which
/// variants share their content); the full gate bakes the whole region at the conversion's 512 and checks its
/// counts, as measured.
#[test]
fn baked_slice_assembly() -> Result<(), Box<dyn std::error::Error>> {
    let Some(install) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let Some(reference) = oracle(OracleSet::Baseline, Game::Sekiro, "scene") else {
        return Ok(());
    };
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let region: [f32; 4] = serde_json::from_value(manifest["region"].clone())?;
    let dll = common::oodle(dll)?;
    if primase_testdata::depth() == primase_testdata::Depth::Smoke {
        let seed = primase_testdata::seed();
        let cells = common::cells(region);
        let sample = common::sample(seed, cells.len(), SMOKE_CELLS);
        println!(
            "smoke: {SMOKE_CELLS} of {} cells of the region, seed {seed} (rerun with [run] seed = '{seed}'): {sample:?}",
            cells.len()
        );
        for k in sample {
            region_holds(
                &install,
                &dll,
                cells[k],
                SMOKE_MAX_SIZE,
                &format!("seed {seed}, cell {k} {:?}", cells[k]),
            )?;
        }
        return Ok(());
    }
    let (run, counts) = region_holds(&install, &dll, region, 512, "the whole region")?;
    let bake = &run.steps[run.steps.len() - 2].log;
    let pbr = &run.steps[run.steps.len() - 1].log;
    assert_eq!(bake.counts["source materials baked"], 290);
    assert_eq!(bake.counts["bake groups"], 208);
    // 79 leaves materials lowered, and 3 two-sided copies of them (face sets that don't cull, split off): 82 drawn
    assert_eq!(pbr.counts["materials of class leaves given their usage"], 79);
    assert_eq!(counts, [1, 82, 4]);
    Ok(())
}

/// The cells of the region a smoke gate bakes.
const SMOKE_CELLS: usize = 2;

/// The smoke gate's bake size cap. A cell holds most of the region's materials (the centre one 1,121 of its 1,632 baked
/// textures), so a bake costs about what its groups' pixels cost, nearly all of it the programs' evaluation: what a
/// region's bake must hold does not depend on the resolution, and a 128 cap evaluates a sixteenth of the pixels.
const SMOKE_MAX_SIZE: u32 = 128;

/// Every source material moved to a new entity, the last first, and every reference to it with it: the same world,
/// its source materials spawned in the opposite order.
fn respawn_materials_reversed(world: &mut World) {
    let sources: Vec<_> = world.iter::<Material>().map(|(e, _)| e).collect();
    let mut moved = BTreeMap::new();
    for &old in sources.iter().rev() {
        let new = world.spawn();
        assert!(
            world
                .move_components(old, new)
                .expect("both alive, no geometry data")
        );
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
