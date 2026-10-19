//! A world whose geometry its cells load: placements whose pivot does not locate their geometry found from prototype
//! boxes ([`spanning`]), the plan made before any mesh is loaded ([`plan_with_geometry`]), and each cell's world
//! loaded once and written by every tile set, as the in-memory writer writes the loaded world.

#![allow(clippy::unwrap_used)]

use primase_core::{Entity, Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_web::{LocalBox, Options, Result, TilePlan, plan_with_geometry, spanning, write};
use std::collections::{BTreeMap, BTreeSet};

/// A prototype whose triangle spans x 0..`width` m, placed at `x`, and its world.
fn world(placements: &[(f32, f32)]) -> (World, Vec<Entity>) {
    let mut w = World::new();
    let mut placed = Vec::new();
    for &(x, width) in placements {
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [width, 0., 0.], [0., 0., -1.]]),
                )]),
                indices: Some(vec![0, 1, 2]),
                material: None,
            }]),
        );
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 0., 0.];
        w.insert(e, t);
        placed.push(e);
    }
    (w, placed)
}

fn mesh_box(w: &World, p: Entity) -> Result<Option<LocalBox>> {
    Ok(w.get::<Mesh>(p).map(|m| {
        let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for v in m.primitives()[0].positions().unwrap() {
            for k in 0..3 {
                b[0][k] = b[0][k].min(f64::from(v[k]));
                b[1][k] = b[1][k].max(f64::from(v[k]));
            }
        }
        b
    }))
}

#[test]
fn spanning_marks_wide_and_misplaced_placements() {
    // a 2 m prop, a 300 m wide one, and a 2 m one whose geometry lies 50 m from its pivot
    let (mut w, placed) = world(&[(10., 2.), (10., 300.), (500., 2.)]);
    let far = w.get::<Instance>(placed[2]).unwrap().prototype;
    if let Some(Stream::F32x3(v)) = w
        .get_mut::<Mesh>(far)
        .unwrap()
        .primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .get_mut(&Semantic::Position)
    {
        v.iter_mut().for_each(|p| p[0] += 50.);
    }
    let mut asked = 0;
    let spans = spanning(&w, 200., |p| {
        asked += 1;
        mesh_box(&w, p)
    })
    .unwrap();
    assert_eq!(spans.marked, [placed[1], placed[2]]);
    assert_eq!(
        (spans.measured, spans.wider_than_a_cell, spans.pivot_outside),
        (3, 1, 1)
    );
    assert_eq!(asked, 3, "one box per prototype");
}

#[test]
fn a_world_that_loads_its_meshes_per_cell_writes_what_the_loaded_world_writes() {
    // the read world has placements and prototypes but no meshes: each cell's world gets them attached, as a lazy
    // reader loads them; one prototype has a box but its cell loads no mesh (a missing asset)
    let (full, placed) = world(&[(10., 2.), (450., 2.), (10., 300.)]);
    let missing = full.get::<Instance>(placed[1]).unwrap().prototype;
    let mut light = full.clone();
    for p in full.iter::<Mesh>().map(|(e, _)| e).collect::<Vec<_>>() {
        light.remove::<Mesh>(p);
    }
    let load = |roots: &BTreeSet<Entity>| {
        let mut w = light.subset(&light.closure(roots.iter().copied()));
        for e in w.entities().collect::<Vec<_>>() {
            if let Some(m) = full.get::<Mesh>(e)
                && e != missing
            {
                w.insert(e, m.clone());
            }
        }
        w
    };
    let spans = spanning(&light, 200., |p| mesh_box(&full, p)).unwrap();
    assert_eq!(spans.marked, [placed[2]]);
    let options = Options {
        compress: false,
        split_by_tile: spans.marked.iter().copied().collect(),
        ..Options::default()
    };
    let plain = Options {
        gpu_instancing: false,
        ..options.clone()
    };
    let geometry = |p: Entity| mesh_box(&full, p).unwrap().is_some();
    let mut plans: Vec<TilePlan> = [&options, &plain]
        .map(|o| plan_with_geometry(&light, o, &geometry).unwrap())
        .into();
    // nothing is written before every placement filed by triangle centre is split
    let first = plans[0].cells()[0];
    let cell_world = load(&plans[0].roots(first));
    let err = plans[0].write_cell(first, &cell_world).unwrap_err();
    assert!(err.to_string().contains("split every placement"), "{err}");
    // the wide placement is split from its geometry alone, by every plan
    for e in plans[0].unsplit() {
        let w = load(&BTreeSet::from([e, light.get::<Instance>(e).unwrap().prototype]));
        for plan in &mut plans {
            plan.split(&w, e).unwrap();
        }
    }
    // each cell's world loaded once and written by both plans
    let mut files = [BTreeMap::new(), BTreeMap::new()];
    for cell in plans[0].cells() {
        assert_eq!(plans[1].roots(cell), plans[0].roots(cell));
        let w = load(&plans[0].roots(cell));
        for (plan, files) in plans.iter_mut().zip(&mut files) {
            for tile in plan.write_cell(cell, &w).unwrap() {
                files.insert(tile.info.file.clone(), tile.bytes);
            }
        }
    }
    // the eager writer on the full world without the missing prototype's mesh
    let mut eager = full.clone();
    eager.remove::<Mesh>(missing);
    for ((plan, files), o) in plans.into_iter().zip(files).zip([&options, &plain]) {
        let index = plan.finish().unwrap();
        let whole = write(&eager, o).unwrap();
        assert_eq!(index.tiles, whole.index.tiles);
        assert_eq!(index.placements_without_mesh, 1);
        assert_eq!(whole.index.placements_without_mesh, 1);
        assert_eq!(files, whole.files);
    }
}

#[test]
fn nothing_to_draw_still_publishes_an_index() {
    let (mut w, _) = world(&[(10., 2.), (450., 2.)]);
    for p in w.iter::<Mesh>().map(|(e, _)| e).collect::<Vec<_>>() {
        w.remove::<Mesh>(p);
    }
    let plan = plan_with_geometry(&w, &Options::default(), &|_| false).unwrap();
    assert!(plan.cells().is_empty());
    let index = plan.finish().unwrap();
    assert_eq!(index.placements_without_mesh, 2);
    let out = std::env::temp_dir().join(format!("primase-web-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    index.write_to(&out).unwrap();
    assert!(out.join("tiles.json").is_file());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn the_target_wants_a_cells_files_loaded_and_lods_chosen() {
    use primase_core::pass::{Legality, Mode};
    use primase_core::{Component, Lods, MeshFile, Opaque, OpaqueData, Texture, TextureFile};
    let (mut w, placed) = world(&[(10., 2.)]);
    let proto = w.get::<Instance>(placed[0]).unwrap().prototype;
    w.insert(
        proto,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![None],
        },
    );
    let image = w.spawn();
    w.insert(image, TextureFile { path: "a.png".into() });
    w.insert_opaque(
        placed[0],
        "test.dialect",
        Opaque {
            version: 1,
            data: OpaqueData::Json(serde_json::json!(1)),
        },
    );
    let t = primase_web::target();
    // the mesh is loaded beside its file; the texture is not
    assert!(t.illegal_entities(&w, MeshFile::NAME, Mode::Full).is_empty());
    assert_eq!(t.illegal_entities(&w, TextureFile::NAME, Mode::Full), [image]);
    w.insert(
        image,
        Texture {
            width: 1,
            height: 1,
            format: primase_core::TexFormat::R8Unorm,
            levels: vec![vec![0]],
            color_space: primase_core::ColorSpace::Linear,
            role: primase_core::TexRole::Mask,
            normal: None,
        },
    );
    assert!(t.illegal_entities(&w, TextureFile::NAME, Mode::Full).is_empty());
    w.remove::<Mesh>(proto);
    assert_eq!(t.illegal_entities(&w, MeshFile::NAME, Mode::Full), [proto]);
    // a dialect's component is carried; levels of detail are chosen first
    assert!(t.illegal_entities(&w, "test.dialect", Mode::Full).is_empty());
    assert!(matches!(t.legality(Lods::NAME), Some(Legality::Illegal)));
}

#[test]
fn collision_a_cell_loads_on_its_prototypes_is_written_with_the_cell() {
    use primase_core::{Collision, Surfaces};
    // two placements of one prototype, in cells 0 and 2, and a third of another; the read world has no collision,
    // each cell's world brings the first prototype's (a reader that loads it per part)
    let (mut light, placed) = world(&[(10., 2.), (450., 2.), (20., 2.)]);
    let proto = light.get::<Instance>(placed[0]).unwrap().prototype;
    light.insert(
        placed[1],
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let light = light;
    let load = |roots: &BTreeSet<Entity>| {
        let mut w = light.subset(&light.closure(roots.iter().copied()));
        if w.is_alive(proto) {
            let mut s = Surfaces::default();
            let concrete = s.index("concrete").unwrap();
            w.insert_resource(s);
            let mut c = Collision::default();
            c.extend(
                &[[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]],
                &[[0, 1, 2]],
                concrete,
                |p| p,
            );
            w.insert(proto, c);
        }
        w
    };
    let options = Options {
        compress: false,
        ..Options::default()
    };
    let mut plan = plan_with_geometry(&light, &options, &|_| true).unwrap();
    let mut sidecars = Vec::new();
    for cell in plan.cells() {
        let w = load(&plan.roots(cell));
        for tile in plan.write_cell(cell, &w).unwrap() {
            if let Some(c) = tile.info.collision {
                sidecars.push((tile.info.col, c.records));
            }
        }
    }
    plan.finish().unwrap();
    // one record per placement of the colliding prototype, in its own cell
    assert_eq!(sidecars, [(0, 1), (2, 1)]);
}
