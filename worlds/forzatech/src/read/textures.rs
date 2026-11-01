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
//! A texture entity ([`load_texture`]) holds the tier whose payload is largest as its `core.texture` and the others as
//! its forms (`core.texture_forms`, ranked by width: no tier is chosen). Each is the swatch as stored: its format, the
//! mips of its one slice (block-compressed levels kept encoded), sRGB or linear by its colour profile. What a material
//! samples it as (colour, normal, mask) is the material's, so its role is `Other` until a material lowering states it.
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

/// A swatch's bytes as a core texture: its one slice's mips as stored. Refused: a Durango (console) payload, an array,
/// a cube or a volume.
pub fn texture(bytes: &[u8]) -> Result<Texture> {
    let s = Swatch::parse(bytes)?;
    let Header::Pc(h) = &s.header else {
        return Err(Error::Model(
            "a Durango texture: its tiled payload is not laid out in the file".into(),
        ));
    };
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

/// The texture entity of `hash`, made on first use with its tiers; `None` when no archive holds it.
pub fn load_texture(
    track: &Track,
    table: &TextureTable,
    made: &mut HashMap<u32, Entity>,
    world: &mut World,
    hash: u32,
) -> Result<Option<Entity>> {
    if let Some(e) = made.get(&hash) {
        return Ok(Some(*e));
    }
    let Some((game, tiers)) = table.get(hash) else {
        return Ok(None);
    };
    let mut forms = Vec::with_capacity(tiers.len());
    for t in tiers {
        let bytes = table.read(track, t)?;
        let tex =
            texture(&bytes).map_err(|e| Error::Model(format!("{game} quality {:?}: {e}", t.quality)))?;
        forms.push((t.quality, tex));
    }
    // finest first: by width, then height
    forms.sort_by_key(|(_, t)| std::cmp::Reverse((t.width, t.height)));
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
