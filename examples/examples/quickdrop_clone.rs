//! A Quick Drop pack of one item: a stock pad cloned under a new key and title, its mesh built again from the stock
//! mesh's first LOD and its collision the stock one, written as an `.fbmod` for ReSkate Studio's `compile-mod`. The
//! registration and the rebuilt mesh are what a game run of it shows working.
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::{fbmod, meshset, physics, rigid};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_reskate::read::quickdrop::read_donor;
use primase_reskate::write::quickdrop::{Item, pack};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn main() -> R {
    let mut args = std::env::args().skip(1);
    let (Some(game), Some(out)) = (args.next(), args.next()) else {
        return Err("usage: quickdrop_clone <skate. game folder> <out.fbmod>".into());
    };
    let data = Folder::open(format!("{game}/Data"))?;
    // SAFETY: the game's own Oodle library.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(format!("{game}/oo2core_9_win64.dll"), Version::V9) }?;
    let src = Source { folder: &data, base: Some(&data), oodle: Some(&oodle) };

    let donor = read_donor(&src, "items/bkblocks/own_bkpads_generic_padlow_00001")?;
    let stock = meshset::read_resource(&donor.meshset.bytes, &donor.meshset.meta)?;
    let parts = primase_frostbite::geometry::rigid_parts(&stock.lods[0], &donor.chunk.1)?;
    let mesh_name = "world/buildkit/quickdrop/primase/own_primase_clonetest/own_primase_clonetest_mesh";
    let built = rigid::build(mesh_name, donor.chunk.0, &parts)?;
    let item = Item {
        key: "own_primase_clonetest".into(),
        title: "primase clone test".into(),
        mesh_name: mesh_name.into(),
        meshset: built.meshset,
        chunk: built.chunk,
        collision: physics::read(&donor.collision_resource.bytes)?,
        volume_moments: None,
    };
    let m = pack(&donor, &[item], "primase Quick Drop clone test", "primase")?;
    std::fs::write(&out, fbmod::write(&m)?)?;
    println!("{out}: {} entries, {} payloads", m.entries.len(), m.payloads.len());
    Ok(())
}
