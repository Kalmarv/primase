//! Skeleton assets and the skeleton a skinned mesh's bone lists address: on the skate. install (and its Oodle 9
//! library), the character skeleton `animation/dingo/animbase_default_skeleton` read from the globals bundle, and the
//! hat bucket's meshes (bone lists of 386 bones) linked to it by the resolve pass; and the reader's checks on a
//! skeleton by itself.

#![allow(clippy::unwrap_used)]

mod inputs;

use frostbite_formats::MESHSET;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::scope::Scope;
use inputs::{Game, Tool, install_file, tool};
use primase_core::World;
use primase_core::pass::{Pass, PassLog};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::components::{FrostbiteMeshSet, FrostbiteSkeleton, FrostbiteSkeletonLink};
use primase_frostbite::passes::ResolveFrostbiteShaders;
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::geometry::read_geometry;
use primase_frostbite::read::program::{materials, resolve_materials};
use primase_frostbite::read::skeleton::read_skeletons;

const GLOBALS_BUNDLE: &str = "win32/configurations/gameconfigurations/delmargameconfiguration";
const HAT: &str = "hatbucket/2023/gen_hat_hatbucket_cas_main_bundlereftable";

#[test]
fn the_hat_buckets_meshes_are_linked_to_the_character_skeleton_by_their_bone_lists() {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return;
    };
    let folder = Folder::open(data).unwrap();
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }.unwrap();
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let items = src.listing("Win32/items.toc").unwrap();
    let globals = src.listing("Win32/globals.toc").unwrap();
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let hat = items.bundles.iter().find(|b| b.name.ends_with(HAT)).unwrap();
    let config = globals.bundles.iter().find(|b| b.name == GLOBALS_BUNDLE).unwrap();
    let mut scope_bundles = vec![hat.clone()];
    scope_bundles.extend(globals.bundles.iter().cloned());
    scope_bundles.extend(root.bundles.iter().cloned());
    let scope = Scope::new(&scope_bundles);
    let materials = materials(&src, &scope).unwrap();
    let mut w = World::new();
    // every skeleton asset of the scope: nine of the configuration bundle and the level's skateboard
    let skeletons = read_skeletons(&src, &scope, &mut w).unwrap();
    assert_eq!(skeletons.len(), 10);
    assert_eq!(w.iter::<FrostbiteSkeleton>().count(), 10);
    let in_config = config
        .entries()
        .filter(|e| e.kind == Kind::Ebx && e.name.ends_with("_skeleton"))
        .count();
    assert_eq!(in_config, 9);
    let counts: std::collections::BTreeMap<String, usize> = w
        .iter::<FrostbiteSkeleton>()
        .map(|(_, s)| (s.name.clone(), s.joints.len()))
        .collect();
    assert_eq!(counts["Animation/Dingo/AnimBase_Default_Skeleton"], 395);
    // no other has the 386 joints the hat's bone lists need
    assert_eq!(counts.values().filter(|&&n| n >= 386).count(), 1);
    let (_, skeleton) = w
        .iter::<FrostbiteSkeleton>()
        .find(|(_, s)| s.joints.len() == 395)
        .unwrap();
    let head: Vec<(&str, Option<u32>)> = (101..=107)
        .map(|j| (skeleton.joints[j].name.as_str(), skeleton.joints[j].parent))
        .collect();
    assert_eq!(
        head,
        [
            ("Neck", Some(45)),
            ("Neck1", Some(101)),
            ("Head", Some(102)),
            ("RM_Head", Some(103)),
            ("PS_Head_Sec_02", Some(103)),
            ("PS_Head_Sec_01", Some(103)),
            ("Face", Some(103)),
        ]
    );
    // the hat: a bone list of 386 bones in every section, the identity
    let mut meshes = Vec::new();
    for entry in hat
        .entries()
        .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
    {
        let made = read_entry(&src, hat, &entry, &mut w).unwrap();
        resolve_materials(&src, &scope, &materials, made[0], &mut w).unwrap();
        meshes.extend(read_geometry(&src, hat, made[0], &mut w).unwrap());
    }
    let set = w.iter::<FrostbiteMeshSet>().next().unwrap().1;
    assert_eq!(set.meshset.lods.len(), 6);
    for section in set.meshset.lods.iter().flat_map(|l| &l.sections) {
        assert_eq!(section.bones, (0..386).collect::<Vec<u16>>());
    }
    let mut log = PassLog::default();
    ResolveFrostbiteShaders.run(&mut w, &mut log).unwrap();
    assert_eq!(meshes.len(), 6);
    for mesh in &meshes {
        let link = w.get::<FrostbiteSkeletonLink>(*mesh).unwrap();
        assert_eq!(link.bones, 386);
        assert_eq!(
            w.get::<FrostbiteSkeleton>(link.skeleton).unwrap().joints.len(),
            395
        );
    }
    // joint 101 appears only as padding (zero weight): the weighted range is Neck1..Face
    let notes: Vec<(&String, &u64)> = log.approximations.iter().collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].0.starts_with(
        "meshes skinned by joints 102-107 (Neck1..Face) of Animation/Dingo/AnimBase_Default_Skeleton, "
    ));
    assert!(notes[0].0.contains("(386 joints)"));
    assert_eq!(*notes[0].1, 6);
}
