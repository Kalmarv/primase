//! The grass library: a GrassPatternLibrary's models (one settings record each) and its groups of rules. What it
//! stores is read as stored; what the engine does with it (where plants grow and how many) is not stored in any forge
//! resource: the engine makes the instances at run time (docs/grass.md §0, §5).
//!
//! The body (docs/grass.md §2.2, docs/vegetation.md §3.1; consumed to the last byte on the one library of Riders
//! Republic, 30,247 bytes): `01`, a u32 model count n, `00`, then n times a GrassModel's settings object (a u64, 0xDDB820C0
//! and a 60-byte body) and a BoundingVolume object (a u64, 0x4AEC3476, six f32 and 24 bytes); a u32 0; the groups, each a
//! GrassPattern object (a u64, 0xBDD96F63, a u32 rule count r, `00`) and r GrassPatternElements (a u64, 0xA6FC073D and 82
//! bytes); then 443 bytes: a 256-byte table (the group of a material, `FF` for none) and 187 bytes.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// A record type: the CRC-32 of `GrassPatternLibrary`.
pub const GRASS_PATTERN_LIBRARY: u32 = 0x463C_C1B8;
/// A GrassModel's type, a GrassPattern's, a GrassPatternElement's and a BoundingVolume's.
const GRASS_MODEL: u32 = 0xDDB8_20C0;
const GRASS_PATTERN: u32 = 0xBDD9_6F63;
const GRASS_PATTERN_ELEMENT: u32 = 0xA6FC_073D;
const BOUNDING_VOLUME: u32 = 0x4AEC_3476;

/// Why a library body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("grass library: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("grass library {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
}

/// Little-endian fields read in order, each bounded.
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
        self.take(N, what)?.try_into().map_err(|_| Error::Short {
            what,
            at,
            have: self.b.len(),
        })
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        let [b] = self.array::<1>(what)?;
        Ok(b)
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }
    fn expect(&mut self, what: &'static str, want: u64, width: usize) -> Result<(), Error> {
        let at = self.at;
        let found = match width {
            1 => u64::from(self.u8(what)?),
            4 => u64::from(self.u32(what)?),
            _ => self.u64(what)?,
        };
        if found == want {
            Ok(())
        } else {
            Err(Error::Value {
                what,
                at,
                found,
                read: "the grammar's value",
            })
        }
    }
    /// An object's header: its id, whose type must be `kind`.
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        self.expect(what, u64::from(kind), 4)?;
        Ok(id)
    }
    /// A count of items of `each` bytes the rest of the body can hold.
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(each.max(1)).is_none_or(|bytes| bytes > left) {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
    }
}

/// A GrassModel's settings, as the registry's class stores them (docs/grass.md §2.2): the six f32, the bytes and the
/// enum, the imposter shader levels, and the 23 bytes after them that the class list doesn't name.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassModelSettings {
    pub z_offset: f32,
    pub height_scale_for_occlusion: f32,
    pub terrain_normal_follower_ratio: f32,
    pub root_height: f32,
    pub softness: f32,
    pub normal_bend_factor: f32,
    pub visible_virtual_lod_count: u8,
    pub texture_atlas_element_count: u8,
    /// `RaycastTranslucency` (the enum `GrassModel::RaycastTranslucency`).
    pub raycast_translucency: u32,
    /// `ImposterStartShaderModelLodIndex` and the two after it: three bytes.
    pub imposter_shader_start_model_lod_index: [u8; 3],
    pub imposter_lookat_ratio: f32,
    /// The 23 bytes that follow: a LOD table of 5 bytes (`00 01 02 03 03`), four f32 and 2 bytes. The four f32 are the
    /// model's LOD switch distances in metres by the notes' reading by value: (0, 10, 20, 40) on 26 models, (0, 5, 10,
    /// 20) on 16 and (0, 20, 40, 80) on 4 (counted over the 46 models).
    pub unnamed: [u8; 23],
}

impl GrassModelSettings {
    /// The f32 at `at` of the 23 unnamed bytes, 0 where it lies outside them.
    fn unnamed_f32(&self, at: usize) -> f32 {
        self.unnamed
            .get(at..at.saturating_add(4))
            .and_then(|w| <[u8; 4]>::try_from(w).ok())
            .map_or(0.0, f32::from_le_bytes)
    }
    /// The four f32 after the LOD table in the unnamed bytes: the model's LOD switch distances in metres, by the
    /// notes' reading by value.
    pub fn switch_distances(&self) -> [f32; 4] {
        [5, 9, 13, 17].map(|at| self.unnamed_f32(at))
    }
}

/// One model of the library: its settings and its bounding volume.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassModel {
    pub id: u64,
    pub settings: GrassModelSettings,
    /// The BoundingVolume object's id, its six f32 (min and max) and the 24 bytes after them as stored.
    pub bounds_id: u64,
    pub bounds: [f32; 6],
    pub bounds_rest: [u8; 24],
}

/// A rule: one model that may grow where its group is picked (a GrassPatternElement).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassRule {
    pub id: u64,
    /// `GrassModelIndex`: the position in the setup's ManagedEntities (and the library's models). 0 to 50 occur;
    /// 46 to 50 name no model.
    pub model: u16,
    pub tiling_log_count: u8,
    pub density: f32,
    pub tilt_factor: f32,
    pub scaling_enabled: bool,
    pub scale_a: f32,
    pub scale_b: f32,
    pub height_scaling_enabled: bool,
    pub height_scale_a: f32,
    pub height_scale_b: f32,
    /// The field `63D19E01` (5.0 on 145 rules, 0.0 on 66, 2.5 on others): not named.
    pub unnamed_f32: f32,
    /// The 49 bytes after it as stored (a byte 100, a u16, ten f32 of 1.0 and six bytes).
    pub unnamed: [u8; 49],
}

/// A group: the rules that apply where a ground sample picks it (a GrassPattern).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassGroup {
    pub id: u64,
    pub rules: Vec<StoredGrassRule>,
}

/// A GrassPatternLibrary body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassLibrary {
    pub models: Vec<StoredGrassModel>,
    pub groups: Vec<StoredGrassGroup>,
    /// The 256-byte table that follows the groups: the group of a material (`FF`: none; all but entry 255 on Riders
    /// Republic's, whose entry 255 is 184).
    pub table: [u8; 256],
    /// The 187 bytes after the table, as stored (zeros).
    pub rest: Vec<u8>,
}

impl StoredGrassLibrary {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredGrassLibrary, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.expect("first byte", 1, 1)?;
        let n = c.count("model count", 132)?;
        c.expect("model array", 0, 1)?;
        let mut models = Vec::with_capacity(n);
        for _ in 0..n {
            let id = c.object("GrassModel", GRASS_MODEL)?;
            let settings = {
                let f = |c: &mut Cursor<'_>| c.f32("GrassModel");
                GrassModelSettings {
                    z_offset: f(&mut c)?,
                    height_scale_for_occlusion: f(&mut c)?,
                    terrain_normal_follower_ratio: f(&mut c)?,
                    root_height: f(&mut c)?,
                    softness: f(&mut c)?,
                    normal_bend_factor: f(&mut c)?,
                    visible_virtual_lod_count: c.u8("GrassModel")?,
                    texture_atlas_element_count: c.u8("GrassModel")?,
                    raycast_translucency: c.u32("GrassModel")?,
                    imposter_shader_start_model_lod_index: c.array("GrassModel")?,
                    imposter_lookat_ratio: c.f32("GrassModel")?,
                    unnamed: c.array("GrassModel")?,
                }
            };
            let bounds_id = c.object("GrassModel bounds", BOUNDING_VOLUME)?;
            let mut bounds = [0.0f32; 6];
            for v in &mut bounds {
                *v = c.f32("GrassModel bounds")?;
            }
            let bounds_rest = c.array("GrassModel bounds")?;
            models.push(StoredGrassModel {
                id,
                settings,
                bounds_id,
                bounds,
                bounds_rest,
            });
        }
        c.expect("word after the models", 0, 4)?;
        let mut groups = Vec::new();
        // the groups run on while an object of type GrassPattern follows; the 443 bytes after them begin with a table
        // of 256 bytes
        while c.b.get(c.at.saturating_add(8)..c.at.saturating_add(12)) == Some(&GRASS_PATTERN.to_le_bytes()) {
            let id = c.object("GrassPattern", GRASS_PATTERN)?;
            let r = c.count("rule count", 94)?;
            c.expect("GrassPattern byte", 0, 1)?;
            let mut rules = Vec::with_capacity(r);
            for _ in 0..r {
                let id = c.object("GrassPatternElement", GRASS_PATTERN_ELEMENT)?;
                rules.push(StoredGrassRule {
                    id,
                    model: c.u16("GrassPatternElement")?,
                    tiling_log_count: c.u8("GrassPatternElement")?,
                    density: c.f32("GrassPatternElement")?,
                    tilt_factor: c.f32("GrassPatternElement")?,
                    scaling_enabled: c.u8("GrassPatternElement")? != 0,
                    scale_a: c.f32("GrassPatternElement")?,
                    scale_b: c.f32("GrassPatternElement")?,
                    height_scaling_enabled: c.u8("GrassPatternElement")? != 0,
                    height_scale_a: c.f32("GrassPatternElement")?,
                    height_scale_b: c.f32("GrassPatternElement")?,
                    unnamed_f32: c.f32("GrassPatternElement")?,
                    unnamed: c.array("GrassPatternElement")?,
                });
            }
            groups.push(StoredGrassGroup { id, rules });
        }
        let table = c.array::<256>("table")?;
        let rest = c
            .take(c.b.len().saturating_sub(c.at), "the bytes after the table")?
            .to_vec();
        Ok(StoredGrassLibrary {
            models,
            groups,
            table,
            rest,
        })
    }
}
