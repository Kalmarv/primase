//! Installed colour-grading strips, with only independently established SourceID bindings enabled.
use fromsoft_formats::{Archive, Game, Result, dcx::Decoder, tpf};
use primase_core::TexFormat;

/// A normalized RGB lookup, red fastest, then green, then blue. No input gamma or colour correction is invented.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourLookup {
    pub texture: String,
    pub side: u32,
    pub values: Vec<[f32; 3]>,
}

/// Decode the established 16-by-256 RGBA8 strip layout into its 16-cube RGB values.
/// The three-dimensional orientation and normalization match every RGB word of the native Ashina LUT readback.
pub fn from_texture(texture: &tpf::Texture) -> Result<ColourLookup> {
    let refuse = |detail| crate::invalid("Sekiro colour LUT", 0, detail);
    if texture.kind != tpf::TextureKind::Image2D || texture.format != 105 {
        return Err(refuse("expected a 2D RGBA8 colour-grading strip"));
    }
    let image =
        primase_dds::read(&texture.dds).map_err(|error| crate::invalid("Sekiro colour LUT", 0, error))?;
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
                    f32::from(pixel[0]) / 255.0,
                    f32::from(pixel[1]) / 255.0,
                    f32::from(pixel[2]) / 255.0,
                ]
            })
            .collect(),
    })
}

/// Read a SourceID binding established against the native cube. Other areas and IDs are refused by name.
/// SourceID 0 in Ashina selects `m11_01_CGrading0000` from the map's installed grading package.
pub fn read(archive: &Archive, decoder: &Decoder<'_>, area: &str, source_id: i32) -> Result<ColourLookup> {
    if archive.game() != Game::Sekiro || !matches!(area, "m11_01" | "m11_01_00_00") || source_id != 0 {
        return Err(crate::invalid(
            "Sekiro colour LUT",
            0,
            format!("binding for {area} SourceID {source_id} is not established"),
        ));
    }
    let bytes = archive.read("/map/m11/m11_cgrading.tpf.dcx", decoder)?;
    let package = tpf::Package::read(&bytes)?;
    let mut matching = package
        .textures
        .iter()
        .filter(|texture| texture.name.eq_ignore_ascii_case("m11_01_CGrading0000"));
    let texture = matching
        .next()
        .ok_or_else(|| crate::invalid("Sekiro colour LUT", 0, "m11_01_CGrading0000 is missing"))?;
    if matching.next().is_some() {
        return Err(crate::invalid(
            "Sekiro colour LUT",
            0,
            "m11_01_CGrading0000 is not unique",
        ));
    }
    from_texture(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unestablished_binding_does_not_fall_back_to_the_observed_cube() {
        let archive = Archive::empty(Game::Sekiro);
        let decoder = Decoder {
            oodle: None,
            max_output: 1 << 20,
        };
        let error = read(&archive, &decoder, "m11_01", 20).unwrap_err();
        assert!(error.to_string().contains("SourceID 20 is not established"));
    }
}
