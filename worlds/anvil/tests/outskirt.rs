//! MAD_Outskirt (Riders Republic build 25394987; docs/material-spec.md §27.14.3): its placed Material lowered with the
//! world map's rows in core's frame (the program's ENU: worked by hand, `u = (x - 16384) / 32768`, `v = (-z - 16384) /
//! 32768` of a core point, its `divide by 32k` 1/32768 and `offset by 16k` -16384), its normal map at 16 times them;
//! then `lower-anvil-world-uv` on a triangle placed twice: a copy for the second placement, each with its placement's
//! coordinates.
use primase_anvil::components::{AnvilMaterial, AnvilWorldUv};
use primase_anvil::passes::{AlphaChoice, Decals1Cutout, LowerAnvilMaterial, LowerAnvilWorldUv, ShaderUv};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{Instance, Mesh, NormalGreen, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_testdata::{Game, install};
use std::collections::BTreeMap;

#[test]
fn outskirt_maps_each_placement_by_its_world_position() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut id = None;
    for f in [0, 1] {
        let forge = archive.forge(f).expect("forge");
        let entries: Vec<(usize, u64)> = (0..forge.names.len())
            .filter(|&k| forge.names[k].type_id == MATERIAL)
            .map(|k| (k, forge.index[k].id))
            .collect();
        for (entry, i) in entries {
            let body = archive.resource(At { forge: f, entry }).expect("material").body;
            let Ok(m) = StoredMaterial::parse(&body) else {
                continue;
            };
            let Some(t) = m.references[0].and_then(|t| archive.find(t.id)) else {
                continue;
            };
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Outskirt" {
                id = Some(i);
            }
        }
    }
    let mut w = World::new();
    let mut placed = Placed::default();
    let material = Placer::new(&mut archive, &mut w, SourceFrame::ENU, InstancedLevels::Slot)
        .material(id.expect("the Outskirt Material"), &mut placed)
        .expect("read");
    assert_eq!(
        w.get::<AnvilMaterial>(material).expect("material").frame,
        SourceFrame::ENU
    );
    // a triangle drawn with it, placed twice
    let prototype = w.spawn();
    w.insert(
        prototype,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [10.0, 0.0, 0.0], [0.0, 5.0, -20.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    let at = |w: &mut World, t: [f32; 3]| {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(
            p,
            Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t],
            },
        );
        p
    };
    let (a, b) = (
        at(&mut w, [100.0, 0.0, -200.0]),
        at(&mut w, [-3000.0, 50.0, 7000.0]),
    );
    let mut log = PassLog::default();
    LowerAnvilMaterial {
        normal_green: NormalGreen::Up,
        alpha: AlphaChoice::Opaque,
        shader_uv: ShaderUv::TexCoord,
        emissive_exposure: 1.0,
        underground_view: 0.5,
        fake_river_s: 0.0,
        decals1_cutout: Decals1Cutout::Surface,
    }
    .run(&mut w, &mut log)
    .expect("lower");
    let s = 1.0 / 32768.0;
    assert_eq!(
        w.get::<AnvilWorldUv>(material).expect("its world map").rows,
        [[s, 0.0, 0.0, -0.5], [0.0, 0.0, -s, -0.5]]
    );
    let pbr = w.get::<PbrMaterial>(material).expect("pbr");
    assert_eq!(
        pbr.normal_texture.as_ref().and_then(|n| n.transform),
        Some(UvTransform {
            offset: [0.0, 0.0],
            scale: [16.0, 16.0],
            rotation: 0.0,
        })
    );
    LowerAnvilWorldUv.run(&mut w, &mut log).expect("world uv");
    println!("{log:?}");
    let pa = w.get::<Instance>(a).expect("a").prototype;
    let pb = w.get::<Instance>(b).expect("b").prototype;
    assert_eq!(pa, prototype);
    assert_ne!(pb, prototype, "the second placement's own copy");
    assert!(!w.has::<AnvilWorldUv>(material));
    for (proto, t) in [(pa, [100.0, 0.0, -200.0]), (pb, [-3000.0, 50.0, 7000.0])] {
        let Some(Stream::F32x2(uv)) = w.get::<Mesh>(proto).expect("mesh").primitives()[0]
            .streams
            .get(&Semantic::TexCoord(0))
            .cloned()
        else {
            panic!("coordinates")
        };
        for (q, got) in [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 5.0, -20.0]]
            .iter()
            .zip(&uv)
        {
            let (x, z) = (q[0] + t[0], q[2] + t[2]);
            let want = [(x - 16384.0) / 32768.0, (-z - 16384.0) / 32768.0];
            assert!(
                (f64::from(got[0]) - want[0]).abs() < 1e-7 && (f64::from(got[1]) - want[1]).abs() < 1e-7,
                "{got:?} against {want:?}"
            );
        }
    }
    assert_eq!(
        log.counts["world-mapped prototypes copied for a placement of their own"],
        1
    );
}
