//! Installed colour-grading strips, with only independently established SourceID bindings enabled.
use fromsoft_formats::{Archive, Game, Result, dcx::Decoder, tpf};
use primase_core::TexFormat;

pub use crate::sekiro::source::ColourLookupData as ColourLookup;

/// Decode the established 16-by-256 RGBA8 strip layout into its 16-cube RGB values.
/// The three-dimensional orientation and normalization match every RGB word of the native Ashina LUT readback.
pub fn from_texture(texture: &tpf::Texture) -> Result<ColourLookup> {
    decode_strip(texture, CubeStorage::Float32)
}

#[derive(Clone, Copy)]
enum CubeStorage {
    Float32,
    Float16,
}

impl CubeStorage {
    fn unorm8(self, byte: u8) -> f32 {
        let value = f32::from(byte) / 255.;
        match self {
            Self::Float32 => value,
            Self::Float16 => {
                // The native typed UAV store narrows float32 toward zero (Direct3D section 3.2.2).
                // UNORM8 is nonnegative and every nonzero value is normal in binary16.
                let nearest = half::f16::from_f32(value);
                let bits = nearest.to_bits() - u16::from(nearest.to_f32() > value);
                half::f16::from_bits(bits).to_f32()
            }
        }
    }
}

fn decode_strip(texture: &tpf::Texture, storage: CubeStorage) -> Result<ColourLookup> {
    let refuse = |detail| crate::invalid("FromSoft colour LUT", 0, detail);
    if texture.kind != tpf::TextureKind::Image2D || texture.format != 105 {
        return Err(refuse("expected a 2D RGBA8 colour-grading strip"));
    }
    let image =
        primase_dds::read(&texture.dds).map_err(|error| crate::invalid("FromSoft colour LUT", 0, error))?;
    if image.width != 16 || image.height != 256 || image.format != TexFormat::Rgba8Unorm {
        return Err(refuse("expected a 16-by-256 RGBA8 colour-grading strip"));
    }
    let pixels = image
        .levels
        .first()
        .ok_or_else(|| refuse("missing strip pixels"))?;
    if pixels.len() != 16 * 256 * 4 {
        return Err(refuse("incomplete colour-grading strip"));
    }
    Ok(ColourLookup {
        texture: texture.name.clone(),
        side: 16,
        values: pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| {
                [
                    storage.unorm8(pixel[0]),
                    storage.unorm8(pixel[1]),
                    storage.unorm8(pixel[2]),
                ]
            })
            .collect(),
    })
}

/// Read a SourceID binding established against native evidence. Other areas and IDs are refused by name.
/// SourceID 0 in Ashina selects `m11_01_CGrading0000` from the map's installed grading package.
/// Stormveil SourceID 0 selects `m10_00_CGrading0000`; its copy shader stores a binary16 cube.
pub fn read(archive: &Archive, decoder: &Decoder<'_>, area: &str, source_id: i32) -> Result<ColourLookup> {
    let (path, name, storage) = match (archive.game(), area, source_id) {
        (Game::Sekiro, "m11_01" | "m11_01_00_00", 0) => (
            "/map/m11/m11_cgrading.tpf.dcx",
            "m11_01_CGrading0000",
            CubeStorage::Float32,
        ),
        (Game::EldenRing, "m10_00" | "m10_00_00_00", 0) => (
            "/map/m10/common/m10_cgrading.tpf.dcx",
            "m10_00_CGrading0000",
            CubeStorage::Float16,
        ),
        _ => {
            return Err(crate::invalid(
                "FromSoft colour LUT",
                0,
                format!("binding for {area} SourceID {source_id} is not established"),
            ));
        }
    };
    let bytes = archive.read(path, decoder)?;
    let package = tpf::Package::read(&bytes)?;
    let mut matching = package
        .textures
        .iter()
        .filter(|texture| texture.name.eq_ignore_ascii_case(name));
    let texture = matching
        .next()
        .ok_or_else(|| crate::invalid("FromSoft colour LUT", 0, format!("{name} is missing")))?;
    if matching.next().is_some() {
        return Err(crate::invalid(
            "FromSoft colour LUT",
            0,
            format!("{name} is not unique"),
        ));
    }
    decode_strip(texture, storage)
}

/// Keep the established SourceID-zero binding, or its named refusal, beside the area's selected source records.
pub fn read_into(world: &mut primase_core::World, archive: &Archive, decoder: &Decoder<'_>, area: &str) {
    world.insert_resource(crate::sekiro::source::PostLookup {
        source_id: 0,
        value: read(archive, decoder, area, 0).map_err(|error| error.to_string()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unestablished_binding_does_not_fall_back_to_the_observed_cube() {
        let decoder = Decoder {
            oodle: None,
            max_output: 1 << 20,
        };
        for (game, area) in [(Game::Sekiro, "m11_01"), (Game::EldenRing, "m10_00")] {
            let archive = Archive::empty(game);
            let error = read(&archive, &decoder, area, 20).unwrap_err();
            assert!(error.to_string().contains("SourceID 20 is not established"));
        }
    }

    #[test]
    fn half_cube_stores_truncate_every_unorm8_value_toward_zero() {
        for byte in 0..=255 {
            let input = f32::from(byte) / 255.;
            let stored = CubeStorage::Float16.unorm8(byte);
            // Binary16 keeps ten fraction bits, leaving the lowest thirteen binary32 bits zero.
            let expected = f32::from_bits(input.to_bits() & !0x1fff);
            assert_eq!(stored.to_bits(), expected.to_bits(), "UNORM8 {byte}");
        }
    }
}
