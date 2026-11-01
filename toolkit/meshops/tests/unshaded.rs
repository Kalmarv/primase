//! `omit-unshaded-primitives` through the pass manager, its drops checked against what it measurably left out.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::{Conservation, Measure};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Entity, LayerWeights, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic,
    Stream, Topology, WeightMapping, World,
};
use primase_meshops::OmitUnshadedPrimitives;

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

fn file(n: usize) -> MeshFile {
    MeshFile {
        path: "prototype.glb".into(),
        mesh: 0,
        materials: vec![None; n],
    }
}

fn run(w: &mut World) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(OmitUnshadedPrimitives)],
            },
            &mut report,
        )
        .map_err(|e| format!("{e:?}"))?;
    Ok(report)
}

/// The materials: one with `pbr.material`, one with `core.layered`, and a reader's own one that no lowering shaded.
fn materials(w: &mut World) -> (Entity, Entity, Entity) {
    let (pbr, layered, unshaded) = (w.spawn(), w.spawn(), w.spawn());
    w.insert(pbr, PbrMaterial::default());
    w.insert(
        layered,
        LayeredMaterial {
            layers: vec![None; 4],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
        },
    );
    w.insert(unshaded, Name("signage".into()));
    (pbr, layered, unshaded)
}

#[test]
fn primitives_with_no_material_or_only_unshaded_ones_are_left_out_by_which() {
    let mut w = World::new();
    let (pbr, layered, unshaded) = materials(&mut w);
    let e = w.spawn();
    w.insert(
        e,
        Mesh::new(vec![
            triangle(Some(pbr)),
            triangle(Some(unshaded)),
            triangle(Some(layered)),
            triangle(None),
            // no material of its own, a shaded variant draws it
            triangle(None),
            // an unshaded material of its own and an unshaded variant
            triangle(Some(unshaded)),
        ]),
    );
    w.insert(
        e,
        MaterialVariants(vec![
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(1, pbr)]),
            BTreeMap::from([(1, unshaded)]),
        ]),
    );
    w.insert(e, file(6));
    let report = run(&mut w).unwrap();
    let mesh = w.get::<Mesh>(e).unwrap();
    assert_eq!(
        mesh.primitives().iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(pbr), Some(layered), None]
    );
    assert_eq!(
        w.get::<MaterialVariants>(e).unwrap().0,
        [BTreeMap::new(), BTreeMap::new(), BTreeMap::from([(1, pbr)])]
    );
    assert!(!w.has::<MeshFile>(e), "changed, the mesh is no longer its file's");
    let log = &report.steps[0].log;
    assert_eq!(log.declared(Measure::Primitives), 3);
    assert_eq!(
        log.drops[&(
            Measure::Primitives,
            "primitives whose materials have no shading (pbr.material or core.layered)".into()
        )],
        2
    );
    assert_eq!(
        log.drops[&(
            Measure::Primitives,
            "primitives without a material, of their own or a variant's".into()
        )],
        1
    );
    assert_eq!(
        log.counts["triangles of the primitives with unshaded materials left out"],
        2
    );
    assert_eq!(
        log.counts["triangles of the primitives without a material left out"],
        1
    );
    assert_eq!(log.counts["unshaded materials the primitives left out drew"], 1);
    assert_eq!(log.counts["meshes with unshaded primitives"], 1);
}

#[test]
fn a_mesh_left_with_no_primitive_goes_with_its_per_primitive_components() {
    let mut w = World::new();
    let (_, _, unshaded) = materials(&mut w);
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![triangle(Some(unshaded)), triangle(None)]));
    w.insert(e, file(2));
    let report = run(&mut w).unwrap();
    assert!(w.is_alive(e), "the entity stays, drawing nothing");
    assert!(!w.has::<Mesh>(e) && !w.has::<MeshFile>(e));
    let log = &report.steps[0].log;
    assert_eq!(log.declared(Measure::Primitives), 2);
    assert_eq!(log.counts["meshes left with no primitive, removed"], 1);
    for name in ["core.mesh", "core.mesh_file"] {
        assert!(log.removed.contains(name), "{name}");
    }
}

/// Leaving out only the unshaded variant would draw its placements with the primitive's own material: refused, with
/// nothing changed.
#[test]
fn a_primitive_drawn_shaded_and_unshaded_is_refused() {
    let mut w = World::new();
    let (pbr, _, unshaded) = materials(&mut w);
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![triangle(Some(pbr))]));
    w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, unshaded)])]));
    let err = run(&mut w).unwrap_err();
    assert!(
        err.contains("primitive 0") && err.contains("specialize-material-variants"),
        "{err}"
    );
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(), 1);
}

#[test]
fn a_world_with_every_primitive_shaded_is_unchanged() {
    let mut w = World::new();
    let (pbr, layered, _) = materials(&mut w);
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![triangle(Some(pbr)), triangle(Some(layered))]));
    w.insert(e, file(2));
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(), 2);
    assert!(w.has::<MeshFile>(e));
    assert_eq!(report.steps[0].log.counts["meshes with unshaded primitives"], 0);
    assert_eq!(report.steps[0].log.declared(Measure::Primitives), 0);
}
