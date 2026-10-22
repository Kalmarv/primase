//! The Placer on the Entity flags it reads (Riders Republic build 25394987; docs/placement.md §19): a hidden and
//! phantom Entity is placed with every placement marked; a FarFake cell's placements are marked with its cell number;
//! a river's RigidBody (a water surface: `WaterLevel` without `Static`) is placed as collision carrying its filter, as
//! the others are; `select-anvil-entities` then drops what the program says (here all three kinds) and nothing else; a
//! prototype carries its selector's switch distances (each level's far limit, rising), its finest level the
//! prototype's own where the selector's first levels hold no reference: the dependencies fill them (the `_LOD0` and
//! `_LOD1` meshes), so level 0 is present.
use primase_anvil::components::{
    AnvilCollisionFilter, AnvilEntityState, AnvilFake, AnvilLodDistances, AnvilLods,
};
use primase_anvil::passes::{CollisionOnly, Keep, SelectAnvilEntities};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::entity::{ComponentBody, ENTITY, StoredEntity, query};
use primase_anvil::read::lod::{LOD_SELECTOR, StoredLodSelector};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Instance, World};
use primase_testdata::real_data;

const ORDER: [&str; 4] = [
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

/// The first Entity of forge `forge` whose name and body `pick` takes.
fn first(archive: &mut Archive, forge: usize, pick: impl Fn(&str, &StoredEntity) -> bool) -> At {
    let names = archive.forge(forge).expect("forge").names.clone();
    for (entry, n) in names.iter().enumerate() {
        if n.type_id != ENTITY {
            continue;
        }
        let body = archive.resource(At { forge, entry }).expect("entity").body;
        if let Ok(e) = StoredEntity::parse(&body)
            && pick(&n.name, &e)
        {
            return At { forge, entry };
        }
    }
    panic!("no such Entity in forge {forge}")
}

#[test]
fn the_placer_reads_the_entity_flags() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    // hidden and phantom (run bytes 00 00 00 01 01): nothing placed
    let names = archive.forge(3).expect("forge").names.clone();
    let ufo = At {
        forge: 3,
        entry: names
            .iter()
            .position(|n| n.name == "MAD_FX_Mesh_UFOBeamSmall")
            .expect("the UFO beam"),
    };
    // a FarFake cell
    let cell = first(&mut archive, 1, |name, _| name.starts_with("MAD_USNAT_FarFake_"));
    let cell_name = archive.forge(1).expect("forge").names[cell.entry].name.clone();
    let n: u32 = cell_name["MAD_USNAT_FarFake_".len()..]
        .split('_')
        .next()
        .and_then(|k| k.parse().ok())
        .expect("its cell number");
    // a river surface's RigidBody
    let river = first(&mut archive, 1, |name, e| {
        name.starts_with("River_Mesh_generation")
            && e.components
                .iter()
                .any(|c| matches!(c.body, ComponentBody::RigidBody { shape: Some(_), .. }))
    });
    // an unscaled Entity whose selector's levels 0 and 1 hold no reference (its two dependencies fill them)
    let mut empty_first = None;
    {
        let names = archive.forge(1).expect("forge").names.clone();
        for (entry, nm) in names.iter().enumerate() {
            if nm.type_id != ENTITY {
                continue;
            }
            let body = archive.resource(At { forge: 1, entry }).expect("entity").body;
            let Ok(e) = StoredEntity::parse(&body) else {
                continue;
            };
            let Some(object) = e.visuals().next().and_then(|v| v.object) else {
                continue;
            };
            let Some(at) = archive.find(object.id) else {
                continue;
            };
            let r = archive.resource(at).expect("object");
            if r.type_id != LOD_SELECTOR || e.scale != 1.0 {
                continue;
            }
            let s = StoredLodSelector::parse(&r.body).expect("a selector");
            if s.levels[0].reference.is_none()
                && s.levels[1].reference.is_none()
                && s.levels[2].reference.is_some()
                && s.dependencies.len() == 2
                && e.visuals().all(|v| match &v.instance {
                    primase_anvil::read::entity::Instance::Lod { slots, .. } => {
                        slots.iter().all(|s| match s {
                            primase_anvil::read::entity::Slot::Mesh(m) => m.instances.is_empty(),
                            _ => true,
                        })
                    }
                    _ => true,
                })
            {
                empty_first = Some((At { forge: 1, entry }, s));
                break;
            }
        }
    }
    let (empty_first, selector) = empty_first.expect("a selector whose levels 0 and 1 hold no reference");

    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    )
    .with_instanced_collision(InstancedCollision::PerInstance);
    placer.place(ufo, &mut placed).expect("the UFO beam");
    assert_eq!(placed.hidden_or_phantom, 1);
    let ufo_placed: Vec<_> = placed
        .placements
        .iter()
        .chain(&placed.collisions)
        .copied()
        .collect();
    assert!(!ufo_placed.is_empty(), "the hidden Entity is placed, marked");
    for &p in &ufo_placed {
        let s = placer.world().get::<AnvilEntityState>(p).expect("marked");
        assert!(s.hidden || s.phantom, "{s:?}");
    }
    let cell_from = placed.placements.len();

    placer.place(cell, &mut placed).expect("the cell");
    assert!(placed.placements.len() > cell_from);
    assert_eq!(placed.fakes, (placed.placements.len() - cell_from) as u64);
    for &p in &placed.placements[cell_from..] {
        assert_eq!(
            placer.world().get::<AnvilFake>(p),
            Some(&AnvilFake {
                mesh_index: n,
                near: false
            })
        );
    }

    let before = placed.collisions.len();
    placer.place(river, &mut placed).expect("the river");
    assert!(placed.water_bodies >= 1);
    let river_collision: Vec<_> = placed.collisions[before..].to_vec();
    assert!(
        river_collision.iter().any(|&c| placer
            .world()
            .get::<AnvilCollisionFilter>(c)
            .is_some_and(|f| f.is_water_surface())),
        "the water surface is placed as collision, its filter saying what it is"
    );
    let before = placed.collisions.len();

    let from = placed.placements.len();
    placer
        .place(empty_first, &mut placed)
        .expect("the selector's owner");
    let p = placed.placements[from];
    let prototype = placer.world().get::<Instance>(p).expect("placed").prototype;
    // its finest level is the prototype's own: level 0, its first dependency
    let lods = placer.world().get::<AnvilLods>(prototype).expect("levels");
    assert_eq!(lods.detail, 0);
    // the switch distances as the selector stores them, rising; the draw distance the last, capped by culling
    let d = placer
        .world()
        .get::<AnvilLodDistances>(prototype)
        .expect("distances");
    let ids = selector.level_ids().expect("levels");
    let want: Vec<(u8, f32)> = (0u8..)
        .zip(&ids)
        .filter(|(_, id)| id.is_some())
        .map(|(l, _)| (l, selector.levels[usize::from(l)].floats[0]))
        .collect();
    assert_eq!(d.levels, want);
    assert!(d.levels.windows(2).all(|w| w[0].1 < w[1].1), "{:?}", d.levels);
    let last = d.levels.last().expect("a level").1;
    assert_eq!(
        d.draw_distance(),
        Some(if d.culling > 0.0 {
            last.min(d.culling)
        } else {
            last
        })
    );
    // its collision placements carry their filter: Static set, but for a water surface's (placed now, marked)
    for &c in &placed.collisions[before..] {
        let f = placer.world().get::<AnvilCollisionFilter>(c).expect("a filter");
        assert!(f.queries(query::STATIC) || f.is_water_surface(), "{f:?}");
    }
    println!(
        "cell {cell_name}, distances {:?} (culling {}), water bodies {}",
        d.levels, d.culling, placed.water_bodies
    );
    // what stays: the cell's and the selector owner's placements and collision, but for a water surface's
    let kept: Vec<_> = placed.placements[cell_from..]
        .iter()
        .chain(&placed.collisions[before..])
        .copied()
        .filter(|&p| {
            !placer
                .world()
                .get::<AnvilCollisionFilter>(p)
                .is_some_and(|f| f.is_water_surface())
        })
        .collect();
    drop(placer);
    // the program drops all three kinds: the UFO's placements and the water surface's collision go, the rest stays
    let mut log = PassLog::default();
    SelectAnvilEntities {
        hidden: Keep::Drop,
        phantom: Keep::Drop,
        water_collision: Keep::Drop,
        collision_only: CollisionOnly::Keep,
    }
    .run(&mut w, &mut log)
    .expect("select");
    assert!(ufo_placed.iter().all(|&p| !w.is_alive(p)));
    assert!(river_collision.iter().all(|&c| {
        !w.is_alive(c)
            || !w
                .get::<AnvilCollisionFilter>(c)
                .is_some_and(|f| f.is_water_surface())
    }));
    assert!(kept.iter().all(|&p| w.is_alive(p)));
    assert!(w.iter::<AnvilEntityState>().next().is_none());
}
