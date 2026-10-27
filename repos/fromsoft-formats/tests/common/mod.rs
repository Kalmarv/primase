//! What the real-data tests share: the configured archives, the Oodle codec, and the reference dumps' float bits.
#![allow(dead_code)]

/// The version-6 Oodle codec: the configured game library on Windows, oozextract elsewhere.
#[allow(unsafe_code)]
pub fn oodle(
    library: primase_testdata::OodleLibrary,
) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
    // SAFETY: the test configuration names a trusted native library.
    unsafe { primase_foundations::compress::oodle::Oodle::open(library.path(), primase_foundations::compress::oodle::Version::V6) }
}

/// A configured game's archives (`installs.sekiro` with `keys.sekiro`, or `installs.elden` with `keys.elden`), or
/// `None` when the configuration lacks them.
pub fn archive(
    game: fromsoft_formats::Game,
) -> Result<Option<fromsoft_formats::Archive>, Box<dyn std::error::Error>> {
    use fromsoft_formats::{Archive, Game, Keys};
    use primase_testdata::{Game as Data, install, keys};
    let data = match game {
        Game::Sekiro => Data::Sekiro,
        Game::EldenRing => Data::Elden,
    };
    Ok(match (install(data), keys(data)) {
        (Some(dir), Some(keys)) => Some(Archive::open_game(
            dir,
            game,
            &Keys::from_folder(keys, game, None)?,
        )?),
        _ => None,
    })
}

/// A float a reference dump writes as its u32 bit pattern in hex.
pub fn bits(v: &serde_json::Value) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_str_radix(v.as_str().ok_or("float bits")?, 16)?)
}

/// Three floats a reference dump writes as bit patterns.
pub fn bits3(v: &serde_json::Value) -> Result<[u32; 3], Box<dyn std::error::Error>> {
    let a = v.as_array().ok_or("vector")?;
    Ok([bits(&a[0])?, bits(&a[1])?, bits(&a[2])?])
}
