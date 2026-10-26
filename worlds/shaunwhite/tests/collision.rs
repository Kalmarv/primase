//! Collision layouts cross-checked with AnvilToolkit 1.3.7's AC2 schema and hand-worked fixtures. Three real cells
//! exercise meshes, inline lists and convex hulls; a fixed InertComponent range was hand-checked separately.
use std::fs::File;

use primase_anvil::read::{data::read_entry, forge::Forge, objects::read_objects};
use primase_shaunwhite::read::collision::{
    BARREL_SHAPE, CAPSULE_SHAPE, Child, INERT_COMPONENT, InertComponent, LIST_SHAPE, MESH_SHAPE, MeshShape,
    MultiInertComponent, Shape,
};
use primase_testdata::{Game, Query, expect_found, install_file};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn words(b: &mut Vec<u8>, values: &[u32]) {
    for n in values {
        b.extend(n.to_le_bytes());
    }
}
fn floats(b: &mut Vec<u8>, values: &[f32]) {
    for n in values {
        b.extend(n.to_le_bytes());
    }
}
fn header(class: u32) -> Vec<u8> {
    let mut b = vec![1];
    words(&mut b, &[42, class]);
    b
}
/// A unit square with two triangles and two distinct surface ids. All fields are laid out here by hand.
fn mesh() -> Vec<u8> {
    let mut b = header(MESH_SHAPE);
    words(&mut b, &[4]);
    for p in [
        [0., 0., 0., 0.],
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [1., 1., 0., 0.],
    ] {
        floats(&mut b, &p);
    }
    words(&mut b, &[0]); // no MOPP bytes needed to read explicit triangles
    floats(&mut b, &[0., 0., 0., 1.]);
    words(&mut b, &[6]);
    for i in [0u16, 1, 2, 2, 1, 3] {
        b.extend(i.to_le_bytes());
    }
    words(&mut b, &[2]);
    b.extend([0, 1]);
    words(&mut b, &[2, 70, 90, 0, 0x43EF_99C2, 1, 2, 5]);
    floats(&mut b, &[0., 0., 0., 1., 1., 1., 0., 1.]);
    b
}

#[test]
fn hand_worked_triangles_keep_winding_and_surface_ids() -> Result {
    let b = mesh();
    let m = MeshShape::parse(&b)?;
    assert_eq!(m.id, 42);
    assert_eq!(m.vertices.len(), 4);
    assert_eq!(
        m.triangles.iter().map(|t| t.indices).collect::<Vec<_>>(),
        [[0, 1, 2], [2, 1, 3]]
    );
    assert_eq!(
        m.triangles.iter().map(|t| t.surface_id).collect::<Vec<_>>(),
        [70, 90]
    );
    assert_eq!((m.filter.layer, m.filter.part, m.mopp_version), (1, 2, 5));
    for n in 0..b.len() {
        assert!(MeshShape::parse(&b[..n]).is_err(), "prefix {n}");
    }
    let mut extra = b.clone();
    extra.push(0);
    assert!(
        MeshShape::parse(&extra)
            .unwrap_err()
            .to_string()
            .contains("trailing bytes")
    );
    let mut bad = b.clone();
    bad[25] = 1; // first vertex w
    assert!(MeshShape::parse(&bad).is_err());
    let mut bad = b.clone();
    bad[101] = 9; // first triangle vertex index
    assert!(
        MeshShape::parse(&bad)
            .unwrap_err()
            .to_string()
            .contains("triangle index")
    );
    let mut bad = b;
    bad[117] = 2; // first material-table index (table has only two entries)
    assert!(
        MeshShape::parse(&bad)
            .unwrap_err()
            .to_string()
            .contains("material index")
    );
    Ok(())
}

#[test]
fn capsule_and_list_keep_authored_shapes() -> Result {
    let mut capsule = header(CAPSULE_SHAPE);
    floats(&mut capsule, &[0., 0., -1., 1., 0., 0., 1., 1., 0.5]);
    words(&mut capsule, &[77]);
    assert!(matches!(
        Shape::parse(&capsule)?,
        Shape::Capsule {
            radius: 0.5,
            surface_id: 77,
            ..
        }
    ));
    let mut list = header(LIST_SHAPE);
    words(&mut list, &[3]);
    list.push(0);
    list.extend(capsule);
    list.push(2);
    words(&mut list, &[123]);
    list.push(3);
    list.push(1); // this game's list flag follows the child array
    let Shape::List { children, .. } = Shape::parse(&list)? else {
        panic!("list");
    };
    assert_eq!(children.len(), 3);
    assert!(matches!(&children[0], Child::Inline(shape) if matches!(**shape, Shape::Capsule { .. })));
    assert_eq!(children[1], Child::Reference { tag: 2, id: 123 });
    assert_eq!(children[2], Child::Null);
    assert!(
        Shape::parse(&header(0))
            .unwrap_err()
            .to_string()
            .contains("shape class")
    );
    Ok(())
}

#[test]
fn three_real_cells_and_an_inert_shape_reference() -> Result {
    let Some(path) = install_file(Game::ShaunWhite, "DataPC_01.forge") else {
        return Ok(());
    };
    let forge = Forge::open(&path)?;
    let mut file = File::open(&path)?;
    let (mut meshes, mut primitives, mut triangles, mut inert) = (0usize, 0usize, 0usize, 0usize);
    for name in [
        "Cell01249_DataBlock",
        "Cell01251_DataBlock",
        "Cell01440_DataBlock",
    ] {
        let k = forge
            .names
            .iter()
            .position(|n| n.name == name)
            .ok_or("missing fixture cell")?;
        let data = read_entry(&forge.payload(&mut file, k)?, 1 << 28)?;
        for object in read_objects(&data.metadata.data, &data.resource.data)? {
            let body = &data.resource.data[object.body];
            if matches!(
                object.class,
                MESH_SHAPE | CAPSULE_SHAPE | LIST_SHAPE | BARREL_SHAPE
            ) {
                let shape = Shape::parse(body)
                    .map_err(|e| format!("{name}: {} {:#x}: {e}", object.name, object.id))?;
                if let Shape::Mesh(m) = shape {
                    assert_eq!(m.id, object.id);
                    meshes += 1;
                    triangles += m.triangles.len();
                } else {
                    primitives += 1;
                }
            }
            // Independently decoded component: Entity 00_Obj_WallLight01_101, its inline InertComponent begins
            // at body+1131, lasts 150 bytes, and refers to the wall-light's collision MeshShape 0x6e9c4d7e.
            if name == "Cell01249_DataBlock" && object.id == 0x5F09_35E2 {
                let component = InertComponent::parse(body.get(1131..1281).ok_or("short entity fixture")?)?;
                assert_eq!(component.id, 0xCBA5_E0CA);
                assert_eq!(component.managed, 0);
                assert_eq!(component.shape_id, 0x6E9C_4D7E);
                assert_eq!(component.walkability, 2);
                assert_eq!(u32::from_le_bytes(body[1136..1140].try_into()?), INERT_COMPONENT);
                inert += 1;
            }
        }
    }
    expect_found(
        Query::Install(Game::ShaunWhite, Some("DataPC_01.forge")),
        "collision meshes",
        meshes,
    );
    assert_eq!((meshes, primitives, inert), (36, 3, 1));
    println!(
        "three cells: {meshes} meshes, {primitives} list/convex shapes, {triangles} triangles, {inert} inert reference"
    );
    Ok(())
}

#[test]
fn a_real_multi_inert_keeps_contained_entity_shape_pairs() -> Result {
    let Some(path) = install_file(Game::ShaunWhite, "DataPC_01.forge") else {
        return Ok(());
    };
    let forge = Forge::open(&path)?;
    let k = forge
        .names
        .iter()
        .position(|n| n.name == "Cell01439_DataBlock")
        .ok_or("missing multi fixture cell")?;
    let mut file = File::open(path)?;
    let data = read_entry(&forge.payload(&mut file, k)?, 1 << 28)?;
    let objects = read_objects(&data.metadata.data, &data.resource.data)?;
    let object = objects
        .iter()
        .find(|o| o.id == 0xCBA6_EBAB)
        .ok_or("missing multi inert entity")?;
    let body = &data.resource.data[object.body.clone()];
    // Hand-checked inline component: body+79, 277 bytes. The first two entities instance the same mesh shape.
    let multi = MultiInertComponent::parse(body.get(79..356).ok_or("short multi fixture")?)?;
    let Shape::MultiMesh {
        entity_ids,
        shape_ids,
        mopp,
        ..
    } = multi.shape
    else {
        panic!("multi mesh");
    };
    assert_eq!(entity_ids.len(), 10);
    assert_eq!(shape_ids.len(), 10);
    assert_eq!(&entity_ids[..2], [0x3BB4_9511, 0x3BB4_9512]);
    assert_eq!(&shape_ids[..2], [0xB310_25C9, 0xB310_25C9]);
    assert_eq!(mopp.len(), 128);
    Ok(())
}
