//! Real MeshShapes read into a world through a stated frame, lowered to collision and drawn, under the pass manager
//! with its checks on (Riders Republic build 25394987).
mod inputs;

use inputs::{Game, install_file};
use primase_anvil::components::AnvilMeshShape;
use primase_anvil::passes::{LowerAnvilCollision, SurfaceBy};
use primase_anvil::read::shapes::read_mesh_shapes;
use primase_core::frame::{Axis, SourceFrame};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Collision, Instance, Mesh, Name, Surfaces, Transform, World};
use primase_meshops::DrawCollision;

#[test]
fn real_shapes_read_lower_and_draw() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    // the diagnostic frame: the stored numbers unconverted (not a claim that the units are metres)
    let mut w = World::new();
    let placed = read_mesh_shapes(&path, &[53_612, 21_435], SourceFrame::WORLD, &mut w).unwrap();
    assert_eq!(placed.len(), 2);
    let board = w.get::<Instance>(placed[0]).unwrap().prototype;
    assert_eq!(w.get::<Transform>(placed[0]), Some(&Transform::IDENTITY));
    assert_eq!(
        w.get::<Name>(board).unwrap().0,
        "MAD_PRP_SKATAREABOWL_POOLS_Board_A"
    );
    let shape = w.get::<AnvilMeshShape>(board).unwrap().clone();
    assert_eq!(
        (shape.entry, shape.positions.len(), shape.faces.len()),
        (53_612, 25, 46)
    );
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilCollision {
                by: SurfaceBy::One,
                surface: "rr-diagnostic-unknown".into(),
            }),
            Box::new(DrawCollision),
        ],
    };
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).unwrap();
    let vans = w.get::<Instance>(placed[1]).unwrap().prototype;
    for (e, triangles) in [(board, 46usize), (vans, 534)] {
        assert_eq!(w.get::<Collision>(e).unwrap().triangles.len(), triangles);
        let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
        assert_eq!(p.indices.as_ref().unwrap().len(), triangles * 3);
        assert!(!w.has::<AnvilMeshShape>(e));
    }
    assert_eq!(
        w.resource::<Surfaces>().unwrap().0[0].name,
        "rr-diagnostic-unknown"
    );
    // a mirroring frame reverses every face, so the fronts stay
    let mut m = World::new();
    let mirror = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
    let p = read_mesh_shapes(&path, &[53_612], mirror, &mut m).unwrap()[0];
    let mirrored = m
        .get::<AnvilMeshShape>(m.get::<Instance>(p).unwrap().prototype)
        .unwrap();
    for (a, b) in shape.faces.iter().zip(&mirrored.faces) {
        assert_eq!(*b, [a[0], a[2], a[1]]);
    }
    assert_eq!(mirrored.positions[0], {
        let q = shape.positions[0];
        [q[0], q[1], -q[2]]
    });
}
