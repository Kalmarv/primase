//! `omit-materialless-primitives` through the pass manager, its drops checked against what it measurably left out.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::{Conservation, Measure};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::world::{Opaque, OpaqueData};
use primase_core::{Entity, MaterialVariants, Mesh, MeshFile, Primitive, Semantic, Stream, Topology, World};
use primase_meshops::OmitMateriallessPrimitives;

/// A triangle drawn with `material`.
fn triangle(material: Option<Entity>) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
        )]),
        indices: None,
        material,
    }
}

/// The file a loaded mesh of `n` primitives came from.
fn file(n: usize) -> MeshFile {
    MeshFile {
        path: "prototype.glb".into(),
        mesh: 0,
        materials: vec![None; n],
    }
}

fn run(w: &mut World) -> RunReport {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(OmitMateriallessPrimitives)],
            },
            &mut report,
        )
        .unwrap();
    report
}

#[test]
fn a_primitive_without_any_material_is_left_out_and_one_a_variant_draws_is_kept() {
    let mut w = World::new();
    let (m, variant) = (w.spawn(), w.spawn());
    w.insert(m, PbrMaterial::default());
    w.insert(variant, PbrMaterial::default());
    let e = w.spawn();
    w.insert(
        e,
        Mesh::new(vec![triangle(Some(m)), triangle(None), triangle(None)]),
    );
    w.insert(e, file(3));
    // the third has no material of its own, but a variant draws it
    w.insert(
        e,
        MaterialVariants(vec![
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(1, variant)]),
        ]),
    );
    let report = run(&mut w);
    let mesh = w.get::<Mesh>(e).unwrap();
    assert_eq!(
        mesh.primitives().iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(m), None]
    );
    // the per-primitive variants follow the primitives
    assert_eq!(
        w.get::<MaterialVariants>(e).unwrap().0,
        [BTreeMap::new(), BTreeMap::from([(1, variant)])]
    );
    // changed, the mesh is no longer its file's: no load restores the primitive left out
    assert!(!w.has::<MeshFile>(e));
    let log = &report.steps[0].log;
    assert_eq!(log.counts["file links of changed meshes removed"], 1);
    assert_eq!(log.declared(Measure::Primitives), 1);
    // a dropped primitive's triangles go with it (the ledger's triangles are those lost inside kept primitives)
    assert_eq!(log.declared(Measure::Triangles), 0);
    assert_eq!(log.counts["triangles of the primitives left out"], 1);
}

#[test]
fn a_mesh_left_with_no_primitive_goes_with_its_per_primitive_components() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![triangle(None), triangle(None)]));
    w.insert(e, MaterialVariants(vec![BTreeMap::new(), BTreeMap::new()]));
    w.insert(e, file(2));
    let report = run(&mut w);
    assert!(w.is_alive(e), "the entity stays, drawing nothing");
    // nothing a writer would refuse (a file link without its mesh), nothing a load would restore
    assert!(!w.has::<Mesh>(e) && !w.has::<MeshFile>(e));
    assert!(!w.has::<MaterialVariants>(e));
    let log = &report.steps[0].log;
    assert_eq!(log.declared(Measure::Primitives), 2);
    assert_eq!(log.counts["triangles of the primitives left out"], 2);
    assert_eq!(log.counts["meshes left with no primitive, removed"], 1);
    for name in ["core.mesh", "core.material_variants", "core.mesh_file"] {
        assert!(log.removed.contains(name), "{name}");
    }
}

#[test]
fn a_world_without_meshes_is_fine() {
    let mut w = World::new();
    let report = run(&mut w);
    assert_eq!(
        report.steps[0].log.counts["meshes with primitives without a material"],
        0
    );
}

/// A mesh the pass cannot change (it carries an opaque component) fails it, and nothing is recorded as done for it.
#[test]
fn a_mesh_it_cannot_change_is_not_recorded_as_changed() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![triangle(None)]));
    w.insert_opaque(
        e,
        "game.parts",
        Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![1]),
        },
    )
    .unwrap();
    let mut log = PassLog::default();
    let err = OmitMateriallessPrimitives.run(&mut w, &mut log).unwrap_err();
    assert!(err.contains("game.parts"), "{err}");
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(), 1);
    assert_eq!(log.declared(Measure::Primitives), 0);
    assert!(log.removed.is_empty());
    assert!(!log.counts.contains_key("triangles of the primitives left out"));
}
