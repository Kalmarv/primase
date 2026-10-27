//! The crowd's skin palette: a TextureMap decoded and sampled.

use anvil_formats::archive::Archive;
use anvil_formats::material::TEXTURE_MAP;

use crate::read::textures::read_texture_map;

/// Why the skin palette can't be read.
#[derive(Debug, thiserror::Error)]
pub enum PaletteError {
    #[error(transparent)]
    Texture(#[from] crate::read::textures::Error),
    #[error("no TextureMap named {0} in the forges, or it holds no image")]
    NoTexture(&'static str),
    #[error("the skin palette is {width} x {height}, not 8 x 4")]
    Palette { width: u32, height: u32 },
}

/// The texture of the crowd's skin palette (`MAD_CHR_BULK`'s: the one skin palette the forges hold).
pub const SKIN_PALETTE: &str = "MAD_CHR_SkinColors_DiffuseMap_PC_Scarlett_Prospero";

/// The colours of the skin palette's four tones, R, G, B: the palette is 8 x 4 texels of 2 x 2 cells (four cells across
/// and two down, the cell of tone `n` at x = 2 n); the bottom row's cells are the tones (the top row's role is not
/// established, so it is not given). `MAD_CHR_SkinColors_DiffuseMap` is referenced by the crowd templates `MAD_CHR_BULK`
/// and `MAD_CHR_Bulk_Test` only; no rider table links it.
pub fn skin_palette(archive: &mut Archive) -> Result<Vec<[u8; 3]>, PaletteError> {
    let id = (0..)
        .map_while(|forge| archive.forge(forge))
        .find_map(|f| {
            f.entries()
                .find(|(_, n)| n.type_id == TEXTURE_MAP && n.name == SKIN_PALETTE)
                .map(|(i, _)| i.id)
        })
        .ok_or(PaletteError::NoTexture(SKIN_PALETTE))?;
    let mut world = primase_core::World::new();
    let e = read_texture_map(archive, &mut world, id)?.ok_or(PaletteError::NoTexture(SKIN_PALETTE))?;
    let texture = world
        .get::<primase_core::Texture>(e)
        .ok_or(PaletteError::NoTexture(SKIN_PALETTE))?;
    let levels = primase_image::levels(texture).map_err(|_| PaletteError::NoTexture(SKIN_PALETTE))?;
    if (levels.width, levels.height) != (8, 4) {
        return Err(PaletteError::Palette {
            width: levels.width,
            height: levels.height,
        });
    }
    let at = |x: i64, y: i64| {
        let t = levels.texel(0, x, y, [primase_foundations::sampling::Address::Clamp; 2]);
        [t[0], t[1], t[2]]
    };
    Ok((0..4).map(|n| at(2 * n, 2)).collect())
}
