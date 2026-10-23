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

use primase_anvil::components::{AnvilMaterial, AnvilMesh};
use primase_anvil::passes::{MESH_LAYOUT_FIELD, RESOLVE_ANVIL_SHADERS};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::mesh::MESH;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::shader::{ShaderBinding, Stage};
use primase_core::{Component, Entity, World};
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
