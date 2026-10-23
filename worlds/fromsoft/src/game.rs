//! The FromSoftware games this crate reads, and what differs between them that their files do not say about
//! themselves. Formats are read by what their bytes state (a map layout's section chain, a FLVER's version); a game's
//! own rules (its archive set, where it keeps its files) are data in that game's module (`sekiro`, `elden`), read
//! through [`Game`] by the shared readers.
use crate::bhd::{PathHash, RecordLayout};
pub use crate::sekiro::source::Resolution;

/// A FromSoftware game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Game {
    Sekiro,
    EldenRing,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Sekiro, Game::EldenRing];

    pub fn name(self) -> &'static str {
        match self {
            Game::Sekiro => "Sekiro",
            Game::EldenRing => "Elden Ring",
        }
    }

    /// The game's archive set.
    pub fn archives(self) -> &'static ArchiveSet {
        match self {
            Game::Sekiro => &crate::sekiro::ARCHIVES,
            Game::EldenRing => &crate::elden::ARCHIVES,
        }
    }

    /// The archive folder holding a map's own files (map pieces, collision, light banks): Sekiro keeps them in
    /// `/map/<map>`, Elden Ring in `/map/<mAA>/<map>`. A map name that is not ASCII is refused by name.
    pub fn map_folder(self, map: &str) -> crate::Result<String> {
        if !map.is_ascii() {
            return Err(crate::invalid("map name", 0, format!("{map:?} is not ASCII")));
        }
        Ok(match self {
            Game::Sekiro => format!("/map/{map}"),
            Game::EldenRing => format!("/map/{}/{map}", &map[..map.len().min(3)]),
        })
    }

    /// The source-identity prefix of a placed object model (`obj/<model>` in Sekiro, `asset/<model>` in Elden Ring).
    pub fn object_prefix(self) -> &'static str {
        match self {
            Game::Sekiro => "obj",
            Game::EldenRing => "asset",
        }
    }

    /// A placed object's model: the binder holding it and its FLVER member, and where its collision files are.
    pub fn object_model(self, model: &str) -> crate::Result<ObjectModel> {
        match self {
            Game::Sekiro => Ok(ObjectModel {
                binder: format!("/obj/{model}.objbnd.dcx"),
                flver: format!("{model}.flver"),
                collision: vec![CollisionSource {
                    binder: None,
                    member: format!("{model}.hkx"),
                    resolution: Resolution::High,
                }],
            }),
            Game::EldenRing => crate::elden::asset_model(model),
        }
    }

    /// The larger maps that also place `map`'s large assets (under names `<map>-<part>`, in their own frame), and
    /// where `map`'s centre lies in each one's frame (source x, z): Elden Ring's open-world tiles; none elsewhere.
    pub fn parent_maps(self, map: &str) -> Vec<(String, [f64; 2])> {
        match self {
            Game::Sekiro => Vec::new(),
            Game::EldenRing => crate::elden::parent_tiles(map),
        }
    }

    /// The binders holding the game's material binaries (Elden Ring's); none for a game whose materials are MTDs.
    pub fn material_binders(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &[],
            Game::EldenRing => crate::elden::MATERIAL_BINDERS,
        }
    }

    /// The binders holding the game's shader bundles (`<shader>.shaderbdle` members), in the order a shader is looked up:
    /// Sekiro's one; Elden Ring's base game's, its two later additions' and SpeedTree's (a shader is in one of them).
    pub fn shader_binders(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &["/shader/shaderbdle.shaderbdlebnd.dcx"],
            Game::EldenRing => crate::elden::SHADER_BINDERS,
        }
    }

    /// The container format of the game's compiled shader programs: Shader Model 5 DXBC (Sekiro), Shader Model 6 DXIL
    /// (Elden Ring).
    pub fn shader_format(self) -> primase_core::shader::ShaderFormat {
        match self {
            Game::Sekiro => primase_core::shader::ShaderFormat::Dxbc,
            Game::EldenRing => primase_core::shader::ShaderFormat::Dxil,
        }
    }

    /// Texture packages every map of the game binds, read whole (Elden Ring's system and decal textures; Sekiro's are
    /// part of its area texture discovery).
    pub fn common_textures(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &[],
            Game::EldenRing => crate::elden::COMMON_TEXTURES,
        }
    }

    /// The package holding the texture `stem`, where the game names packages after their textures (Elden Ring's
    /// assets); `None` otherwise.
    pub fn texture_package(self, stem: &str) -> Option<String> {
        match self {
            Game::Sekiro => None,
            Game::EldenRing => crate::elden::texture_package(stem),
        }
    }

    /// Whether a map model belongs to the game's distant-backdrop series (Sekiro's `m9xxxxx`).
    pub fn far_model(self, model: &str) -> bool {
        match self {
            Game::Sekiro => model.starts_with("m9"),
            Game::EldenRing => false,
        }
    }
}

/// Where a placed object's model is stored ([`Game::object_model`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectModel {
    pub binder: String,
    pub flver: String,
    /// Its collision files, each where the game keeps it.
    pub collision: Vec<CollisionSource>,
}

/// One collision file of an object: the binder holding it (`None`: the model's own binder), its member's name and
/// its resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionSource {
    pub binder: Option<String>,
    pub member: String,
    pub resolution: Resolution,
}

/// One BHD5/BDT pair of an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchivePair {
    /// The number entries of this pair report as their shard.
    pub shard: u8,
    /// The pair's path below the install folder without its `.bhd`/`.bdt` extension (`Data1`, `sd/sd`).
    pub stem: &'static str,
    /// Whether every install holds it; an optional pair (downloadable content) that is absent is reported, not an
    /// error.
    pub required: bool,
}

impl ArchivePair {
    /// A pair every install holds.
    pub const fn required(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: true,
        }
    }
    /// A pair an install may lack.
    pub const fn optional(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: false,
        }
    }
    /// The last component of the stem (`sd` for `sd/sd`): the name its key file takes.
    pub fn name(&self) -> &'static str {
        self.stem.rsplit('/').next().unwrap_or(self.stem)
    }
}

/// Where a game's archive keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Published by the community unpackers and built in, one (modulus, exponent) per pair in the set's order.
    Published(&'static [(&'static [u8], &'static [u8])]),
    /// Not distributed with primase: the caller supplies them.
    Supplied,
}

/// A game's archives: its BHD5/BDT pairs in index order, how its paths hash, how its file records are laid out and
/// where its keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveSet {
    pub pairs: &'static [ArchivePair],
    pub hash: PathHash,
    pub record: RecordLayout,
    pub keys: KeySource,
}
