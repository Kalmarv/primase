//! The GPU half of [`primase_core::shader::surface`]: a program's [`Surface`] packed as rows of four words a uniform
//! table holds ([`rows`]), and [`WGSL`], the functions that decode a G-buffer's values by those rows as the CPU reading
//! does. A composite that shows a program's outputs lit binds the program's render targets, reads the target a row names
//! and passes its four values to these functions: which lane is which term, and how it is stored, is never the
//! composite's to say.
//!
//! The layout of a row is described at the head of [`WGSL`]; the row indices below are its constants.
use primase_core::shader::{NormalEncoding, ScalarEncoding};
use primase_core::shader::surface::{Base, Emission, Place, Roughness, Surface, Term};

/// The WGSL functions: `primase_base`, `primase_normal`, `primase_roughness`, `primase_scalar`, `primase_colour` and
/// `primase_emission` decode a term, `primase_stored` shows its lanes as the program wrote them; each takes a row and
/// its target's four values. With the row constants `PRIMASE_SURFACE_*`; module scope, no bindings: a consumer puts it
/// beside its own shader.
pub const WGSL: &str = include_str!("surface.wgsl");

/// Rows per program.
pub const ROWS: usize = 9;
/// The surface's colour ([`Base`]): kind 1 an albedo, 2 a finished colour.
pub const BASE: usize = 0;
/// The normal: the kind is its encoding ([`normal_kind`]).
pub const NORMAL: usize = 1;
/// The roughness ([`Roughness`]): kind 1 stored, 2 a gloss, 3 a specular exponent.
pub const ROUGHNESS: usize = 2;
pub const METALLIC: usize = 3;
pub const REFLECTANCE: usize = 4;
pub const SPECULAR: usize = 5;
/// The emission ([`Emission`]): kind 1 a colour, 2 a scale of the albedo.
pub const EMISSION: usize = 6;
/// The occlusion of every light ([`Surface::occlusion`]).
pub const OCCLUSION: usize = 7;
/// The occlusion of the environment's ambient light alone ([`Surface::ambient_occlusion`]): a composite scales its
/// ambient light by it and not its other lights.
pub const AMBIENT_OCCLUSION: usize = 8;

/// The kind a normal's row has for its encoding, and the two numbers its row carries (as f32 bits; 0 for none).
pub fn normal_kind(encoding: NormalEncoding) -> (u32, [u32; 2]) {
    match encoding {
        NormalEncoding::Xyz => (1, [0, 0]),
        NormalEncoding::XyzUnorm => (2, [0, 0]),
        NormalEncoding::XyzBiased { bias_bits } => (3, [bias_bits, 0]),
        NormalEncoding::Xy => (4, [0, 0]),
        NormalEncoding::XyUnorm => (5, [0, 0]),
        NormalEncoding::Octahedral { read, .. } => (6, [read.scale_bits, read.bias_bits]),
        NormalEncoding::OctahedralDiamond => (7, [0, 0]),
        NormalEncoding::CubeFace { minor, .. } => (8, [minor.scale_bits, minor.bias_bits]),
    }
}

/// The second word of a row: the place's first lane and width, the kind and the scalar encoding's power and root.
fn layout(place: Place, kind: u32, encoding: ScalarEncoding) -> u32 {
    (place.first() as u32 & 0x3)
        | (place.width() & 0xf) << 4
        | (kind & 0xff) << 8
        | u32::from(encoding.power) << 16
        | u32::from(encoding.root) << 24
}

/// The rows of a program's surface: row [`BASE`] to [`AMBIENT_OCCLUSION`], each `[target + 1, layout, a, b]` (the
/// layout at the head of [`WGSL`]). `target` gives the render target of an output by its position in the outputs the
/// surface was read from; a term whose output has none (`None`), and a term the surface does not have, is a row of
/// zeros, which the composite takes as no output having its role.
pub fn rows(surface: &Surface, target: impl Fn(usize) -> Option<u32>) -> [[u32; 4]; ROWS] {
    let mut rows = [[0u32; 4]; ROWS];
    let at = |output: usize| target(output).map(|t| t + 1);
    let term_row = |t: &Term, kind: u32| -> [u32; 4] {
        at(t.place.output).map_or([0; 4], |x| [x, layout(t.place, kind, t.encoding), t.encoding.scale_bits, 0])
    };
    if let Some(base) = &surface.base {
        let (place, kind) = match *base {
            Base::Albedo(p) => (p, 1),
            Base::Colour(p) => (p, 2),
        };
        if let Some(x) = at(place.output) {
            rows[BASE] = [x, layout(place, kind, ScalarEncoding::LINEAR), 0, 0];
        }
    }
    if let Some(n) = &surface.normal {
        let (kind, [a, b]) = normal_kind(n.encoding);
        if let Some(x) = at(n.place.output) {
            rows[NORMAL] = [x, layout(n.place, kind, ScalarEncoding::LINEAR), a, b];
        }
    }
    if let Some(r) = &surface.roughness {
        let kind = match r {
            Roughness::Roughness(_) => 1,
            Roughness::Gloss(_) => 2,
            Roughness::Exponent(_) => 3,
        };
        rows[ROUGHNESS] = term_row(&r.term(), kind);
    }
    for (row, term) in [
        (METALLIC, &surface.metallic),
        (REFLECTANCE, &surface.reflectance),
        (SPECULAR, &surface.specular),
        (OCCLUSION, &surface.occlusion),
        (AMBIENT_OCCLUSION, &surface.ambient_occlusion),
    ] {
        if let Some(t) = term {
            rows[row] = term_row(t, 1);
        }
    }
    if let Some(e) = &surface.emission {
        let kind = match e {
            Emission::Colour(_) => 1,
            Emission::OfAlbedo(_) => 2,
        };
        rows[EMISSION] = term_row(&e.term(), kind);
    }
    rows
}
