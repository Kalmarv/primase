//! A CompiledMip resource's body: one mip level's image bytes and what names them, read as stored. Nothing inside the
//! image is interpreted (its codec, pixel format and dimensions are the parent TextureMap's and not read here).
//!
//! The body (little-endian, after the resource's class id and extension): a flag byte (1), the image's byte length
//! (u32), the image, a zero byte, the parent TextureMap's class id (u64) and the mip level (u32), ending the body.
//!
//! Established on four CompiledMip resources (Riders Republic build 25394987: three level 0 mips, one level 1, images
//! of 524,288 and 2,097,152 bytes; games/riders-republic docs/recon/compiled-mip-reader-request.md, f9925f0). Another
//! flag or zero byte, or a body not ending exactly after the level, is refused, naming it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// CompiledMip's type id (CRC-32 of its type name).
pub const COMPILED_MIP: u32 = 0x1D4B_87A3;
/// The bytes around the image: the flag and length before it, the zero byte, parent and level after.
const ENVELOPE: usize = 5 + 13;

/// Why a CompiledMip body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("CompiledMip body: {have} bytes, shorter than its {what}")]
    Short { what: &'static str, have: usize },
    #[error("CompiledMip {what}: {found}, only {read} is read (another value isn't established)")]
    Value { what: &'static str, found: u8, read: u8 },
    #[error("CompiledMip body: {have} bytes, but its image of {image} bytes closes it at {closes}")]
    Closure { image: u32, closes: usize, have: usize },
}

/// A CompiledMip body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredMip {
    /// The image bytes, uninterpreted.
    pub image: Vec<u8>,
    /// The class id of the TextureMap this level belongs to.
    pub parent: u64,
    /// The mip level (0 the largest, as the names' `Mip0`, `Mip1` have it).
    pub level: u32,
}

impl StoredMip {
    /// Where the image starts in the body.
    pub const IMAGE_AT: usize = 5;

    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredMip, Error> {
        let have = body.len();
        let [flag, a, b, c, d, rest @ ..] = body else {
            return Err(Error::Short {
                what: "flag and image length",
                have,
            });
        };
        if *flag != 1 {
            return Err(Error::Value {
                what: "flag",
                found: *flag,
                read: 1,
            });
        }
        let length = u32::from_le_bytes([*a, *b, *c, *d]);
        let image = usize::try_from(length).unwrap_or(usize::MAX);
        let closes = image.saturating_add(ENVELOPE);
        if closes != have {
            return Err(Error::Closure {
                image: length,
                closes,
                have,
            });
        }
        let (image, after) = rest
            .split_at_checked(image)
            .ok_or(Error::Short { what: "image", have })?;
        let [zero, after @ ..] = after else {
            return Err(Error::Short {
                what: "zero byte",
                have,
            });
        };
        if *zero != 0 {
            return Err(Error::Value {
                what: "byte after the image",
                found: *zero,
                read: 0,
            });
        }
        let (parent, level) = after
            .split_first_chunk::<8>()
            .ok_or(Error::Short { what: "parent", have })?;
        let level: &[u8; 4] = level
            .try_into()
            .map_err(|_| Error::Short { what: "level", have })?;
        Ok(StoredMip {
            image: image.to_vec(),
            parent: u64::from_le_bytes(*parent),
            level: u32::from_le_bytes(*level),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(flag: u8, image: &[u8], zero: u8, parent: u64, level: u32) -> Vec<u8> {
        let mut b = vec![flag];
        b.extend(u32::try_from(image.len()).expect("small").to_le_bytes());
        b.extend(image);
        b.push(zero);
        b.extend(parent.to_le_bytes());
        b.extend(level.to_le_bytes());
        b
    }

    #[test]
    fn a_body_reads_its_envelope_and_keeps_its_image() {
        let mip = StoredMip::parse(&body(1, &[9, 8, 7], 0, 0x01C7_C843_E75E, 1)).expect("mip");
        assert_eq!(
            mip,
            StoredMip {
                image: vec![9, 8, 7],
                parent: 0x01C7_C843_E75E,
                level: 1
            }
        );
    }

    #[test]
    fn another_flag_or_zero_or_closure_is_refused() {
        assert!(matches!(
            StoredMip::parse(&body(2, &[0], 0, 1, 0)),
            Err(Error::Value { what: "flag", .. })
        ));
        assert!(matches!(
            StoredMip::parse(&body(1, &[0], 5, 1, 0)),
            Err(Error::Value { found: 5, .. })
        ));
        let mut long = body(1, &[0], 0, 1, 0);
        long.push(0);
        assert!(matches!(StoredMip::parse(&long), Err(Error::Closure { .. })));
        assert!(matches!(
            StoredMip::parse(long.get(..10).expect("ten")),
            Err(Error::Closure { .. })
        ));
        assert!(matches!(StoredMip::parse(&[1, 0]), Err(Error::Short { .. })));
    }
}
