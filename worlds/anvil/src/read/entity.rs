//! An Entity resource's body read in order from its first byte to its last: its matrix, its components (each stepped
//! over by its type's length rule, a Visual read through to its mesh instances), its scale and bounds, and its tail.
//! What the fields mean beyond that grammar is not read; a component of a type without a length rule is refused,
//! naming it (the reader can't step over it without searching).
//!
//! The body (little-endian): `01 03`; 16 f32 (a column-major matrix: on every Entity measured its last row is 0 0 0 1
//! and its upper 3x3 has determinant 1); a u32 count and that many components, each `04`, a u64, a u32 type and a
//! body by the type's rule ([`Component`]); 43 bytes; a u32; an f32 (the uniform scale); a bounding volume (a u64, its
//! type 0x4AEC3476, 6 f32 min and max, a u32); an EntityDescriptor (a u64, 0x60121A9E, 13 bytes); a DataLayerFilter
//! (a u64, 0xDB1D406E, a u32 count k, k actions each a u64, 0xE31593E1, `00`, a DataLayer ClassID and a u32, then a
//! u32); then `03` ending the body, or `00`, a u64, GameStateData 0x7313743E, a u32, and two length-prefixed byte runs
//! ending the body.
//!
//! An EntityGroup's body is an Entity's, then a u32 count c, c references to its members, a u32 0 and a u32 (the
//! Riders Republic notes: docs/world.md §7, from the end on 48,755 of 48,803 USNAT groups; the census measures it
//! read from the start). What a member is to its group is not read.
//!
//! The grammar is from the Riders Republic placement notes (docs/entity-grammar.md, consumed to the last byte on 8 of
//! 8 fixtures and 97% of 5,200 sampled USNAT Entities); the census (worlds/anvil/tests/census.rs) measures it on every
//! Entity of the game. Fixed component sizes and the 125-byte Visual tail are observations on every body measured,
//! not stored lengths.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A record type: equals the CRC-32 of the ASCII string `Entity` (the name a label).
pub const ENTITY: u32 = 0x0984_415E;
/// A record type: the CRC-32 of `EntityGroup`.
pub const ENTITY_GROUP: u32 = 0x3F74_2D26;
/// A Visual component's type.
pub const VISUAL: u32 = 0xEC65_8D29;
/// A component type read with a Visual's grammar (the AC schemas name it StaticDecalComponent: a tool claim; the Riders
/// Republic notes measure the grammar on 614 bodies, 608 of them then read to their last byte).
pub const STATIC_DECAL: u32 = 0x2F3A_7F6D;
/// A RigidBodyComponent's type: the CRC-32 of its name.
pub const RIGID_BODY: u32 = 0xC177_D702;
/// Where in a RigidBodyComponent's body (after its type) the reference to its shape lies: a reference on every one of
/// Riders Republic's, naming a MeshShape, SphereShape, ReferenceListShape, CapsuleShape or BoxShape (the census; the
/// notes' collision placement, docs/skate-plan.md).
const SHAPE_AT: usize = 748;
/// A bounding volume's type.
pub const BOUNDING_VOLUME: u32 = 0x4AEC_3476;
/// A LODSelectorInstance's type.
pub const LOD_SELECTOR_INSTANCE: u32 = 0x0143_7462;
/// A MeshInstanceData's type.
pub const MESH_INSTANCE_DATA: u32 = 0x536E_963B;
/// An ImpostorInstanceData's type.
pub const IMPOSTOR_INSTANCE_DATA: u32 = 0xB3F0_6BC3;
/// A LightInstance's type.
pub const LIGHT_INSTANCE: u32 = 0x554C_614C;
/// A SplashFXInstanceData's type.
pub const SPLASH_FX_INSTANCE_DATA: u32 = 0xB8B0_8A89;

/// Why an Entity body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Entity body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("Entity {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("Entity component {index}: type {kind:08X} has no length rule")]
    Component { index: usize, kind: u32 },
    #[error("Entity Visual: instance data type {0:08X} isn't read")]
    Instance(u32),
    #[error("Entity reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("Entity body: {0} bytes after its end")]
    Trailing(usize),
}

/// A bounding volume as stored: its id, two corners and a u32.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingVolume {
    pub id: u64,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub word: u32,
}

/// A MeshInstanceData as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshInstance {
    pub id: u64,
    /// Its instance matrices, column-major (none: placed once by its owner).
    pub instances: Vec<[f32; 16]>,
    pub bounds: BoundingVolume,
    /// The 16 bytes after them, as stored.
    pub reserved: [u8; 16],
    /// The CompiledMeshInstance's 17 bytes after its type, as stored.
    pub compiled: [u8; 17],
    /// Its material-info entries: (the owner's id, a source Material, a replacement Material).
    pub material_infos: Vec<[u64; 3]>,
    /// One bounding volume per instance.
    pub instance_bounds: Vec<BoundingVolume>,
}

/// An ImpostorInstanceData as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ImpostorInstance {
    pub id: u64,
    pub instances: Vec<[f32; 16]>,
    pub bounds: BoundingVolume,
    /// Its 10-byte reference as stored (lead `03 00`, then an Impostor's ClassID).
    pub reference: [u8; 10],
}

/// One of a LODSelectorInstance's five slots.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    None,
    Mesh(MeshInstance),
    Impostor(ImpostorInstance),
}

/// What a Visual instantiates.
#[derive(Clone, Debug, PartialEq)]
pub enum Instance {
    /// A LODSelectorInstance: matrices, bounds, its LODSelector reference as stored and five slots.
    Lod {
        id: u64,
        instances: Vec<[f32; 16]>,
        bounds: BoundingVolume,
        selector: [u8; 10],
        slots: Vec<Slot>,
    },
    /// A MeshInstanceData directly.
    Mesh(MeshInstance),
    /// Kept as stored (a LightInstance's 311 bytes, the tail within; a SplashFXInstanceData's).
    Raw { kind: u32, id: u64, bytes: Vec<u8> },
    /// None: the instance tag `03` (null), as a StaticDecalComponent with no object holds.
    None,
}

/// A Visual component as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Visual {
    pub head: [u8; 4],
    /// What it shows: a LODSelector on most, a Mesh directly on some.
    pub object: Option<Reference>,
    pub instance: Instance,
    /// Its 125-byte tail, as stored (none where the instance data holds it).
    pub tail: Vec<u8>,
}

/// A component: its id, type and body.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub id: u64,
    pub kind: u32,
    pub body: ComponentBody,
}

/// A component's body.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentBody {
    Visual(Visual),
    /// A StaticDecalComponent: a Visual's grammar, its object decal geometry made for this one placement (the Riders
    /// Republic notes: on every CITYSCHOOL wall a LODSelector named for the Entity's own StaticDecalComponent).
    StaticDecal(Visual),
    /// A RigidBodyComponent: its bytes as stored, and the shape the reference at its offset 748 names.
    RigidBody {
        shape: Option<Reference>,
        bytes: Vec<u8>,
    },
    /// A type stepped over by its length rule, its bytes kept as stored.
    Raw(Vec<u8>),
}

/// How an Entity body ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tail {
    /// `03`.
    End,
    /// An inline GameStateData: its id, a u32 and two byte runs.
    GameState {
        id: u64,
        word: u32,
        first: Vec<u8>,
        second: Vec<u8>,
    },
}

/// An Entity body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntity {
    /// Column-major.
    pub matrix: [f32; 16],
    pub components: Vec<Component>,
    /// The 43 bytes after the components.
    pub run: [u8; 43],
    /// The u32 before the scale.
    pub word: u32,
    pub scale: f32,
    pub bounds: BoundingVolume,
    /// The EntityDescriptor's 13 bytes after its header.
    pub descriptor: [u8; 13],
    /// The DataLayerFilter's actions: (the DataLayer's ClassID, the u32 after it).
    pub data_layers: Vec<(u64, u32)>,
    /// The u32 ending the DataLayerFilter.
    pub filter_word: u32,
    pub tail: Tail,
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
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        if n.checked_mul(each.max(1))
            .is_none_or(|bytes| bytes > self.b.len())
        {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
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
    /// Nothing left after the grammar.
    fn end(&self) -> Result<(), Error> {
        match self.b.len().saturating_sub(self.at) {
            0 => Ok(()),
            left => Err(Error::Trailing(left)),
        }
    }
    /// An embedded object's header: a u64 id and its type, which must be `kind`.
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        self.expect(what, u64::from(kind), 4)?;
        Ok(id)
    }
    fn matrix(&mut self) -> Result<[f32; 16], Error> {
        let mut m = [0.0; 16];
        for x in &mut m {
            *x = self.f32("matrix")?;
        }
        Ok(m)
    }
    fn matrices(&mut self) -> Result<Vec<[f32; 16]>, Error> {
        let n = self.count("instance count", 64)?;
        (0..n).map(|_| self.matrix()).collect()
    }
    fn bounds(&mut self) -> Result<BoundingVolume, Error> {
        let id = self.object("bounding volume", BOUNDING_VOLUME)?;
        let mut f = [0.0f32; 6];
        for x in &mut f {
            *x = self.f32("bounding volume")?;
        }
        let [a, b, c, d, e, g] = f;
        Ok(BoundingVolume {
            id,
            min: [a, b, c],
            max: [d, e, g],
            word: self.u32("bounding volume")?,
        })
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        reference::parse(self.array::<{ reference::LENGTH }>("reference")?)
            .map_err(|source| Error::Reference { at, source })
    }
    fn mesh_instance(&mut self, id: u64) -> Result<MeshInstance, Error> {
        let instances = self.matrices()?;
        let bounds = self.bounds()?;
        let reserved = self.array::<16>("MeshInstanceData")?;
        self.expect("CompiledMeshInstance", 0x4368_101B, 4)?;
        let compiled = self.array::<17>("CompiledMeshInstance")?;
        let a = self.count("material-info count", 41)?;
        let mut material_infos = Vec::with_capacity(a);
        for _ in 0..a {
            self.expect("material info", 0, 1)?;
            self.object("material info", 0x995B_FBF5)?;
            self.expect("material info", 2, 1)?;
            let owner = self.u64("material info")?;
            self.expect("material info", 0, 1)?;
            let source = self.u64("material info")?;
            let at = self.at;
            if self.array::<2>("material info")? != [1, 1] {
                return Err(Error::Value {
                    what: "material info replacement tag",
                    at,
                    found: 0,
                    read: "01 01",
                });
            }
            let replacement = self.u64("material info")?;
            material_infos.push([owner, source, replacement]);
        }
        let b = self.count("instance bounds count", 40)?;
        let instance_bounds = (0..b).map(|_| self.bounds()).collect::<Result<_, _>>()?;
        Ok(MeshInstance {
            id,
            instances,
            bounds,
            reserved,
            compiled,
            material_infos,
            instance_bounds,
        })
    }
    /// The Visual's 125-byte tail: a u32, a u32, `03`, 116 bytes.
    fn visual_tail(&mut self) -> Result<Vec<u8>, Error> {
        let start = self.at;
        self.take(8, "Visual tail")?;
        self.expect("Visual tail handle", 3, 1)?;
        self.take(116, "Visual tail")?;
        Ok(self.b.get(start..self.at).unwrap_or_default().to_vec())
    }
    fn visual(&mut self) -> Result<Visual, Error> {
        let head = self.array::<4>("Visual")?;
        let object = self.reference()?;
        let at = self.at;
        match self.u8("Visual instance tag")? {
            0 => {}
            3 => {
                return Ok(Visual {
                    head,
                    object,
                    instance: Instance::None,
                    tail: self.visual_tail()?,
                });
            }
            found => {
                return Err(Error::Value {
                    what: "Visual instance tag",
                    at,
                    found: u64::from(found),
                    read: "0, or 3 (none)",
                });
            }
        }
        let id = self.u64("Visual instance")?;
        let kind = self.u32("Visual instance type")?;
        let (instance, tail) = match kind {
            LOD_SELECTOR_INSTANCE => {
                let instances = self.matrices()?;
                let bounds = self.bounds()?;
                let selector = self.array::<10>("LODSelectorInstance selector")?;
                let mut slots = Vec::with_capacity(5);
                for _ in 0..5 {
                    slots.push(match self.u8("slot tag")? {
                        3 => Slot::None,
                        0 => {
                            let slot_id = self.u64("slot")?;
                            match self.u32("slot type")? {
                                MESH_INSTANCE_DATA => Slot::Mesh(self.mesh_instance(slot_id)?),
                                IMPOSTOR_INSTANCE_DATA => {
                                    let instances = self.matrices()?;
                                    let bounds = self.bounds()?;
                                    Slot::Impostor(ImpostorInstance {
                                        id: slot_id,
                                        instances,
                                        bounds,
                                        reference: self.array::<10>("impostor reference")?,
                                    })
                                }
                                other => return Err(Error::Instance(other)),
                            }
                        }
                        tag => {
                            return Err(Error::Value {
                                what: "slot tag",
                                at: self.at.saturating_sub(1),
                                found: u64::from(tag),
                                read: "3 or 0",
                            });
                        }
                    });
                }
                let instance = Instance::Lod {
                    id,
                    instances,
                    bounds,
                    selector,
                    slots,
                };
                (instance, self.visual_tail()?)
            }
            MESH_INSTANCE_DATA => (Instance::Mesh(self.mesh_instance(id)?), self.visual_tail()?),
            LIGHT_INSTANCE => {
                let bytes = self.take(311, "LightInstance")?.to_vec();
                (Instance::Raw { kind, id, bytes }, Vec::new())
            }
            SPLASH_FX_INSTANCE_DATA => {
                let start = self.at;
                self.matrices()?;
                self.bounds()?;
                self.take(10, "SplashFXInstanceData reference")?;
                let bytes = self.b.get(start..self.at).unwrap_or_default().to_vec();
                (Instance::Raw { kind, id, bytes }, self.visual_tail()?)
            }
            other => return Err(Error::Instance(other)),
        };
        Ok(Visual {
            head,
            object,
            instance,
            tail,
        })
    }
    /// A component body other than a Visual, stepped over by its type's rule; `None` where the type has none.
    fn raw_component(&mut self, kind: u32) -> Result<Option<Vec<u8>>, Error> {
        let start = self.at;
        match kind {
            // RigidBodyComponent: 826 bytes, a handle (03, or 05 and a u64), a u32 0, an object, 4 bytes
            0xC177_D702 => {
                self.take(826, "RigidBodyComponent")?;
                match self.u8("RigidBodyComponent handle")? {
                    3 => {}
                    5 => {
                        self.u64("RigidBodyComponent handle")?;
                    }
                    found => {
                        return Err(Error::Value {
                            what: "RigidBodyComponent handle",
                            at: self.at.saturating_sub(1),
                            found: u64::from(found),
                            read: "3 or 5",
                        });
                    }
                }
                self.expect("RigidBodyComponent", 0, 4)?;
                self.object("RigidBodyComponent", 0x18D5_28B6)?;
                self.take(4, "RigidBodyComponent")?;
            }
            // AmbientEffectGeneratorComponent: 4 bytes, a count c, 87 c bytes, 4 bytes
            0x713F_0C7D => {
                self.take(4, "component")?;
                let c = self.count("component count", 87)?;
                self.take(c.saturating_mul(87), "component")?;
                self.take(4, "component")?;
            }
            // MaterialOverrider: 4 bytes, a count d, d OverrideDefinitions, 10 bytes
            0x78A1_3BF7 => {
                self.take(4, "MaterialOverrider")?;
                let d = self.count("MaterialOverrider count", 31)?;
                for _ in 0..d {
                    self.object("OverrideDefinition", 0x49FC_D7BF)?;
                    self.take(19, "OverrideDefinition")?;
                }
                self.take(10, "MaterialOverrider")?;
            }
            // GuidanceSystem: 4 bytes, N GuidanceObjects (an object, 29 bytes), 12 bytes, a counted run of 2-byte
            // items, an EdgeFilter (40), a Partitioner (46 + 2k, its u16 p at +12 and u32 k at +14), p + 1 PartNodes
            // (an object, 12 bytes), a byte
            0x55AF_1C3E => {
                self.take(4, "GuidanceSystem")?;
                let n = self.count("GuidanceSystem object count", 41)?;
                for _ in 0..n {
                    self.object("GuidanceObject", 0x344F_A659)?;
                    self.take(29, "GuidanceObject")?;
                }
                self.take(12, "GuidanceSystem")?;
                let m = self.count("GuidanceSystem run count", 2)?;
                self.take(m.saturating_mul(2), "GuidanceSystem run")?;
                self.take(40, "EdgeFilter")?;
                self.take(12, "Partitioner")?;
                let p = usize::from(u16::from_le_bytes(self.array::<2>("Partitioner")?));
                let k = self.count("Partitioner count", 2)?;
                self.take(k.saturating_mul(2).saturating_add(28), "Partitioner")?;
                for _ in 0..=p {
                    self.object("PartNode", 0x5374_195C)?;
                    self.take(12, "PartNode")?;
                }
                self.take(1, "GuidanceSystem")?;
            }
            // HoudiniDecalInstancerComponent: 212 bytes, an object, R references, S items (00, an object, 3 bytes, a
            // count k, 100 k bytes), 46 bytes
            0x4148_CD56 => {
                self.take(212, "HoudiniDecalInstancerComponent")?;
                self.object("HoudiniDecalInstancerComponent", 0xB43F_ECF1)?;
                let r = self.count("HoudiniDecalInstancerComponent references", 10)?;
                self.take(r.saturating_mul(10), "HoudiniDecalInstancerComponent references")?;
                let items = self.count("HoudiniDecalInstancerComponent items", 20)?;
                for _ in 0..items {
                    self.expect("HoudiniDecalInstancerComponent item", 0, 1)?;
                    self.object("HoudiniDecalInstancerComponent item", 0x1ADB_43B0)?;
                    self.take(3, "HoudiniDecalInstancerComponent item")?;
                    let k = self.count("HoudiniDecalInstancerComponent item count", 100)?;
                    self.take(k.saturating_mul(100), "HoudiniDecalInstancerComponent item")?;
                }
                self.take(46, "HoudiniDecalInstancerComponent")?;
            }
            // StartingGridComponent: 4 bytes, a count n, n 78-byte objects (GameplayPositionWithLocal: the notes'
            // 8 + 78 n on 1,362 of 1,362)
            0x1725_EC54 => {
                self.take(4, "component")?;
                let n = self.count("component count", 78)?;
                self.take(n.saturating_mul(78), "component")?;
            }
            // QuestInteractiveComponent: 89 bytes, a null handle 03, an EventListener's 13 bytes (the notes: 532 of 532; a 120-byte form with
            // another byte there is seen 4 times, without a rule)
            0x733E_6F5A => {
                self.take(89, "component")?;
                if self.u8("component handle")? != 3 {
                    return Ok(None);
                }
                self.take(13, "component")?;
            }
            // OutOfBoundsVisualComponent (the notes: 169 on 716 of 716)
            0x2994_1A30 => {
                self.take(169, "component")?;
            }
            // fixed sizes, observed on every body measured
            0x5139_1C93 => {
                self.take(12, "component")?;
            }
            0x7F0A_4643 => {
                self.take(14, "component")?;
            }
            0x2AA1_79AB => {
                self.take(17, "component")?;
            }
            0xADF1_D5E6 => {
                self.take(450, "component")?;
            }
            0xA8AB_FC79 => {
                self.take(46, "component")?;
            }
            0x064C_1BFA => {
                self.take(24, "component")?;
            }
            0x8167_852B => {
                self.take(426, "component")?;
            }
            0xA703_3693 => {
                self.take(9, "component")?;
            }
            _ => return Ok(None),
        }
        Ok(Some(self.b.get(start..self.at).unwrap_or_default().to_vec()))
    }
}

impl StoredEntity {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredEntity, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let e = entity(&mut c)?;
        c.end()?;
        Ok(e)
    }

    /// The Visual components.
    pub fn visuals(&self) -> impl Iterator<Item = &Visual> {
        self.components.iter().filter_map(|c| match &c.body {
            ComponentBody::Visual(v) => Some(v),
            ComponentBody::StaticDecal(_) | ComponentBody::RigidBody { .. } | ComponentBody::Raw(_) => None,
        })
    }
}

/// An EntityGroup body as stored: an Entity, then its members.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntityGroup {
    pub entity: StoredEntity,
    /// Its member references, in stored order.
    pub members: Vec<Reference>,
    /// The u32 ending the body, as stored.
    pub word: u32,
}

impl StoredEntityGroup {
    /// The body parsed: the Entity grammar, a u32 count c, c references (none of them `03`), a u32 0 and a u32.
    pub fn parse(body: &[u8]) -> Result<StoredEntityGroup, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let entity = entity(&mut c)?;
        let n = c.count("member count", reference::LENGTH)?;
        let mut members = Vec::with_capacity(n);
        for _ in 0..n {
            let at = c.at;
            members.push(c.reference()?.ok_or(Error::Value {
                what: "member reference",
                at,
                found: 3,
                read: "a reference (tag 1)",
            })?);
        }
        c.expect("word after the members", 0, 4)?;
        let word = c.u32("last word")?;
        c.end()?;
        Ok(StoredEntityGroup {
            entity,
            members,
            word,
        })
    }
}

/// An Entity's grammar from its first byte through its tail.
fn entity(c: &mut Cursor<'_>) -> Result<StoredEntity, Error> {
    c.expect("first byte", 1, 1)?;
    c.expect("second byte", 3, 1)?;
    let matrix = c.matrix()?;
    let n = c.count("component count", 13)?;
    let mut components = Vec::with_capacity(n);
    for index in 0..n {
        c.expect("component tag", 4, 1)?;
        let id = c.u64("component")?;
        let kind = c.u32("component type")?;
        let start = c.at;
        let body = match kind {
            VISUAL => ComponentBody::Visual(c.visual()?),
            STATIC_DECAL => ComponentBody::StaticDecal(c.visual()?),
            RIGID_BODY => {
                let bytes = c.raw_component(kind)?.ok_or(Error::Component { index, kind })?;
                let at = start.saturating_add(SHAPE_AT);
                let shape: [u8; reference::LENGTH] = bytes
                    .get(SHAPE_AT..SHAPE_AT.saturating_add(reference::LENGTH))
                    .and_then(|r| r.try_into().ok())
                    .ok_or(Error::Short {
                        what: "RigidBodyComponent shape",
                        at,
                        have: bytes.len(),
                    })?;
                let shape = reference::parse(shape).map_err(|source| Error::Reference { at, source })?;
                ComponentBody::RigidBody { shape, bytes }
            }
            _ => ComponentBody::Raw(c.raw_component(kind)?.ok_or(Error::Component { index, kind })?),
        };
        components.push(Component { id, kind, body });
    }
    let run = c.array::<43>("run")?;
    let word = c.u32("word before the scale")?;
    let scale = c.f32("scale")?;
    let bounds = c.bounds()?;
    c.object("EntityDescriptor", 0x6012_1A9E)?;
    let descriptor = c.array::<13>("EntityDescriptor")?;
    c.object("DataLayerFilter", 0xDB1D_406E)?;
    let k = c.count("DataLayerAction count", 25)?;
    let mut data_layers = Vec::with_capacity(k);
    for _ in 0..k {
        c.object("DataLayerAction", 0xE315_93E1)?;
        c.expect("DataLayerAction", 0, 1)?;
        data_layers.push((c.u64("DataLayerAction")?, c.u32("DataLayerAction")?));
    }
    let filter_word = c.u32("DataLayerFilter")?;
    let tail = match c.u8("tail")? {
        3 => Tail::End,
        0 => {
            let id = c.object("GameStateData", 0x7313_743E)?;
            let word = c.u32("GameStateData")?;
            let l1 = c.count("GameStateData first run", 1)?;
            let first = c.take(l1, "GameStateData first run")?.to_vec();
            let l2 = c.count("GameStateData second run", 1)?;
            let second = c.take(l2, "GameStateData second run")?.to_vec();
            Tail::GameState {
                id,
                word,
                first,
                second,
            }
        }
        found => {
            return Err(Error::Value {
                what: "tail",
                at: c.at.saturating_sub(1),
                found: u64::from(found),
                read: "3 or 0",
            });
        }
    };
    Ok(StoredEntity {
        matrix,
        components,
        run,
        word,
        scale,
        bounds,
        descriptor,
        data_layers,
        filter_word,
        tail,
    })
}
