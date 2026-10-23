//! The vertex layout number of a lowered mesh's primitives (`anvil.layout`, left by `lower-anvil-mesh`): on a mesh built
//! by hand (one number per primitive, the mesh's `kind`, kept when the primitives are rebuilt), and on real meshes of
//! Riders Republic's forges, where `resolve-anvil-shaders` binds each Material the same vertex stages before the mesh is
//! lowered (from its `anvil.mesh`) as after (from its `anvil.layout`).
use std::collections::BTreeSet;

use primase_anvil::components::{AnvilLayout, AnvilMaterial, AnvilMesh, VertexAttributes};
use primase_anvil::passes::{LowerAnvilMesh, MESH_LAYOUT_FIELD, RESOLVE_ANVIL_SHADERS, Static16Normal};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::mesh::MESH;
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::shader::{ShaderBinding, Stage};
use primase_core::{Component, Entity, Mesh, World};
use primase_testdata::{Game, install};

const PER_FORGE: usize = 120;

const ORDER: [&str; 6] = [
    "DataPC_MAD_WhiteRoom_patch_01.forge",
    "DataPC_MAD_WhiteRoom.forge",
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

fn lower(w: &mut World) -> Result<(), String> {
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(LowerAnvilMesh {
            static16_normal: Static16Normal::Omit,
        })],
    };
    manager
        .run(w, &pipeline, &mut RunReport::default())
        .map_err(|e| format!("{e:#}"))
}

/// Two quads of two triangles each in two ranges, each drawn with a Material of its own.
fn two_ranges(w: &mut World, kind: u8) -> (Entity, [Entity; 2]) {
    let materials = [w.spawn(), w.spawn()];
    let mesh = w.spawn();
    w.insert(
        mesh,
        AnvilMesh {
            entry: 0,
            index_id: 0,
            frame: SourceFrame::WORLD,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
            ranges: vec![0, 1],
            materials: vec![0, 1],
            material_entities: materials.iter().map(|&m| Some(m)).collect(),
            kind,
            stride: 0,
            vertex_records: Vec::new(),
            attributes: VertexAttributes::default(),
            skin: None,
        },
    );
    (mesh, materials)
}

#[test]
fn a_lowered_mesh_keeps_its_layout_number_for_every_primitive() {
    let mut w = World::new();
    let (mesh, _) = two_ranges(&mut w, 7);
    lower(&mut w).expect("lower");
    assert!(!w.has::<AnvilMesh>(mesh));
    assert_eq!(w.get::<AnvilLayout>(mesh).expect("a layout").kind, [7, 7]);
    assert_eq!(w.get::<Mesh>(mesh).expect("a mesh").primitives().len(), 2);
    assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
}

#[test]
fn the_layout_follows_the_primitives_it_describes() {
    let mut w = World::new();
    let (mesh, _) = two_ranges(&mut w, 3);
    lower(&mut w).expect("lower");
    let layout = w.get::<AnvilLayout>(mesh).expect("a layout").clone();
    let kept = layout.remap_primitives(&[1]).expect("answers").expect("remaps");
    assert_eq!(kept.kind, [3]);
    assert!(layout.remap_primitives(&[2]).expect("answers").is_err());
}

#[test]
fn a_layout_for_the_wrong_number_of_primitives_fails_its_check() {
    let mut w = World::new();
    let (mesh, _) = two_ranges(&mut w, 3);
    lower(&mut w).expect("lower");
    w.insert(mesh, AnvilLayout { kind: vec![3] });
    assert!(
        w.verify_components()
            .iter()
            .any(|(name, _, _)| *name == AnvilLayout::NAME)
    );
}

/// The vertex stages' keys of every Material the world binds, with its template: sorted.
fn stages(w: &World) -> Vec<(Option<String>, BTreeSet<u32>)> {
    let mut out: Vec<_> = w
        .iter::<ShaderBinding>()
        .map(|(e, b)| {
            let keys = b
                .stages
                .iter()
                .filter(|s| s.stage == Stage::Vertex)
                .map(|s| {
                    let key = s.variant.as_ref().expect("a vertex stage without its key");
                    assert_eq!(key.field, MESH_LAYOUT_FIELD);
                    key.value
                })
                .collect();
            (
                w.get::<AnvilMaterial>(e).and_then(|m| m.template_name.clone()),
                keys,
            )
        })
        .collect();
    out.sort();
    out
}

fn placed_world(archive: &mut Archive, meshes: &[At]) -> World {
    let mut w = World::new();
    let mut placed = Placed::default();
    {
        let mut placer = Placer::new(archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot)
            .with_materials()
            .with_shaders();
        for at in meshes {
            let _ = placer.place_object(*at, &mut placed);
        }
    }
    w
}

#[test]
fn the_same_vertex_stages_are_bound_before_and_after_lowering() {
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
    let options = Options::from([("unset-textures".to_owned(), "white".to_owned())]);
    let resolve = RESOLVE_ANVIL_SHADERS.instance(&options).expect("options");

    let mut before = placed_world(&mut archive, &meshes);
    resolve
        .run(&mut before, &mut PassLog::default())
        .expect("run before");

    let mut after = placed_world(&mut archive, &meshes);
    lower(&mut after).expect("lower");
    assert_eq!(after.iter::<AnvilMesh>().count(), 0);
    resolve
        .run(&mut after, &mut PassLog::default())
        .expect("run after");

    let (a, b) = (stages(&before), stages(&after));
    let with_vertex = a.iter().filter(|(_, k)| !k.is_empty()).count();
    println!(
        "{} meshes placed, {} Materials bound, {with_vertex} with a vertex stage; the sets of keys equal: {}",
        meshes.len(),
        a.len(),
        a == b
    );
    assert!(
        with_vertex > 0,
        "no Material had a vertex stage: the comparison measured nothing"
    );
    assert_eq!(a, b);
}
