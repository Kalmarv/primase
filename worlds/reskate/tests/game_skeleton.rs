//! skate.'s render skeleton read from the game's data (`PRIMASE_SKATE_DATA`, `PRIMASE_OODLE9`):
//! - against ReSkate's EBX reader (`PRIMASE_SKATE_SKELETON_ORACLE`, `renderskeleton.json`): every bone's name,
//!   parent and name hash, its rest (`LocalPose`), model pose and inverse bind as stored, exactly (the oracle prints
//!   f32 with 9 significant digits). That reader is the source of our EBX port, so this checks the port and which
//!   field is which;
//! - the interpretation (a `LinearTransform`'s rows as an affine map's columns, a joint's rest in its parent's space):
//!   independent of any reader, the asset's own poses agree with it: every model pose is its parent's composed with
//!   its local pose, and every inverse model pose composed with the model pose is the identity, within f32 rounding.

use primase_compress::oodle::{Oodle, Version};
use primase_core::{Rest, Skeleton, World};
use primase_frostbite::folder::{Folder, Source};
use primase_reskate::game::skeleton::{SkateSkeletonAsset, check_poses, read_skeleton};
use primase_testdata::real_data;

type R = Result<(), Box<dyn std::error::Error>>;

const TOC: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";
const ASSET: &str = "characters/maincharacters/common/animbase/animbase_default_renderskeleton";

#[test]
fn the_render_skeleton_reads_as_the_game_stores_it() -> R {
    let (Some(data), Some(dll)) = (real_data("PRIMASE_SKATE_DATA"), real_data("PRIMASE_OODLE9")) else {
        return Ok(());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let (bundle, doc) = src.ebx(TOC, ASSET)?;
    assert_eq!(bundle, "win32/levels/game/bam_levelroot/bam_levelroot");
    let mut w = World::new();
    let e = read_skeleton(&doc, ASSET, &mut w)?;
    assert_eq!(w.verify_components(), vec![]);
    let skeleton = w.get::<Skeleton>(e).unwrap();
    let asset = w.get::<SkateSkeletonAsset>(e).unwrap();
    assert_eq!(skeleton.joints.len(), 386);

    // the asset's poses agree with each other under our reading
    let (chain, inverse) = check_poses(skeleton, asset);
    println!(
        "model pose = parent model pose * local pose within {chain:e}; inverse * model = identity within {inverse:e}"
    );
    assert!(
        chain <= 1e-5,
        "model poses are not the parents' composed with the locals: {chain}"
    );
    assert!(
        inverse <= 1e-5,
        "inverse model poses are not the model poses' inverses: {inverse}"
    );

    // ReSkate's reading
    let Some(oracle) = real_data("PRIMASE_SKATE_SKELETON_ORACLE") else {
        return Ok(());
    };
    let oracle: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(oracle)?)?;
    let bones = oracle["bones"].as_array().ok_or("bones")?;
    assert_eq!(bones.len(), skeleton.joints.len());
    let f = |v: &serde_json::Value| v.as_f64().map(|x| x as f32).unwrap_or(f32::NAN);
    let rows = |o: &serde_json::Value| -> [[f32; 3]; 4] {
        let r = |k: &str| -> [f32; 3] { std::array::from_fn(|i| f(&o[k][i])) };
        [r("right"), r("up"), r("forward"), r("translation")]
    };
    for (i, (b, j)) in bones.iter().zip(&skeleton.joints).enumerate() {
        assert_eq!(b["name"].as_str(), Some(j.name.as_str()), "bone {i}");
        assert_eq!(
            b["parent"].as_i64(),
            Some(j.parent.map_or(-1, |p| i64::from(p.0))),
            "bone {i}"
        );
        assert_eq!(
            b["name_hash"].as_u64(),
            Some(u64::from(asset.bone_name_hashes[i])),
            "bone {i}"
        );
        let Rest::Affine(rest) = j.rest else {
            panic!("bone {i}: rest not as stored");
        };
        assert_eq!(rest.cols, rows(&b["rest"]), "bone {i} rest");
        assert_eq!(asset.model_pose[i].cols, rows(&b["world"]), "bone {i} model pose");
        let ib: [[f32; 3]; 4] = std::array::from_fn(|r| std::array::from_fn(|c| f(&b["inverse_bind"][r][c])));
        assert_eq!(asset.inverse_model_pose[i].cols, ib, "bone {i} inverse bind");
    }
    Ok(())
}
