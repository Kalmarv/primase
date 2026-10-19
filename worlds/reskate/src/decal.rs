//! The stock decal row's per-vertex opacity, as measured in game: `Color0.r` is a decal's opacity (with a mask-shaped
//! dissolve; green, blue and alpha have no effect), and exactly (0, 0, 0, 0) is a "no vertex colour" value that draws
//! fully opaque. Shared by the passes that decide which decal corners show (`lower-layered-reskate`,
//! `lower-vertex-alpha-reskate`) and the post-compile stage that writes the values (`postcompile::vblend`).

/// `Color0` = (0, 0, 0, 0) draws a decal fully opaque (measured): never written to a decal section.
pub const NO_VERTEX_COLOUR: [u8; 4] = [0; 4];

/// The `Color0` that gives a decal opacity `w` (0..1, clamped): (w, 0, 0, 1). Alpha is 1 so that `w = 0` is
/// transparent rather than [`NO_VERTEX_COLOUR`].
pub fn decal_opacity(w: f32) -> [u8; 4] {
    let r = (w.clamp(0.0, 1.0) * 255.0).round() as u8;
    [r, 0, 0, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decal_opacity_is_never_the_opaque_sentinel() {
        // alpha is always 255, so no weight is the all-zero sentinel; an interior weight by hand: 0.5 x 255 = 127.5,
        // rounded half away from zero to 128
        assert_eq!(decal_opacity(0.5), [128, 0, 0, 255]);
        assert_ne!(decal_opacity(0.5), NO_VERTEX_COLOUR);
        assert_eq!(decal_opacity(0.0), [0, 0, 0, 255]);
        assert_eq!(decal_opacity(1.0), [255, 0, 0, 255]);
        assert_eq!(decal_opacity(-3.0), [0, 0, 0, 255]);
    }
}
