//! Deferred decals: a HoudiniDecalInstancerComponent's records (each a projection box in the world) and the
//! DDecalModels they draw. Little-endian; a reference is [`crate::read::reference`]'s.
//!
//! The component body (docs/entity-grammar.md §9.2; its length rule in [`crate::read::entity`]): 212 bytes; an object
//! `0xB43FECF1` (a StaticDDecalLibrary); a count R and R references (DDecalModels); a count S and S items, each `00`,
//! an object `0x1ADB43B0` (DDecalStaticInstancers), 3 bytes (the first the item's model, an index into the R
//! references: less than R on 34,477 of 34,477 items), a count k and k 100-byte records; 46 bytes. A record: a u64 0,
//! a u32 `0x295F035C`, the box's first half-axis (3 f32) at 12, 4 bytes, its second half-axis at 28, 3 f32 at 40,
//! 4 bytes at 52, an f32 at 56, the box's centre (3 f32, world: the Entity's own matrix not applied, §9.3) at 60, an
//! f32 at 72, the box's minimum at 76 and maximum at 88. The two f32 at 56 and 72 are the record's atlas window along
//! the texture's length (their meaning a reading: material-spec §23.5, entity-grammar §9.3).
//!
//! A DDecalModel body (115 bytes on all 69, entity-grammar §9.3): `01`, a byte, four references (its Diffuse, Normal,
//! Height and Mask1 maps), 20 flag bytes, 53 bytes of parameters (kept as stored).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A component type: HoudiniDecalInstancerComponent.
pub const HOUDINI_DECAL_INSTANCER: u32 = 0x4148_CD56;
/// A record type: DDecalModel.
pub const DDECAL_MODEL: u32 = 0x5056_3D58;

/// Why a deferred decal body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("deferred decal body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("deferred decal {what}: {found:#x} at {at}, not the grammar's value")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
    },
    #[error("deferred decal reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("deferred decal body: {0} bytes after its end")]
    Trailing(usize),
}

/// One 100-byte record, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredDecal(pub [u8; 100]);

impl StoredDecal {
    fn f32_at(&self, at: usize) -> f32 {
        self.0
            .get(at..at.saturating_add(4))
            .and_then(|w| w.try_into().ok())
            .map_or(0.0, f32::from_le_bytes)
    }
    fn vec3_at(&self, at: usize) -> [f32; 3] {
        [0, 4, 8].map(|k| self.f32_at(at.saturating_add(k)))
    }
    /// The box's first half-axis (at 12).
    pub fn axis_x(&self) -> [f32; 3] {
        self.vec3_at(12)
    }
    /// The box's second half-axis (at 28).
    pub fn axis_y(&self) -> [f32; 3] {
        self.vec3_at(28)
    }
    /// The box's centre, in the world (at 60).
    pub fn centre(&self) -> [f32; 3] {
        self.vec3_at(60)
    }
    /// The two f32 at 56 and 72: the atlas window along the texture's length.
    pub fn window(&self) -> [f32; 2] {
        [self.f32_at(56), self.f32_at(72)]
    }
}

/// One DDecalStaticInstancers item: its object id, its 3 bytes (the first its model's index), its records.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredDecalItem {
    pub id: u64,
    pub bytes: [u8; 3],
    pub decals: Vec<StoredDecal>,
}

impl StoredDecalItem {
    /// Its model: an index into the component's references.
    pub fn model(&self) -> usize {
        usize::from(self.bytes[0])
    }
}

/// A HoudiniDecalInstancerComponent body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredDeferredDecals {
    pub head: Vec<u8>,
    pub library: u64,
    pub models: Vec<Option<Reference>>,
    pub items: Vec<StoredDecalItem>,
    pub tail: Vec<u8>,
}

/// A DDecalModel body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredDecalModel {
    pub byte: u8,
    /// Its Diffuse, Normal, Height and Mask1 maps.
    pub textures: [Option<Reference>; 4],
    pub flags: [u8; 20],
    pub parameters: Vec<u8>,
}

/// Bytes read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&[u8], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(n).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s = self.b.get(self.at..end).ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let at = self.at;
        let s = self.take(N, what)?;
        s.try_into().map_err(|_| Error::Short {
            what,
            at,
            have: s.len(),
        })
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(u8::from_le_bytes(self.array(what)?))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    fn expect(&mut self, what: &'static str, want: u64, found: u64, at: usize) -> Result<(), Error> {
        if want == found {
            Ok(())
        } else {
            Err(Error::Value { what, at, found })
        }
    }
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        let at = self.at;
        let found = self.u32(what)?;
        self.expect(what, u64::from(kind), u64::from(found), at)?;
        Ok(id)
    }
    /// A count of items at least `each` bytes long, bounded by what is left.
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(each).is_none_or(|need| need > left) {
            return Err(Error::Short {
                what,
                at,
                have: self.b.len(),
            });
        }
        Ok(n)
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        let bytes = self.array::<{ reference::LENGTH }>("reference")?;
        reference::parse(bytes).map_err(|source| Error::Reference { at, source })
    }
    fn end(&self) -> Result<(), Error> {
        match self.b.len().saturating_sub(self.at) {
            0 => Ok(()),
            left => Err(Error::Trailing(left)),
        }
    }
}

impl StoredDeferredDecals {
    /// The component body parsed (see the module documentation).
    pub fn parse(body: &[u8]) -> Result<StoredDeferredDecals, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let head = c.take(212, "head")?.to_vec();
        let library = c.object("StaticDDecalLibrary", 0xB43F_ECF1)?;
        let r = c.count("models", reference::LENGTH)?;
        let mut models = Vec::with_capacity(r);
        for _ in 0..r {
            models.push(c.reference()?);
        }
        let s = c.count("items", 20)?;
        let mut items = Vec::with_capacity(s);
        for _ in 0..s {
            let at = c.at;
            let tag = c.u8("item")?;
            c.expect("item tag", 0, u64::from(tag), at)?;
            let id = c.object("DDecalStaticInstancers", 0x1ADB_43B0)?;
            let bytes = c.array::<3>("item")?;
            let k = c.count("records", 100)?;
            let mut decals = Vec::with_capacity(k);
            for _ in 0..k {
                decals.push(StoredDecal(c.array::<100>("record")?));
            }
            items.push(StoredDecalItem { id, bytes, decals });
        }
        let tail = c.take(46, "tail")?.to_vec();
        c.end()?;
        Ok(StoredDeferredDecals {
            head,
            library,
            models,
            items,
            tail,
        })
    }
}

impl StoredDecalModel {
    /// The DDecalModel body parsed (see the module documentation).
    pub fn parse(body: &[u8]) -> Result<StoredDecalModel, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let at = c.at;
        let first = c.u8("first byte")?;
        c.expect("first byte", 1, u64::from(first), at)?;
        let byte = c.u8("byte")?;
        let textures = [c.reference()?, c.reference()?, c.reference()?, c.reference()?];
        let flags = c.array::<20>("flags")?;
        let parameters = c.take(53, "parameters")?.to_vec();
        c.end()?;
        Ok(StoredDecalModel {
            byte,
            textures,
            flags,
            parameters,
        })
    }
}
