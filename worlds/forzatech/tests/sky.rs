//! Reads the emissive scale of the `Forte_Summer_01` sky from the Tokyo track's time-of-day file
//! (`media/TimeOfDay/TimeOfDay_Brio.ppr`) of the install the `GAME_DATA` configuration names: its curve is 1 at night
//! and 50 by day, so the peak the route takes as cd/m2 per unit of emissive strength is 50 (the figure the route
//! stated by hand before it read the file). Also checks that a name two skies share, and a name no sky has, are
//! refused. Without the install it skips with a line naming its key.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test reading the files you point it at"
)]

mod inputs;

use inputs::{Game, install_file};
use primase_core::convert::NoticeKind;
use primase_forzatech::read::sky::SkyEmission;

fn file() -> Option<Vec<u8>> {
    let folder = install_file(Game::Fh6, "media/TimeOfDay")?;
    Some(std::fs::read(folder.join("TimeOfDay_Brio.ppr")).unwrap())
}

#[test]
fn forte_summer_01_peaks_at_50() {
    let Some(bytes) = file() else {
        return;
    };
    let sky = SkyEmission::parse(&bytes, "Forte_Summer_01").unwrap();
    println!("{:?}", sky.keys());
    assert_eq!(sky.keys().len(), 6);
    assert_eq!(sky.keys()[0], (0.0, 1.0));
    assert_eq!(sky.keys()[5], (1.0, 1.0));
    assert_eq!(sky.peak().unwrap(), 50.0);
    let notice = sky.notice("test");
    println!("{notice}");
    assert_eq!(notice.kind, NoticeKind::Approximation);
    assert_eq!(notice.count, 1);
    assert_eq!(notice.examples, ["Forte_Summer_01: peak of 6 keys, 1 to 50"]);
}

#[test]
fn a_name_must_pick_one_sky() {
    let Some(bytes) = file() else {
        return;
    };
    // 13 of the 190 names of the file's 203 skies are borne by two
    let twice = SkyEmission::parse(&bytes, "BeautifulArea_BA03_010").unwrap_err();
    assert!(twice.to_string().contains("more than one group"), "{twice}");
    let none = SkyEmission::parse(&bytes, "Forte_Summer_00").unwrap_err();
    assert!(none.to_string().contains("no sky named"), "{none}");
    // a name of the file that is not a sky's: a preset's, which selects a sky by time of day
    let preset = SkyEmission::parse(&bytes, "BP_Summer01_Dawn_01").unwrap_err();
    assert!(preset.to_string().contains("no sky named"), "{preset}");
}
