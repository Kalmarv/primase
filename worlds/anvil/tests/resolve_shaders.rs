//! `resolve-anvil-shaders` on the Materials of meshes of the six forges (Riders Republic build 25394987): up to
//! `PER_FORGE` Meshes of every forge placed with their Materials and the Materials' template permutations, each bound
//! Material's `core.shader_binding` verified (`Component::verify`: its shaders, resources, lanes, offsets and variant
//! keys are the programs'), the Materials bound and left unbound counted with the reason, and each Material's vertex
//! stages held to the vertex layouts of the meshes drawn with it: one stage per distinct `anvil.mesh.kind`, each with
//! that `anvil.mesh.kind` as its variant key. Prints the counts.
//!
//! Also the Materials of a sample of every MaterialTemplate (up to `PER_TEMPLATE` each), read without meshes: bound
//! with the pixel stage only.
use std::collections::{BTreeMap, BTreeSet};

use anvil_formats::archive::{Archive, At};
use anvil_formats::build_table::BUILD_TABLE;
use anvil_formats::material::{MATERIAL, StoredMaterial};
use anvil_formats::mesh::MESH;
use anvil_formats::rider::{HAIR_COLOURS, colours};
use anvil_formats::skeleton::SKELETON;
use iridia_interface::Stage;
use primase_anvil::components::{AnvilMaterial, AnvilMesh};
use primase_anvil::passes::{MESH_LAYOUT_FIELD, RESOLVE_ANVIL_SHADERS};
use primase_anvil::read::placements::{
    Gender, InstancedLevels, Placed, Placer, RiderPartChoice, RiderSpec, Variant,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::shader::ShaderBinding;
use primase_core::{Component, Entity, Name, World};
use primase_testdata::{Game, install};

const PER_TEMPLATE: usize = 4;
const PER_FORGE: usize = 600;

const ORDER: [&str; 6] = [
    "DataPC_MAD_WhiteRoom_patch_01.forge",
    "DataPC_MAD_WhiteRoom.forge",
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

fn options() -> Options {
    Options::from([("unset-textures".to_owned(), "white".to_owned())])
}

fn print(log: &PassLog) {
    for (what, n) in &log.counts {
        println!("  {n:>6}  {what}");
    }
    for (what, n) in &log.approximations {
        println!("  {n:>6}  (approximation) {what}");
    }
}

fn verify(world: &World, e: Entity) {
    let b = world.get::<ShaderBinding>(e).expect("a binding");
    b.verify(e, world).unwrap_or_else(|why| {
        panic!(
            "{:?}: {why}",
            world
                .get::<AnvilMaterial>(e)
                .and_then(|m| m.template_name.clone())
        )
    });
}

/// The vertex layout numbers of the meshes drawn with each Material.
fn layouts_of(world: &World) -> BTreeMap<Entity, BTreeSet<u8>> {
    let mut out: BTreeMap<Entity, BTreeSet<u8>> = BTreeMap::new();
    for (_, mesh) in world.iter::<AnvilMesh>() {
        for m in mesh.material_entities.iter().flatten() {
            out.entry(*m).or_default().insert(mesh.kind);
        }
    }
    out
}

#[test]
fn a_materials_vertex_stages_are_its_meshes_layouts() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let mut meshes = Vec::new();
    for f in 0..ORDER.len() {
        let forge = archive.forge(f).expect("forge");
        meshes.extend(
            (0..forge.names.len())
                .filter(|&k| forge.names[k].type_id == MESH)
                .take(PER_FORGE)
                .map(|entry| At { forge: f, entry }),
        );
    }
    let mut w = World::new();
    let mut placed = Placed::default();
    {
        let mut placer = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot)
            .with_materials()
            .with_shaders();
        for at in &meshes {
            let _ = placer.place_object(*at, &mut placed);
        }
    }
    let layouts = layouts_of(&w);
    let mut log = PassLog::default();
    RESOLVE_ANVIL_SHADERS
        .instance(&options())
        .expect("options")
        .run(&mut w, &mut log)
        .expect("run");
    let mut by_vertex_stages: BTreeMap<usize, usize> = BTreeMap::new();
    let mut checked = 0usize;
    for (&material, kinds) in &layouts {
        let Some(binding) = w.get::<ShaderBinding>(material) else {
            continue;
        };
        verify(&w, material);
        // one vertex stage per layout the material is drawn with, each keyed by it; one pixel stage
        let vertex: BTreeSet<u32> = binding
            .stages
            .iter()
            .filter(|s| s.stage == Stage::Vertex)
            .map(|s| {
                let key = s.variant.as_ref().expect("a vertex stage without its key");
                assert_eq!(key.field, MESH_LAYOUT_FIELD);
                key.value
            })
            .collect();
        let vertex_stages = binding.stages.iter().filter(|s| s.stage == Stage::Vertex).count();
        assert_eq!(vertex.len(), vertex_stages, "a layout bound twice");
        let pixel: Vec<_> = binding
            .stages
            .iter()
            .filter(|s| s.stage == Stage::Pixel)
            .collect();
        assert_eq!(pixel.len(), 1);
        assert!(pixel.iter().all(|s| s.variant.is_none()));
        // a layout whose vertex permutation the template lacks is counted, not bound: the stages are a subset
        let wanted: BTreeSet<u32> = kinds.iter().map(|k| u32::from(*k)).collect();
        assert!(vertex.is_subset(&wanted), "{vertex:?} of {wanted:?}");
        *by_vertex_stages.entry(vertex_stages).or_default() += 1;
        checked += 1;
    }
    println!(
        "{} meshes placed, {} Materials drawn by them, {checked} bound; by number of vertex stages {by_vertex_stages:?}",
        w.iter::<AnvilMesh>().count(),
        layouts.len()
    );
    print(&log);
    assert!(checked > 100, "{checked}");
    assert!(
        by_vertex_stages.keys().any(|n| *n >= 2),
        "no Material was drawn with two layouts: {by_vertex_stages:?}"
    );
}

#[test]
fn a_sample_of_every_template_is_bound_or_left_with_a_reason() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    // Material ids by their template's name, a few each
    let mut by_template: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for f in 0..ORDER.len() {
        let forge = archive.forge(f).expect("forge");
        let entries: Vec<(usize, u64)> = (0..forge.names.len())
            .filter(|&k| forge.names[k].type_id == MATERIAL)
            .map(|k| (k, forge.index[k].id))
            .collect();
        for (entry, id) in entries {
            let body = archive.resource(At { forge: f, entry }).expect("material").body;
            let Ok(m) = StoredMaterial::parse(&body) else {
                continue;
            };
            let Some(t) = m.references[0].and_then(|t| archive.find(t.id)) else {
                continue;
            };
            let name = archive.forge(t.forge).expect("forge").names[t.entry].name.clone();
            let ids = by_template.entry(name).or_default();
            if ids.len() < PER_TEMPLATE && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let ids: Vec<u64> = by_template.values().flatten().copied().collect();
    println!("{} templates, {} Materials sampled", by_template.len(), ids.len());
    let mut w = World::new();
    let mut placed = Placed::default();
    let materials: Vec<Entity> = {
        let mut placer =
            Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot).with_shaders();
        ids.iter()
            .filter_map(|&id| placer.material(id, &mut placed))
            .collect()
    };
    println!(
        "read {} Materials ({} refused)",
        materials.len(),
        placed.refused.len()
    );
    let mut log = PassLog::default();
    RESOLVE_ANVIL_SHADERS
        .instance(&options())
        .expect("options")
        .run(&mut w, &mut log)
        .expect("run");
    let mut bound = 0;
    for &e in &materials {
        if let Some(b) = w.get::<ShaderBinding>(e) {
            verify(&w, e);
            // no mesh is drawn with them: the pixel stage only
            assert!(b.stages.iter().all(|s| s.stage == Stage::Pixel));
            bound += 1;
        }
    }
    println!("{bound} bound with the pixel stage only");
    print(&log);
    assert!(bound > 0);
}

/// A rider's hair Material tests alpha (AlphaTestEnabled, value 32): its pixel stage is the template's alpha-tested
/// permutation (key `0x404000`), the one that discards by Mask1's alpha, with the vertex stage of each layout drawn; the
/// pass counts it as inferred.
#[test]
fn a_hair_material_is_bound_to_the_alpha_tested_permutation() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let id_of = |a: &Archive, type_id: u32, name: &str| -> u64 {
        (0..ORDER.len())
            .find_map(|f| {
                a.forge(f)?
                    .entries()
                    .find(|(_, n)| n.type_id == type_id && n.name == name)
                    .map(|(i, _)| i.id)
            })
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let list = colours(&mut archive, HAIR_COLOURS).expect("hair colours");
    let colour = list
        .iter()
        .find(|c| c.name == "MAD_CHR_HairCut_V001_Blond")
        .expect("a hair colour");
    let spec = RiderSpec {
        name: "rider".to_owned(),
        skeletons: ["MAD_CHR_Skel_U", "MAD_CHR_SkelAddon_U", "MAD_CHR_SkelAddon_M"]
            .iter()
            .map(|n| id_of(&archive, SKELETON, n))
            .collect(),
        gender: Gender::Male,
        parts: vec![
            RiderPartChoice {
                table: id_of(&archive, BUILD_TABLE, "MAD_CHR_HEAD_000_U"),
                variant: Variant::Stated,
            },
            RiderPartChoice {
                table: id_of(&archive, BUILD_TABLE, "MAD_CHR_Hair_005_U_Full"),
                variant: Variant::Row {
                    table: colour.table,
                    condition: colour.condition,
                },
            },
        ],
    };
    let mut w = World::new();
    let mut placed = Placed::default();
    Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    )
    .with_materials()
    .with_shaders()
    .place_rider(&spec, &mut placed);
    let mut log = PassLog::default();
    RESOLVE_ANVIL_SHADERS
        .instance(&options())
        .expect("options")
        .run(&mut w, &mut log)
        .expect("run");
    print(&log);
    let hair = w
        .iter::<AnvilMaterial>()
        .find(|(e, _)| {
            w.get::<Name>(*e)
                .is_some_and(|n| n.0 == "MAD_CHR_HairCut_V001_Blond")
        })
        .map(|(e, _)| e)
        .expect("the hair Material");
    verify(&w, hair);
    let binding = w.get::<ShaderBinding>(hair).expect("bound");
    let name = |e: Entity| w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default();
    let pixel: Vec<String> = binding
        .stages
        .iter()
        .filter(|s| s.stage == Stage::Pixel)
        .map(|s| name(s.shader))
        .collect();
    assert_eq!(pixel, ["MAD_CHR_Hair_Anisotropic-404000.ps"]);
    assert!(
        binding.stages.iter().any(|s| s.stage == Stage::Vertex),
        "no vertex stage bound"
    );
    assert!(
        log.counts
            .keys()
            .any(|k| k.contains("bound to their template's alpha-tested permutation")),
        "{:?}",
        log.counts
    );
}
