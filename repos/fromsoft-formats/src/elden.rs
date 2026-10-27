//! Elden Ring's own rules: what differs from the other FromSoftware games and is not stated by the files themselves.
use crate::bhd::{PathHash, RecordLayout};
use crate::game::{ArchivePair, ArchiveSet, CollisionSource, ObjectModel, Resolution};
use crate::{Result, invalid};

/// Data0 to Data3, the downloadable content's DLC and the sound archives `sd/sd` and `sd/sd_dlc02`, as the community
/// unpacker (UXM's `EldenRingGameInfo.xml`) lists them; base-133 u64 path hashes.
pub(crate) const ARCHIVES: ArchiveSet = ArchiveSet {
    pairs: &[
        ArchivePair::required(0, "Data0"),
        ArchivePair::required(1, "Data1"),
        ArchivePair::required(2, "Data2"),
        ArchivePair::required(3, "Data3"),
        ArchivePair::optional(4, "DLC"),
        ArchivePair::required(5, "sd/sd"),
        ArchivePair::optional(6, "sd/sd_dlc02"),
    ],
    hash: PathHash::Base133,
    record: RecordLayout::ELDEN_RING,
};

/// Elden Ring's params: `regulation.bin` beside the executable, AES-256-CBC encrypted (the file's first 16 bytes the
/// initialisation vector), holding a DCX envelope (ZSTD in the game's current release) around the param binder. The key
/// is the user's key material, as the archives' are: `regulation.key` in the keys folder, the
/// 32 bytes as 64 hexadecimal digits (the key SoulsFormats' `RegulationDecryptor` names for the game).
pub(crate) const PARAMS: crate::game::ParamSource = crate::game::ParamSource::Regulation {
    file: "regulation.bin",
    key_file: "regulation.key",
};

/// Elden Ring's NPC param, `NPC_PARAM_ST` data version 9 with rows of 736 bytes: `modelDispMask0..15` in bytes 330 and
/// 331, `16..31` in bytes 334 and 335 (Paramdex's `ER/Defs/NpcParam.xml` as Smithbox ships it at `cbd477a8`,
/// FormatVersion 203, its fields laid out as SoulsFormats' `PARAMDEF` packs them; the fields its `RemovedVersion`
/// marks gone are not in the game's current rows; the `npc_outfits` test holds every row's bits to SoulsFormats'
/// reading of regulation 11730034 with that definition at the regulation's version).
pub(crate) const NPC_PARAM: crate::param::NpcLayout = crate::param::NpcLayout {
    data_version: 9,
    row_size: 736,
    mask_bytes: [330, 331, 334, 335],
};

/// Where Elden Ring's NPC param keeps the fields [`crate::param::NpcStats`] reads: after the four bytes of
/// `disableParam_NT` and its reserved bits and bytes, `behaviorVariationId` at 4, then after `resistCorrectId_poison` (8)
/// `nameId`, `turnVellocity`, `hitHeight`, `hitRadius`, `weight`, `hitYOffset`, `hp`, `mp` and `getSoul` (Paramdex's
/// `ER/Defs/NpcParam.xml`, data version 9).
pub(crate) const NPC_STATS: crate::param::NpcStatLayout = crate::param::NpcStatLayout {
    data_version: 9,
    row_size: 736,
    offsets: [4, 12, 16, 20, 24, 28, 32, 36, 40, 44],
};

/// An asset model `AEGnnn_mmm`: its binder `/asset/aeg/aegnnn/aegnnn_mmm.geombnd.dcx` and FLVER member; its collision is
/// in binders of its own, `aegnnn_mmm_h.geomhkxbnd.dcx` (high resolution, member `aegnnn_mmm_h.hkx`, a subset of the
/// assets) and `aegnnn_mmm_l.geomhkxbnd.dcx` (low, `aegnnn_mmm_l.hkx`, every collidable asset).
pub(crate) fn asset_model(model: &str) -> Result<ObjectModel> {
    let lower = crate::names::fold(model);
    let group = lower
        .get(..6)
        .filter(|g| g.starts_with("aeg") && g[3..].bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| invalid("Elden Ring asset", 0, format!("asset model {model}")))?;
    let collision = [("h", Resolution::High), ("l", Resolution::Low)]
        .map(|(suffix, resolution)| CollisionSource {
            binder: Some(format!("/asset/aeg/{group}/{lower}_{suffix}.geomhkxbnd.dcx")),
            member: format!("{lower}_{suffix}.hkx"),
            resolution,
        })
        .into();
    Ok(ObjectModel {
        binder: format!("/asset/aeg/{group}/{lower}.geombnd.dcx"),
        flver: format!("{lower}.flver"),
        collision,
    })
}

/// Areas laid out as a grid of tiles: the small tiles' edge in metres and the levels of larger tiles above them. A tile
/// `mAA_XX_ZZ_00` sits in the level-`L` tile `mAA_{XX>>L}_{ZZ>>L}_0L`, whose map layout also places the small tile's
/// larger assets (named `<small tile>-<part>`, in the larger tile's frame) so they stay drawn from farther away.
const TILED_AREAS: &[(&str, f64, &[u32])] = &[("m60", 256., &[1, 2]), ("m61", 256., &[1, 2])];

/// The larger tiles above `tile` and where `tile`'s centre lies in each one's frame (source x, z): offset
/// `(i - (p << L)) * 256 + 128 - (256 << L) / 2` metres for grid index `i`, parent index `p` and level `L`. None for a
/// map outside the tile grids or a tile above level 0.
pub(crate) fn parent_tiles(tile: &str) -> Vec<(String, [f64; 2])> {
    let Some(&(_, edge, levels)) = TILED_AREAS.iter().find(|(area, _, _)| tile.starts_with(area)) else {
        return Vec::new();
    };
    if tile.len() != 12 || !tile.is_ascii() || !tile.ends_with("_00") {
        return Vec::new();
    }
    let (Ok(x), Ok(z)) = (tile[4..6].parse::<u32>(), tile[7..9].parse::<u32>()) else {
        return Vec::new();
    };
    levels
        .iter()
        .map(|&level| {
            let (px, pz) = (x >> level, z >> level);
            let half = edge * f64::from(1u32 << level) / 2.;
            let offset = |i: u32, p: u32| f64::from(i - (p << level)) * edge + edge / 2. - half;
            (
                format!("{}_{px:02}_{pz:02}_{level:02}", &tile[..3]),
                [offset(x, px), offset(z, pz)],
            )
        })
        .collect()
}

/// The binders holding Elden Ring's material binaries, in the order a material name is looked up: the base game's,
/// its two later additions', SpeedTree's.
pub(crate) const MATERIAL_BINDERS: &[&str] = &[
    "/material/allmaterial.matbinbnd.dcx",
    "/material/allmaterial_dlc01.matbinbnd.dcx",
    "/material/allmaterial_dlc02.matbinbnd.dcx",
    "/material/speedtree.matbinbnd.dcx",
];

/// The binders holding Elden Ring's shader bundles: the base game's, its two later additions', SpeedTree's. Each
/// bundle name is in one of them. (The `_[rt]` binders beside them hold the ray-traced variants.)
pub(crate) const SHADER_BINDERS: &[&str] = &[
    "/shader/shaderbdle.shaderbdlebnd.dcx",
    "/shader/shaderbdle_dlc01.shaderbdlebnd.dcx",
    "/shader/shaderbdle_dlc02.shaderbdlebnd.dcx",
    "/shader/speedtree.shaderbdlebnd.dcx",
];

/// The texture packages every map can bind: the system and decal textures.
pub(crate) const COMMON_TEXTURES: &[&str] = &["/other/systex.tpf.dcx", "/other/decaltex.tpf.dcx"];

/// The texture packages every character can bind: [`COMMON_TEXTURES`] and `/parts/common_body.tpf.dcx`, which holds
/// the textures the character materials bind from the parts folder (their matbin sampler paths are
/// `N:\GR\data\Model\parts\tex\AAT100_Crystal_g.tif` and the like): 97 textures, the `aat100_*` skin, scale, leather,
/// fabric, metal, rust, mud, bone and crystal albedo and normal maps and the `aat500_*` belt, hair, shell fur, beard
/// and teeth ones (measured by opening the package in an install of the game). A character's own texture binders
/// (`chr/<id>_h.texbnd`, `_l`) do not hold them: without this package every such binding is unavailable and a
/// material whose only albedo is one draws the preview's fallback grey.
pub(crate) const CHARACTER_TEXTURES: &[&str] = &[
    "/parts/common_body.tpf.dcx",
    "/other/systex.tpf.dcx",
    "/other/decaltex.tpf.dcx",
];

/// The package of an asset texture `aetNNN_MMM_<suffix>`: `/asset/aet/aetNNN/aetNNN_MMM.tpf.dcx` (its full
/// resolution; the `_l` package beside it holds the low one). `None` for another name.
pub(crate) fn texture_package(stem: &str) -> Option<String> {
    let group = stem
        .get(..6)
        .filter(|g| g.starts_with("aet") && g[3..].bytes().all(|b| b.is_ascii_digit()))?;
    let id = stem
        .get(7..10)
        .filter(|id| stem.as_bytes().get(6) == Some(&b'_') && id.bytes().all(|b| b.is_ascii_digit()))?;
    Some(format!("/asset/aet/{group}/{group}_{id}.tpf.dcx"))
}

#[cfg(test)]
mod tests {
    use super::{CHARACTER_TEXTURES, COMMON_TEXTURES, asset_model, parent_tiles, texture_package};

    /// Hand-worked: tile 39_54 is cell (1, 0) of the 512 m tile 19_27 (centre offset -128 + 256 = +128 in x, -128 in
    /// z) and cell (3, 2) of the 1024 m tile 09_13 (x: 3 * 256 + 128 - 512 = 384, z: 2 * 256 + 128 - 512 = 128).
    #[test]
    fn parents_of_a_small_tile() {
        assert_eq!(
            parent_tiles("m60_39_54_00"),
            vec![
                ("m60_19_27_01".to_owned(), [128., -128.]),
                ("m60_09_13_02".to_owned(), [384., 128.]),
            ]
        );
        assert!(parent_tiles("m14_00_00_00").is_empty());
        assert!(parent_tiles("m60_19_27_01").is_empty());
        // twelve bytes, not ASCII: no tile, and no slice through a character
        assert!(parent_tiles("m60_3é_4_00").is_empty());
    }

    #[test]
    fn map_folders_refuse_names_that_are_not_ascii() {
        use crate::Game;
        assert_eq!(
            Game::EldenRing.map_folder("m60_35_46_00").unwrap(),
            "/map/m60/m60_35_46_00"
        );
        assert_eq!(Game::Sekiro.map_folder("m11_01").unwrap(), "/map/m11_01");
        for game in [Game::EldenRing, Game::Sekiro] {
            assert!(
                game.map_folder("mé0_35_46_00")
                    .unwrap_err()
                    .to_string()
                    .contains("not ASCII")
            );
        }
    }

    #[test]
    fn texture_packages() {
        assert_eq!(
            texture_package("aet007_310_a").as_deref(),
            Some("/asset/aet/aet007/aet007_310.tpf.dcx")
        );
        assert_eq!(
            texture_package("aet000_124_n_l").as_deref(),
            Some("/asset/aet/aet000/aet000_124.tpf.dcx")
        );
        assert!(texture_package("aat000_fabric_01_a").is_none());
        assert!(texture_package("systex_dummy").is_none());
    }

    /// Characters bind the parts folder's textures (`AAT100_*`, `AAT500_*`) besides the packages the maps bind.
    #[test]
    fn characters_read_the_common_body_package_and_the_map_packages() {
        use crate::Game;
        assert_eq!(Game::EldenRing.character_shared_textures(), CHARACTER_TEXTURES);
        assert!(CHARACTER_TEXTURES.contains(&"/parts/common_body.tpf.dcx"));
        assert!(COMMON_TEXTURES.iter().all(|p| CHARACTER_TEXTURES.contains(p)));
        // the maps do not read it
        assert!(
            !Game::EldenRing
                .common_textures()
                .contains(&"/parts/common_body.tpf.dcx")
        );
    }

    #[test]
    fn asset_paths() {
        let m = asset_model("AEG007_310").unwrap();
        assert_eq!(m.binder, "/asset/aeg/aeg007/aeg007_310.geombnd.dcx");
        assert_eq!(m.flver, "aeg007_310.flver");
        assert_eq!(
            m.collision[1].binder.as_deref(),
            Some("/asset/aeg/aeg007/aeg007_310_l.geomhkxbnd.dcx")
        );
        assert_eq!(m.collision[0].member, "aeg007_310_h.hkx");
        assert!(asset_model("m000100").is_err());
    }
}
