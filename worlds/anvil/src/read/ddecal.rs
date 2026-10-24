//! Deferred decals: a HoudiniDecalInstancerComponent's records (each a projection box in the world) and the
//! DDecalModels they draw. Little-endian; a reference is [`crate::read::reference`]'s.
//!
//! The component body (docs/entity-grammar.md §9.2; its length rule in [`crate::read::entity`]): 212 bytes; an object
//! `0xB43FECF1` (a StaticDDecalLibrary); a count R and R references (DDecalModels); a count S and S items, each `00`,
//! an object `0x1ADB43B0` (DDecalStaticInstancers), 3 bytes (the first the item's model, an index into the R
//! references: less than R on 34,477 of 34,477 items), a count k and k 100-byte records; 46 bytes. A record: a u64 0,
//! a u32 `0x295F035C`, the box's first half-axis (3 f32) at 12, 4 bytes, its second half-axis at 28, 3 f32 at 40 (the
//! first its fade), 4 bytes at 52, the window's offset (f32) at 56, the box's centre (3 f32, world: the Entity's own
//! matrix not applied, §9.3) at 60, the window's width (f32) at 72, the box's minimum at 76 and maximum at 88. The
//! window is the U range `[offset, offset + width]` the decal takes of its texture's length, and the fade `f` scales
//! its opacity by `1 - f`: the engine's static decal instancer packs them into the GPU record as read
//! (material-spec §23.8: `lo16 = trunc((offset + width / 2) 65535)`, `hi16 = trunc(width / 2 x 65535)`,
//! `w13 = (1 - f) m`).
//!
//! A DDecalModel body (115 bytes on all 70, entity-grammar §9.3; its fields named by the engine's registry,
//! material-spec §23.8): `01`, a byte, four references (its Diffuse, Normal, Height and Mask1 maps), 20 flag bytes, 53
//! bytes of parameters (kept as stored), among them its opacity `m` (f32 `0x14A1C428`) at 72.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_bytes::{Cursor, Field};

use crate::read::fields::{BodyError, body_error, count, end, expect, object, reference};
use crate::read::reference::{self, Reference};

/// A component type: HoudiniDecalInstancerComponent.
pub const HOUDINI_DECAL_INSTANCER: u32 = 0x4148_CD56;
/// A component type: StaticDDecalComponent, which HoudiniDecalInstancerComponent derives from without storing a field
/// more (the registry's class list): its body is the same. On Riders Republic's two (one graffiti rock's, 278 bytes)
/// its library holds no model and no item: they draw nothing.
pub const STATIC_DDECAL: u32 = 0x5B41_3085;
/// A record type: DDecalModel.
pub const DDECAL_MODEL: u32 = 0x5056_3D58;

/// Why a deferred decal body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("deferred decal {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

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
    /// Its window along the texture's length: its offset (at 56) and width (at 72).
    pub fn window(&self) -> [f32; 2] {
        [self.f32_at(56), self.f32_at(72)]
    }
    /// Its fade (the f32 at 40): its opacity is `1 - fade` of its model's.
    pub fn fade(&self) -> f32 {
        self.f32_at(40)
    }
    /// The box's bounds: its minimum (at 76) and maximum (at 88).
    pub fn bounds(&self) -> [[f32; 3]; 2] {
        [self.vec3_at(76), self.vec3_at(88)]
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

impl StoredDecalModel {
    /// Its opacity `m`: the f32 at 72 of the body (10 into its parameters).
    pub fn opacity(&self) -> f32 {
        self.parameters
            .get(10..14)
            .and_then(|w| w.try_into().ok())
            .map_or(0.0, f32::from_le_bytes)
    }
}

impl StoredDeferredDecals {
    /// The component body parsed (see the module documentation).
    pub fn parse(body: &[u8]) -> Result<StoredDeferredDecals, Error> {
        let mut c = Cursor::new(body);
        let head = c.take(212).field("head")?.to_vec();
        let library = object(&mut c, "StaticDDecalLibrary", 0xB43F_ECF1)?;
        let r = count(&mut c, "models", reference::LENGTH)?;
        let mut models = Vec::with_capacity(r);
        for _ in 0..r {
            models.push(reference(&mut c)?);
        }
        let s = count(&mut c, "items", 20)?;
        let mut items = Vec::with_capacity(s);
        for _ in 0..s {
            expect(&mut c, "item tag", 0, 1)?;
            let id = object(&mut c, "DDecalStaticInstancers", 0x1ADB_43B0)?;
            let bytes = c.array::<3>().field("item")?;
            let k = count(&mut c, "records", 100)?;
            let mut decals = Vec::with_capacity(k);
            for _ in 0..k {
                decals.push(StoredDecal(c.array::<100>().field("record")?));
            }
            items.push(StoredDecalItem { id, bytes, decals });
        }
        let tail = c.take(46).field("tail")?.to_vec();
        end(&c)?;
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
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        let byte = c.u8().field("byte")?;
        let textures = [
            reference(&mut c)?,
            reference(&mut c)?,
            reference(&mut c)?,
            reference(&mut c)?,
        ];
        let flags = c.array::<20>().field("flags")?;
        let parameters = c.take(53).field("parameters")?.to_vec();
        end(&c)?;
        Ok(StoredDecalModel {
            byte,
            textures,
            flags,
            parameters,
        })
    }
}
