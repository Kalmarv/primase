//! What a G-buffer pixel program's output stores say about the lanes the G-buffer's roles name, read from the program
//! lifted by [`primase_dxil::lift`]: for each colour output the interface declares (`SV_Target<n>`), the stores into
//! its lanes and how each stored value is made ([`OutputStores`], shared with every engine's G-buffer reading). Nothing
//! here is read from a program's name or a local's name.
//!
//! The G-buffer the `PS_DeferredTextured*` programs write (the layout is checked against the programs by evaluating
//! them, `tests/gbuffer_layout.rs`):
//!
//! | target | lanes | carries |
//! |---|---|---|
//! | 0 | xyz | the diffuse colour (texture times the material's colour, darkened by wetness) |
//! | 0 | w | a byte of the draw's state, from the engine's constants |
//! | 1 | xyz | the world normal biased to 0 to 1 (`n * 0.5 + 0.5`) |
//! | 1 | w | the normal's sub-step bits for the lighting passes (3, 3 and 2 bits of x, y and z in one byte; the twiddle effects write them, the others `saturate(128 z - 127)`): no role, `tests/gbuffer_normal_w.rs` |
//! | 2 | x | the specular intensity, stored as its square root |
//! | 2 | y | the specular falloff over 512, stored as its square root (the lighting passes read `y * y * 512`) |
//! | 2 | z | the Fresnel term |
//! | 3 | x, y | the occlusion of the natural and of the artificial ambient: `sqrt((a + k * vertex colour) / 2)` through constants; the ambient lighting pass scales each ambient by the square of twice the stored square (`tests/gbuffer_encodings.rs`) |
//! | 3 | z | the emission over 16 (the constant 0 where the effect has none) |
//! | 3 | w | a flag: where above one half the ambient pass multiplies both occlusions by `deferredLightTexture2.x`: no role |
//! | 4 | xy | the screen-space displacement to the previous frame's position, in pixels |
//!
//! A program need not write a lane the way its family usually does (one effect stores target 1 as `v / 256`, a program
//! that writes no emission stores the constant 0 there), so what a lane holds is read per program, as the facts of
//! [`GBufferWrites`].
use std::collections::{BTreeMap, BTreeSet};

use iridia::output_stores::OutputStores;
use naga::{Expression, Handle};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GBufferWrites {
    /// Target 1's x, y and z are each stored as `n * 0.5 + 0.5` (a product with a factor 0.5, plus 0.5).
    pub normal_biased: bool,
    /// Target 1's x, y and z are each the constant 0: the pass writes no normal.
    pub normal_zero: bool,
    /// Target 1's x, y and z are each stored as `floor(n * 128 + 128) / 256`, the fractional bits packed into w.
    pub normal_floored: bool,
    /// Target 2's x is stored as a square root, or as the constant 0 (zero in any encoding).
    pub specular_intensity_encoded: bool,
    /// Target 2's y is stored as a square root, or as the constant 0.
    pub specular_exponent_encoded: bool,
    /// Target 2's z is written.
    pub fresnel_written: bool,
    /// Target 3's z is stored as something other than the constant 0.
    pub emission_written: bool,
    /// Target 4's x and y are written.
    pub motion_written: bool,
}

/// The writes of a DXIL container's pixel program, or why they cannot be read: an error where no output's stores are
/// found (the lifted program is not shaped as this reading expects).
pub fn writes(container: &[u8]) -> Result<GBufferWrites, String> {
    let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
    writes_program(&program)
}

/// Reuse a resolve pass's existing lift rather than lifting again for the output-store query.
pub(super) fn writes_program(program: &iridia::Program) -> Result<GBufferWrites, String> {
    let stores = OutputStores::of(program)?;
    // the stored values of a lane of target n
    let lane = |target: u32, lane: u32| -> Vec<Handle<Expression>> { stores.lane(target, lane) };
    let found: usize = (0..5u32)
        .map(|t| (0..4u32).map(|l| lane(t, l).len()).sum::<usize>())
        .sum();
    if found == 0 {
        return Err("no stores into the colour outputs' variables found".into());
    }
    let every = |target: u32, l: u32, test: &dyn Fn(Handle<Expression>) -> bool| {
        let values = lane(target, l);
        !values.is_empty() && values.iter().all(|v| test(*v))
    };
    let emission = lane(3, 2);
    Ok(GBufferWrites {
        normal_biased: (0..3).all(|l| every(1, l, &|v| stores.is_biased(v))),
        normal_floored: (0..3).all(|l| every(1, l, &|v| stores.is_floored_byte(v))),
        normal_zero: (0..3).all(|l| every(1, l, &|v| stores.is_zero(v))),
        specular_intensity_encoded: every(2, 0, &|v| stores.is_sqrt(v) || stores.is_zero(v)),
        specular_exponent_encoded: every(2, 1, &|v| stores.is_sqrt(v) || stores.is_zero(v)),
        fresnel_written: !lane(2, 2).is_empty(),
        emission_written: !emission.is_empty() && emission.iter().any(|v| !stores.is_zero(*v)),
        motion_written: !lane(4, 0).is_empty() && !lane(4, 1).is_empty(),
    })
}

/// The constant-buffer rows (by buffer binding, then row) an output of a program's entry point is computed from: every
/// `cb[row]` the stores into the output's variable reach. For checking what an interpolant depends on.
pub fn constant_rows(
    container: &[u8],
    output: &primase_core::shader::Label,
) -> Result<BTreeMap<u32, BTreeSet<u32>>, String> {
    let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
    OutputStores::of(&program)?.constant_rows(output)
}
