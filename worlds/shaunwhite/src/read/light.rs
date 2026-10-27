//! PC lights: AnvilToolkit 1.3.7 AC2 field order with the game's Light/SpotLight differences established on source
//! bytes. All objects are read to their exact end. SkyLight is this game's class, verified against the executable's
//! property descriptors and serializer: its 106-byte prefix and inline 94-byte SunLight are retained separately.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::components::{LightFlare, LightShape, LightSource, SkySettings};
use primase_foundations::bytes::{Cursor, Endian, Field, FieldError};

const LE: Endian = Endian::Little;
pub const OMNI_LIGHT: u32 = 0x3447_80D6;
pub const SPOT_LIGHT: u32 = 0x8032_0FB8;
pub const DIRECTIONAL_LIGHT: u32 = 0x7E15_FD50;
pub const SUN_LIGHT: u32 = 0x5EDC_3E04;
pub const SKY_LIGHT: u32 = 0x77EE_EBB4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("light {field}: {why}")]
    Invalid { field: &'static str, why: String },
}
fn invalid(field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        why: why.into(),
    }
}
fn word(c: &mut Cursor<'_>, name: &'static str) -> Result<u32, Error> {
    Ok(c.u32(LE).field(name)?)
}
fn flag(c: &mut Cursor<'_>, name: &'static str) -> Result<bool, Error> {
    match c.u8().field(name)? {
        0 => Ok(false),
        1 => Ok(true),
        n => Err(invalid(name, format!("unsupported bool {n}"))),
    }
}
fn expected(c: &mut Cursor<'_>, name: &'static str, value: u32) -> Result<(), Error> {
    let got = word(c, name)?;
    if got == value {
        Ok(())
    } else {
        Err(invalid(name, format!("{got:#010x}, expected {value:#010x}")))
    }
}
fn float(c: &mut Cursor<'_>, name: &'static str) -> Result<f32, Error> {
    let n = c.f32(LE).field(name)?;
    if n.is_finite() {
        Ok(n)
    } else {
        Err(invalid(name, "non-finite"))
    }
}
fn floats<const N: usize>(c: &mut Cursor<'_>, name: &'static str) -> Result<[f32; N], Error> {
    let mut out = [0.; N];
    for x in &mut out {
        *x = float(c, name)?;
    }
    Ok(out)
}
fn finish(c: &Cursor<'_>) -> Result<(), Error> {
    if c.remaining() == 0 {
        Ok(())
    } else {
        Err(invalid("object end", format!("{} trailing bytes", c.remaining())))
    }
}
pub fn is_light(class: u32) -> bool {
    matches!(class, OMNI_LIGHT | SPOT_LIGHT | DIRECTIONAL_LIGHT | SUN_LIGHT)
}

/// A complete top-level light object's body; no header envelope, intensity conversion or placement.
pub fn read_light(bytes: &[u8]) -> Result<LightSource, Error> {
    let mut c = Cursor::new(bytes);
    let light = light(&mut c)?;
    finish(&c)?;
    Ok(light)
}
fn light(c: &mut Cursor<'_>) -> Result<LightSource, Error> {
    flag(c, "managed")?;
    let id = word(c, "id")?;
    let class = word(c, "class")?;
    if !is_light(class) {
        return Err(invalid("class", format!("unsupported {class:#010x}")));
    }
    let category = word(c, "category")?;
    let generated = flag(c, "generated")?;
    let colour_id = word(c, "colour id")?;
    expected(c, "Color class", 0xA797_67ED)?;
    let colour: [f32; 4] = floats(c, "colour")?;
    if colour.iter().any(|v| *v < 0.) {
        return Err(invalid("colour", "negative"));
    }
    let mut flags = [false; 5];
    for v in &mut flags {
        *v = flag(c, "light flags")?;
    }
    let intensity = float(c, "intensity")?;
    if intensity < 0. {
        return Err(invalid("intensity", "negative"));
    }
    let flare_id = word(c, "flare id")?;
    expected(c, "LightFlare class", 0xAA8F_96B6)?;
    let flare = LightFlare {
        id: flare_id,
        enabled: flag(c, "flare enabled")?,
        texture: word(c, "flare texture")?,
        values: floats(c, "flare values")?,
        scale: flag(c, "flare scale")?,
        fade: flag(c, "flare fade")?,
        offset: float(c, "flare offset")?,
        blend: word(c, "flare blend")?,
    };
    let shadow_resolution = word(c, "shadow resolution")?;
    let shape = match class {
        OMNI_LIGHT => LightShape::Point {
            near: float(c, "near")?,
            far: float(c, "far")?,
        },
        SPOT_LIGHT => LightShape::Spot {
            inner: float(c, "LittleAlpha")?,
            outer: float(c, "BigAlpha")?,
            cookie: word(c, "cookie")?,
            unknown: word(c, "spot unknown")?,
            near: float(c, "near")?,
            far: float(c, "far")?,
        },
        _ => LightShape::Directional,
    };
    match shape {
        LightShape::Point { near, far } | LightShape::Spot { near, far, .. }
            if !(near >= 0. && far > near) =>
        {
            return Err(invalid("range", "need 0 <= near < far"));
        }
        LightShape::Spot { inner, outer, .. }
            if !(0. <= inner && inner <= outer && outer <= std::f32::consts::FRAC_PI_2) =>
        {
            return Err(invalid(
                "spot angles",
                "need 0 <= LittleAlpha <= BigAlpha <= pi/2 radians",
            ));
        }
        _ => {}
    }
    Ok(LightSource {
        id,
        class,
        category,
        generated,
        colour_id,
        colour,
        flags,
        intensity,
        flare,
        shadow_resolution,
        shape,
    })
}

/// A complete 200-byte SkyLight body, with its inline SunLight. Fields without established meanings stay unlabelled.
pub fn read_sky(bytes: &[u8]) -> Result<(SkySettings, LightSource), Error> {
    let mut c = Cursor::new(bytes);
    flag(&mut c, "sky managed")?;
    let id = word(&mut c, "sky id")?;
    expected(&mut c, "SkyLight class", SKY_LIGHT)?;
    let direction: [f32; 3] = floats(&mut c, "sun vector")?;
    if direction.iter().all(|v| *v == 0.) {
        return Err(invalid("sun vector", "zero"));
    }
    let sun_colour_id = word(&mut c, "sun IColor id")?;
    expected(&mut c, "sun IColor class", 0xF820_6AF7)?;
    let sun_colour = word(&mut c, "sun IColor")?;
    let sun_intensity = float(&mut c, "sun intensity")?;
    let unknown_scalars = floats(&mut c, "sky unlabelled scalars")?;
    let use_character_sun = flag(&mut c, "use character sun")?;
    let character_colour_id = word(&mut c, "character IColor id")?;
    expected(&mut c, "character IColor class", 0xF820_6AF7)?;
    let character_colour = word(&mut c, "character IColor")?;
    let character_intensity = float(&mut c, "character intensity")?;
    let unknown_vector = floats(&mut c, "sky unlabelled vector")?;
    let ambient_colour = floats(&mut c, "AmbientColor")?;
    let unknown_colour = floats(&mut c, "sky colour 0xea0e031c")?;
    let sun = light(&mut c)?;
    if sun.class != SUN_LIGHT || sun.intensity != sun_intensity {
        return Err(invalid(
            "inline sun",
            "directional light and duplicated intensity disagree",
        ));
    }
    finish(&c)?;
    Ok((
        SkySettings {
            id,
            direction,
            sun_colour_id,
            sun_colour,
            sun_intensity,
            unknown_scalars,
            use_character_sun,
            character_colour_id,
            character_colour,
            character_intensity,
            unknown_vector,
            ambient_colour,
            unknown_colour,
        },
        sun,
    ))
}
