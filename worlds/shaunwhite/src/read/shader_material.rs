//! Fixed-size generic-material fields used by the native shader key setters. The base bitfields serialize
//! as separate values: blend@21, bits4/5@25/26, eight-bit reference@27, display mode@28, bits16..27@32..43,
//! bits29/30@44/45. Runtime bit28 is not serialized. These are not the shader define bit positions.
use super::material::{Error, Material};
use crate::components::MaterialShader;
use primase_bytes::{Endian, View};
const LE: Endian = Endian::Little;

/// Read source controls without choosing a pass, frame, vertex layout or shader permutation.
pub fn read(body: &[u8]) -> Result<MaterialShader, Error> {
    let m = Material::parse(body)?;
    let v = View::new(body);
    let flag = |at: usize| -> Result<bool, Error> {
        match body.get(at).copied() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            x => Err(Error::Invalid {
                what: "shader material bool",
                value: format!("at {at}: {x:?}"),
            }),
        }
    };
    let word = |at: usize| {
        v.u32(at, LE).map_err(|e| Error::Invalid {
            what: "shader material word",
            value: e.to_string(),
        })
    };
    let blend = word(21)?;
    let display = word(28)?;
    if blend > 15 || display > 3 {
        return Err(Error::Invalid {
            what: "base material flag enum",
            value: format!("blend {blend}, display {display}"),
        });
    }
    let mut flags = blend
        | u32::from(flag(25)?) << 4
        | u32::from(flag(26)?) << 5
        | u32::from(body[27]) << 6
        | display << 14;
    for bit in 16..=27 {
        flags |= u32::from(flag(32 + bit - 16)?) << bit;
    }
    flags |= u32::from(flag(44)?) << 29 | u32::from(flag(45)?) << 30;
    let mut unsupported = Vec::new();
    for (at, name) in [(310, "video texture"), (320, "multi-texture set")] {
        if flag(at)? {
            unsupported.push(name.to_owned());
        }
    }
    let hide = word(525)?;
    if hide > 2 {
        return Err(Error::Invalid {
            what: "InfluenceHidePixelMode",
            value: hide.to_string(),
        });
    }
    let dynamic_uv = word(496)?;
    if dynamic_uv > 2 {
        return Err(Error::Invalid {
            what: "dynamic UV mode",
            value: dynamic_uv.to_string(),
        });
    }
    // Non-normal alpha-display paths require further material key setters, not a similar normal-alpha record.
    if display != 0 || word(324)? != 0 || flag(319)? {
        unsupported.push("non-normal alpha-display/key fields".to_owned());
    }
    Ok(MaterialShader {
        flags,
        layers: [flag(122)?, flag(123)?],
        half_lambert: flag(553)?,
        glossiness: flag(554)?,
        layer3_no_specular: flag(495)?,
        rim_alpha: flag(92)?,
        hide,
        dynamic_uv,
        texture_slots: m.slots.iter().map(|s| s.name.to_owned()).collect(),
        unsupported,
    })
}
