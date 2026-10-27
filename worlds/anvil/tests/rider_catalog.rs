//! The crowd's skin palette, its TextureMap decoded, on Riders Republic's forges.
use anvil_formats::archive::Archive;
use primase_testdata::{Game, install};

/// The four tones of the crowd's skin palette, as the notes decode its 8 x 4 texture (bottom row of each 2 x 2 cell).
#[test]
fn the_skin_palette_has_four_tones() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&forges.map(|f| dir.join(f))).expect("archive");
    let tones = primase_anvil::read::rider::skin_palette(&mut archive).expect("palette");
    assert_eq!(
        tones,
        [[199, 144, 113], [102, 56, 29], [162, 100, 54], [185, 139, 105]]
    );
}
