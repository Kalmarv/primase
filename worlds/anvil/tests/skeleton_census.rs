//! Every Skeleton of Riders Republic's forges (`installs.riders`) read to its last byte and counted as the notes'
//! census counts them (docs/types.md "Skeleton": 1,138 consumed exactly), with this reader's own counts: the bones,
//! the pose groups, poses and pose bones, the IKData and Reflex3 constraints, and the bone tree's invariants (a parent
//! and the root name bones of the skeleton, each bone's key low half its position, every rotation of unit length).
use primase_anvil::read::archive::Archive;
use primase_anvil::read::listing::Kind;
use primase_anvil::read::skeleton::StoredSkeleton;
use primase_testdata::{Game, install};

#[test]
fn every_skeleton_reads_and_its_tree_holds() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    let paths: Vec<_> = forges.iter().map(|f| dir.join(f)).collect();
    let mut archive = Archive::open(&paths).expect("archive");
    let rows: Vec<_> = archive
        .listing()
        .filter(|(_, l)| l.kind == Some(Kind::Skeleton))
        .map(|(forge, l)| {
            (
                primase_anvil::read::archive::At {
                    forge,
                    entry: l.entry,
                },
                l.name.to_owned(),
            )
        })
        .collect();
    let (mut bones, mut groups, mut poses, mut pose_bones, mut ik, mut reflex) =
        (0usize, 0usize, 0usize, 0usize, 0, 0);
    for (at, name) in &rows {
        let body = archive.resource(*at).expect("resource").body;
        let s = StoredSkeleton::parse(&body).unwrap_or_else(|e| panic!("{name}: {e}"));
        bones += s.bones.len();
        groups += s.pose_groups.len();
        poses += s.pose_groups.iter().map(|g| g.poses.len()).sum::<usize>();
        pose_bones += s
            .pose_groups
            .iter()
            .flat_map(|g| &g.poses)
            .map(|p| p.bones.len())
            .sum::<usize>();
        ik += usize::from(s.ik_data.is_some());
        reflex += usize::from(s.reflex3.is_some());
        let ids: Vec<u64> = s.bones.iter().map(|b| b.id).collect();
        assert!(
            s.root.is_none_or(|r| ids.contains(&r)),
            "{name}: the root names no bone"
        );
        for (i, b) in s.bones.iter().enumerate() {
            assert!(
                b.parent.is_none_or(|p| ids.contains(&p)),
                "{name}: bone {i}'s parent"
            );
            assert!(
                b.mirror.is_none_or(|m| ids.contains(&m)),
                "{name}: bone {i}'s mirror"
            );
            assert_eq!(
                usize::try_from(b.key & 0xFFFF).ok(),
                Some(i),
                "{name}: bone {i}'s key {:#x}",
                b.key
            );
            let len = b.local_rotation.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "{name}: bone {i}'s local rotation has length {len}"
            );
        }
    }
    println!(
        "skeletons {}, bones {bones}, pose groups {groups}, poses {poses}, pose bones {pose_bones}, IKData {ik}, \
         Reflex3 {reflex}",
        rows.len()
    );
    assert_eq!(
        (rows.len(), bones, groups, poses, pose_bones, ik, reflex),
        (1_138, 54_925, 28, 6_003, 70_429, 3, 1_137)
    );
}
