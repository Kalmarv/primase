//! Textures by the hash materials name them by: [`forzatech_formats::material::path_hash`] of a texture's game path.
//!
//! The table ([`TextureTable::of`]): every `.pb` entry of the track's GeoChunks, named
//! `...\zipcache\pc\<path>\<name>_<guid>_quality<N>.pb`, is quality tier `N` of the texture whose game path is
//! `game:\media\<path>\<name>_<guid>.swatchbin`; a texture has one entry per tier its chunks hold. And every
//! `.swatchbin` of the install's texture library ([`Track::texture_library`]: `_library\Textures.zip` and the
//! `_library\TexturesPG` zips) is the one tier of the texture whose game path is the zip's path in `media` less `.zip`,
//! then the entry's (`game:\media\_library\texturespg\defaulttextures\swatches\<name>.swatchbin`): observed on the
//! default textures the Shibuya Scramble box's shaders name, not a census. Two game paths of one hash, or two tiers of
//! one quality, are refused.
//!
//! A texture entity ([`load_texture`]) holds the tiers the caller's scope loads ([`TextureScope`], the read scope's
//! `texture-form`): with every tier, the one whose payload is largest as its `core.texture` and the others as its forms
//! (`core.texture_forms`, ranked by width: no tier is chosen); with one tier by size, that tier alone, the others left
//! unloaded and recorded ([`Unloaded`]). Each is the swatch as stored: its format, the mips of its one slice
//! (block-compressed levels kept encoded), sRGB or linear by its colour profile. What a material samples it as (colour,
//! normal, mask) is the material's, so its role is `Other` until a material lowering states it.
use super::open::{Archive, Track};
use crate::{Error, Result};
use forzatech_formats::material::path_hash;
use forzatech_formats::texture::{Encoding, Header, Texture as Swatch};
use forzatech_formats::track::Location;
use primase_core::{
    ColorSpace, Entity, Name, Source, TexFormat, TexRole, Texture, TextureForm, TextureForms, World,
};
use std::collections::HashMap;

/// Where a tier is stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// A GeoChunk entry.
    Chunk(Location),
    /// An entry of a library zip: the zip's index in [`TextureTable`]'s zips and the entry's name.
    Zip { zip: usize, entry: String },
}

/// One stored tier of a texture: its quality number (`None` for a library zip's `.swatchbin`, which has one) and where
/// it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tier {
    pub quality: Option<u8>,
    pub at: Stored,
}

/// The textures of a track and the install's texture library by path hash.
#[derive(Debug, Default)]
pub struct TextureTable {
    by_hash: HashMap<u32, (String, Vec<Tier>)>,
    zips: Vec<Archive>,
}

impl TextureTable {
    /// The table of `track`'s GeoChunk textures and the library zips [`Track::texture_library`] opens.
    pub fn of(track: &Track) -> Result<TextureTable> {
        let mut table = TextureTable::default();
        for (at, path) in track.paths() {
            let Some(stem) = path.strip_suffix(".pb") else {
                continue;
            };
            let Some((stem, quality)) = stem.rsplit_once("_quality") else {
                return Err(Error::Track(format!(
                    "{path}: a .pb entry not named <name>_quality<N>.pb"
                )));
            };
            let quality: u8 = quality
                .parse()
                .map_err(|_| Error::Track(format!("{path}: quality {quality:?} is not a number")))?;
            table.add(
                format!("game:\\media\\{stem}.swatchbin"),
                Some(quality),
                Stored::Chunk(at),
            )?;
        }
        for (folder, zip) in track.texture_library()? {
            let names: Vec<String> = zip
                .names()
                .filter(|n| n.ends_with(".swatchbin"))
                .map(str::to_owned)
                .collect();
            let k = table.zips.len();
            table.zips.push(zip);
            for entry in names {
                let game = format!("game:\\media\\{folder}\\{}", entry.replace('/', "\\"));
                table.add(game, None, Stored::Zip { zip: k, entry })?;
            }
        }
        Ok(table)
    }

    /// Adds a tier; refused: two game paths of one hash, two tiers of one quality.
    fn add(&mut self, game: String, quality: Option<u8>, at: Stored) -> Result<()> {
        let hash = path_hash(&game);
        let (first, tiers) = self
            .by_hash
            .entry(hash)
            .or_insert_with(|| (game.clone(), Vec::new()));
        if *first != game {
            return Err(Error::Track(format!(
                "{first} and {game} have one path hash, {hash:#010x}"
            )));
        }
        if tiers.iter().any(|t| t.quality == quality) {
            return Err(Error::Track(format!(
                "{game}: two entries of quality {quality:?}"
            )));
        }
        tiers.push(Tier { quality, at });
        Ok(())
    }

    /// A texture's game path and tiers.
    pub fn get(&self, hash: u32) -> Option<(&str, &[Tier])> {
        self.by_hash.get(&hash).map(|(p, t)| (p.as_str(), t.as_slice()))
    }

    /// A tier's bytes.
    pub fn read(&self, track: &Track, tier: &Tier) -> Result<Vec<u8>> {
        match &tier.at {
            Stored::Chunk(at) => track.read(*at),
            Stored::Zip { zip, entry } => {
                let z = self
                    .zips
                    .get(*zip)
                    .ok_or_else(|| Error::Track(format!("no library zip {zip}")))?;
                z.read(entry)?
                    .ok_or_else(|| Error::Track(format!("{}: no entry {entry}", z.path().display())))
            }
        }
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// Which of a texture's stored tiers a read loads: the caller's scope (`texture-form`), never a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureScope {
    /// Every tier (`all`): the largest as the texture's own, the others as its forms.
    All,
    /// The smallest tier at least `N` pixels on its larger side, the largest when none is (`reach-N`); the others are
    /// left unloaded. The track's tiers of a texture are its halvings, one level each but the smallest, which holds the
    /// rest of the chain (observed over the Shibuya Scramble box's textures).
    Reaching(u32),
}

impl TextureScope {
    /// The scope as its option value: `all` or `reach-N`.
    pub fn value(self) -> String {
        match self {
            TextureScope::All => "all".into(),
            TextureScope::Reaching(n) => format!("reach-{n}"),
        }
    }

    /// The scope of an option value; refused: anything but `all` or `reach-N` with `N` at least 1.
    pub fn parse(value: &str) -> std::result::Result<TextureScope, String> {
        if value == "all" {
            return Ok(TextureScope::All);
        }
        value
            .strip_prefix("reach-")
            .and_then(|n| n.parse().ok())
            .filter(|&n| n > 0)
            .map(TextureScope::Reaching)
            .ok_or_else(|| {
                format!("texture-form {value:?}: needs all or reach-N (N a size of 1 or more pixels)")
            })
    }
}

/// The scopes of a read's textures: the far-field terrain textures' (`terrain-texture-form`: those under a
/// `tbheightfield` folder, the cells' pre-baked ground) and every other texture's (`texture-form`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureScopes {
    pub textures: TextureScope,
    pub terrain: TextureScope,
}

impl TextureScopes {
    /// The scope of the texture stored at `game` (its game path), and the option that states it.
    pub fn of(self, game: &str) -> (TextureScope, &'static str) {
        if is_terrain(game) {
            (self.terrain, "terrain-texture-form")
        } else {
            (self.textures, "texture-form")
        }
    }
}

/// Whether the texture stored at `game` is a far-field terrain texture: a folder of its path is `tbheightfield`.
fn is_terrain(game: &str) -> bool {
    game.split(['\\', '/'])
        .rev()
        .skip(1)
        .any(|dir| dir.eq_ignore_ascii_case("tbheightfield"))
}

/// A stored tier a read left unloaded: its size (`WxH`), its identity (the texture's game path and quality) and the
/// option whose scope left it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unloaded {
    pub size: String,
    pub source: String,
    pub option: &'static str,
}

/// What a [`TextureScope::Reaching`] read did to the tiers: those it left unloaded, and the textures none of whose
/// tiers reaches the size, given their largest; and the far-field terrain textures loaded (by `terrain-texture-form`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TierRecord {
    pub unloaded: Vec<Unloaded>,
    pub short: usize,
    pub terrain: usize,
}

/// A swatch's DXGI-equivalent core format.
pub fn format(e: Encoding) -> TexFormat {
    match e {
        Encoding::Bc1 => TexFormat::Bc1,
        Encoding::Bc2 => TexFormat::Bc2,
        Encoding::Bc3 => TexFormat::Bc3,
        Encoding::Bc4Unorm => TexFormat::Bc4Unorm,
        Encoding::Bc4Snorm => TexFormat::Bc4Snorm,
        Encoding::Bc5Unorm => TexFormat::Bc5Unorm,
        Encoding::Bc5Snorm => TexFormat::Bc5Snorm,
        Encoding::Bc6hUf16 => TexFormat::Bc6hUfloat,
        Encoding::Bc6hSf16 => TexFormat::Bc6hSfloat,
        Encoding::Bc7 => TexFormat::Bc7,
        Encoding::R32G32B32A32Float => TexFormat::Rgba32Float,
        Encoding::R16G16B16A16Unorm => TexFormat::Rgba16Unorm,
        Encoding::R16G16B16A16Float => TexFormat::Rgba16Float,
        Encoding::R8G8B8A8Unorm => TexFormat::Rgba8Unorm,
        Encoding::B5G6R5Unorm => TexFormat::B5g6r5Unorm,
        Encoding::B5G5R5A1Unorm => TexFormat::B5g5r5a1Unorm,
        Encoding::R8Unorm => TexFormat::R8Unorm,
        Encoding::A8Unorm => TexFormat::A8Unorm,
    }
}

/// Of tier sizes listed finest first, the index of the smallest at least `n` pixels on its larger side (the last listed
/// of equal sizes); `None` when none is.
fn reaching(sizes: &[(u32, u32)], n: u32) -> Option<usize> {
    sizes.iter().rposition(|&(w, h)| w.max(h) >= n)
}

/// A swatch's PC header; refused: a Durango (console) payload.
fn pc_header<'s>(s: &'s Swatch<'_>) -> Result<&'s forzatech_formats::texture::PcHeader> {
    match &s.header {
        Header::Pc(h) => Ok(h),
        Header::Durango(_) => Err(Error::Model(
            "a Durango texture: its tiled payload is not laid out in the file".into(),
        )),
    }
}

/// A swatch's bytes as a core texture: its one slice's mips as stored. Refused: a Durango (console) payload, an array,
/// a cube or a volume.
pub fn texture(bytes: &[u8]) -> Result<Texture> {
    let s = Swatch::parse(bytes)?;
    let h = pc_header(&s)?;
    let [slice] = h.slices.as_slice() else {
        return Err(Error::Model(format!(
            "{} slices: arrays and cubes are not read",
            h.slices.len()
        )));
    };
    if h.depth != 1 {
        return Err(Error::Model(format!("depth {}: volumes are not read", h.depth)));
    }
    let levels = (0..slice.mips.len())
        .map(|k| {
            s.mip(0, k)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| Error::Model(format!("mip {k} out of range")))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Texture {
        width: h.width,
        height: h.height,
        format: format(slice.encoding),
        levels,
        color_space: if h.srgb() {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        },
        role: TexRole::Other,
        normal: None,
    })
}

/// The texture entity of `hash`, made on first use with the tiers its scope in `scopes` loads, what it leaves recorded
/// in `record`; `None` when no archive holds it.
pub fn load_texture(
    track: &Track,
    table: &TextureTable,
    scopes: TextureScopes,
    made: &mut HashMap<u32, Entity>,
    world: &mut World,
    hash: u32,
    record: &mut TierRecord,
) -> Result<Option<Entity>> {
    if let Some(e) = made.get(&hash) {
        return Ok(Some(*e));
    }
    let Some((game, tiers)) = table.get(hash) else {
        return Ok(None);
    };
    let (scope, option) = scopes.of(game);
    if is_terrain(game) {
        record.terrain += 1;
    }
    let at = |t: &Tier| match t.quality {
        Some(q) => format!("{game} quality {q}"),
        None => game.to_owned(),
    };
    let mut stored = Vec::with_capacity(tiers.len());
    for t in tiers {
        let bytes = table.read(track, t)?;
        let size = Swatch::parse(&bytes)
            .map_err(Error::from)
            .and_then(|s| pc_header(&s).map(|h| (h.width, h.height)))
            .map_err(|e| Error::Model(format!("{}: {e}", at(t))))?;
        stored.push((t, size, bytes));
    }
    // finest first: by width, then height
    stored.sort_by_key(|(_, size, _)| std::cmp::Reverse(*size));
    if let TextureScope::Reaching(n) = scope {
        let sizes: Vec<(u32, u32)> = stored.iter().map(|(_, size, _)| *size).collect();
        let keep = reaching(&sizes, n).unwrap_or_else(|| {
            record.short += 1;
            0
        });
        for (k, (t, (w, h), _)) in stored.iter().enumerate() {
            if k != keep {
                record.unloaded.push(Unloaded {
                    size: format!("{w}x{h}"),
                    source: at(t),
                    option,
                });
            }
        }
        let kept = stored.swap_remove(keep);
        stored = vec![kept];
    }
    let mut forms = Vec::with_capacity(stored.len());
    for (t, _, bytes) in stored {
        let tex = texture(&bytes).map_err(|e| Error::Model(format!("{}: {e}", at(t))))?;
        forms.push((t.quality, tex));
    }
    let mut forms = forms.into_iter();
    let Some((_, own)) = forms.next() else {
        return Ok(None);
    };
    let e = world.spawn();
    world.insert(e, own);
    let name = game.rsplit('\\').next().unwrap_or(game);
    world.insert(e, Name(name.trim_end_matches(".swatchbin").to_owned()));
    world.insert(e, Source(game.to_owned()));
    let others: Vec<TextureForm> = forms
        .enumerate()
        .map(|(k, (_, t))| {
            let f = world.spawn();
            world.insert(f, t);
            TextureForm {
                detail: u8::try_from(k + 1).unwrap_or(u8::MAX),
                texture: f,
            }
        })
        .collect();
    if !others.is_empty() {
        world.insert(
            e,
            TextureForms {
                detail: 0,
                forms: others,
            },
        );
    }
    made.insert(hash, e);
    Ok(Some(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texture-form value round-trips; anything but `all` or `reach-N` (N at least 1) is refused.
    #[test]
    fn the_scope_is_all_or_a_size() {
        for v in ["all", "reach-256", "reach-1"] {
            assert_eq!(TextureScope::parse(v).map(TextureScope::value), Ok(v.to_owned()));
        }
        for v in ["", "reach-0", "reach-", "256", "finest", "reach-x"] {
            assert!(TextureScope::parse(v).is_err(), "{v}");
        }
    }

    /// The smallest tier reaching the size by its larger side, whatever the aspect; none when every tier is smaller.
    #[test]
    fn reaching_keeps_the_smallest_tier_at_least_the_size() {
        let square = [(1024, 1024), (512, 512), (256, 256), (128, 128)];
        assert_eq!(reaching(&square, 256), Some(2));
        assert_eq!(reaching(&square, 200), Some(2));
        assert_eq!(reaching(&square, 257), Some(1));
        assert_eq!(reaching(&square, 1), Some(3));
        assert_eq!(reaching(&square, 2048), None);
        let tall = [
            (256, 4096),
            (128, 2048),
            (64, 1024),
            (32, 512),
            (16, 256),
            (8, 128),
        ];
        assert_eq!(reaching(&tall, 256), Some(4));
    }

    #[test]
    fn a_texture_under_a_tbheightfield_folder_takes_the_terrain_scope() {
        let scopes = TextureScopes {
            textures: TextureScope::Reaching(256),
            terrain: TextureScope::All,
        };
        for game in [
            "textures\\TBHeightfield\\swatches\\submap_x-3069_z-10230_xsummerx_diff.swatchbin",
            "media/tracks/brio/textures/tbheightfield/swatches/mainmap.swatchbin",
        ] {
            assert_eq!(
                scopes.of(game),
                (TextureScope::All, "terrain-texture-form"),
                "{game}"
            );
        }
        for game in [
            "textures\\city\\pavement_cty_asp_darkasphalt_bclr.swatchbin",
            // the file's own name is not a folder
            "textures\\swatches\\tbheightfield",
        ] {
            assert_eq!(
                scopes.of(game),
                (TextureScope::Reaching(256), "texture-form"),
                "{game}"
            );
        }
    }
}
