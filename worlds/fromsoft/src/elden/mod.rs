//! Elden Ring's own rules: what differs from the other FromSoftware games and is not stated by the files themselves.
use crate::bhd::{PathHash, RecordLayout};
use crate::game::{ArchivePair, ArchiveSet, CollisionSource, KeySource, ObjectModel, Resolution};
use crate::{Result, invalid};

/// Data0 to Data3, the downloadable content's DLC and the sound archives `sd/sd` and `sd/sd_dlc02`, as the community
/// unpacker (UXM's `EldenRingGameInfo.xml`) lists them; base-133 u64 path hashes; keys supplied by the caller.
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
    keys: KeySource::Supplied,
};

/// An asset model `AEGnnn_mmm`: its binder `/asset/aeg/aegnnn/aegnnn_mmm.geombnd.dcx` and FLVER member; its collision is
/// in binders of its own, `aegnnn_mmm_h.geomhkxbnd.dcx` (high resolution, member `aegnnn_mmm_h.hkx`, a subset of the
/// assets) and `aegnnn_mmm_l.geomhkxbnd.dcx` (low, `aegnnn_mmm_l.hkx`, every collidable asset).
pub(crate) fn asset_model(model: &str) -> Result<ObjectModel> {
    let lower = model.to_ascii_lowercase();
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
    if tile.len() != 12 || !tile.ends_with("_00") {
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

/// The texture packages every map can bind: the system and decal textures.
pub(crate) const COMMON_TEXTURES: &[&str] = &["/other/systex.tpf.dcx", "/other/decaltex.tpf.dcx"];

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
    use super::{asset_model, parent_tiles, texture_package};

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
