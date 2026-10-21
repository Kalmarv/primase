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
    /// Its header's tag: 04, or 00 (the notes: in UI groups and Trigger_Discovery groups; what it means is not
    /// established).
    pub tag: u8,
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
    /// A GameplayContextDisabler's condition node (04, a u64, its type): an operator (0x025AD7F9: a byte, an op
    /// 0, 1 or 2, a count n, n child nodes) or a leaf (0x0FA689EF: 2 bytes, a u64). False where its type is neither
    /// or it nests deeper than the notes saw (2).
    fn condition(&mut self, depth: usize) -> Result<bool, Error> {
        self.expect("condition node", 4, 1)?;
        self.u64("condition node")?;
        match self.u32("condition node type")? {
            0x025A_D7F9 if depth < 2 => {
                self.take(1, "condition operator")?;
                let at = self.at;
                let op = self.u32("condition operator")?;
                if op > 2 {
                    return Err(Error::Value {
                        what: "condition operator",
                        at,
                        found: u64::from(op),
                        read: "0, 1 or 2",
                    });
                }
                let n = self.count("condition operator count", 23)?;
                for _ in 0..n {
                    if !self.condition(depth.saturating_add(1))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            0x0FA6_89EF => {
                self.take(10, "condition leaf")?;
                Ok(true)
            }
            _ => Ok(false),
        }
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
            // UICheckpointController: 51 bytes (the notes: the next component's header exactly there on 18,259 of
            // 18,259)
            0x4CB6_2343 => {
                self.take(51, "UICheckpointController")?;
            }
            // CheckpointComponent: 4 bytes, a count k, k inline objects (00, a u64, a type, its body), 226 bytes; each
            // object's size by its type, observed on every body measured (the notes: 18,085 of 18,395 groups holding
            // it then parse to their end); a type outside the table has no rule
            0x69D5_E169 => {
                self.take(4, "CheckpointComponent")?;
                let k = self.count("CheckpointComponent count", 41)?;
                for _ in 0..k {
                    self.expect("CheckpointComponent object", 0, 1)?;
                    self.u64("CheckpointComponent object")?;
                    let size: usize = match self.u32("CheckpointComponent object type")? {
                        0xB0AE_57CB => 56,
                        0x66D1_487D => 53,
                        0x0AB1_C905 | 0x0F5F_CE90 => 49,
                        0xA117_C2BE => 76,
                        0x1FA3_0270 | 0x4247_DAA2 | 0xC75E_1946 => 41,
                        0xF24B_6C13 => 264,
                        0x065C_746A => 52,
                        0x956F_6AB5 => 93,
                        0x2350_A2E0 => 115,
                        0xB114_3610 => 43,
                        0x561E_06FC => 121,
                        0x74E3_8111 => 73,
                        _ => return Ok(None),
                    };
                    self.take(size.saturating_sub(13), "CheckpointComponent object")?;
                }
                self.take(226, "CheckpointComponent")?;
            }
            // FakeWaterMeshComponent: 4 bytes, a reference (its FakeWaterComponentChunk), a FakeWaterMeshInstance
            // (00, an object, a count k and 64 k bytes), a bounding volume, 10 bytes, the 125-byte Visual tail, 3 vec4,
            // a u32, a vertex count V, a u32, V 8-byte vertices (4 f16), an index count I, a u32, I u16 indices (the
            // notes: 274 + 8 V + 2 I, the one sequential end on 512 of 512)
            0x961F_3B5D => {
                self.take(14, "FakeWaterMeshComponent")?;
                self.expect("FakeWaterMeshInstance", 0, 1)?;
                self.object("FakeWaterMeshInstance", 0xC345_7872)?;
                let k = self.count("FakeWaterMeshInstance count", 64)?;
                self.take(k.saturating_mul(64), "FakeWaterMeshInstance")?;
                self.take(40 + 10 + 125 + 48 + 4, "FakeWaterMeshComponent")?;
                let v = self.count("FakeWaterMeshComponent vertex count", 8)?;
                self.take(4, "FakeWaterMeshComponent")?;
                self.take(v.saturating_mul(8), "FakeWaterMeshComponent vertices")?;
                let i = self.count("FakeWaterMeshComponent index count", 2)?;
                self.take(4, "FakeWaterMeshComponent")?;
                self.take(i.saturating_mul(2), "FakeWaterMeshComponent indices")?;
            }
            // AutoFakeOrchestratorComponent: 4 bytes, n SubMeshFakeIndexSpans (an object, a count m, m FakeIndexSpans
            // of an object and two u32), a count p and 8 p bytes, 9 bytes (the notes: the one sequential end on 4,052
            // of 4,052)
            0xD4B4_4556 => {
                self.take(4, "AutoFakeOrchestratorComponent")?;
                let n = self.count("AutoFakeOrchestratorComponent count", 16)?;
                for _ in 0..n {
                    self.object("SubMeshFakeIndexSpans", 0x2AFD_2E35)?;
                    let m = self.count("SubMeshFakeIndexSpans count", 20)?;
                    for _ in 0..m {
                        self.object("FakeIndexSpan", 0xE747_72BA)?;
                        self.take(8, "FakeIndexSpan")?;
                    }
                }
                let p = self.count("AutoFakeOrchestratorComponent run", 8)?;
                self.take(p.saturating_mul(8), "AutoFakeOrchestratorComponent run")?;
                self.take(9, "AutoFakeOrchestratorComponent")?;
            }
            // DominoComponent: 4 bytes, a u32, an AIScriptInstance (00, an object), 2 bytes, a reference (its script),
            // a count n, n parameters, 21 bytes (the notes: 2,384 of 2,626 bodies end there); a parameter's value by its
            // kind (W's low byte), a value kind or embedded class outside the rule has none
            0xBE71_1F06 => {
                self.take(8, "DominoComponent")?;
                self.expect("AIScriptInstance", 0, 1)?;
                self.object("AIScriptInstance", 0x5EFD_B48C)?;
                self.take(12, "DominoComponent")?;
                let n = self.count("DominoComponent parameter count", 82)?;
                for _ in 0..n {
                    self.expect("DominoScriptInstanceParameter", 0, 1)?;
                    self.object("DominoScriptInstanceParameter", 0xEE14_E8BE)?;
                    self.expect("DominoDataProviderData", 0, 1)?;
                    self.object("DominoDataProviderData", 0x295E_9B7D)?;
                    self.object("DominoDynamic", 0x1A24_BE34)?;
                    self.expect("DominoDynamic", 3, 4)?;
                    self.object("DominoType", 0x86F5_9438)?;
                    let kind = self.u32("DominoType kind")? & 0xFF;
                    let class = self.u32("DominoType class")?;
                    self.take(4, "DominoType")?;
                    self.expect("Value", 0xDCB6_7730, 4)?;
                    self.expect("Value class", u64::from(class), 4)?;
                    self.take(8, "Value")?;
                    match kind {
                        0x1C => {
                            self.take(10, "Value reference")?;
                        }
                        0x12 => {
                            self.expect("Value object", 0, 1)?;
                            self.u64("Value object")?;
                        }
                        0x1D => {
                            let m = self.count("Value list", 9)?;
                            for _ in 0..m {
                                self.expect("Value list item", 0, 1)?;
                                self.u64("Value list item")?;
                            }
                        }
                        0x00 => {
                            self.take(1, "Value")?;
                        }
                        0x16 => {
                            self.u64("Value embedded object")?;
                            let size = match self.u32("Value embedded class")? {
                                0x7E42_F87F => 52,
                                0xB7DE_E7C7 => 84,
                                _ => return Ok(None),
                            };
                            self.take(size, "Value embedded object")?;
                        }
                        _ => return Ok(None),
                    }
                    self.take(13, "DominoScriptInstanceParameter")?;
                }
                self.take(21, "DominoComponent")?;
            }
            // WaterComponent: 4 bytes, a reference (its WaterMesh), a WaterMeshInstance (00, an object, a count k and
            // 64 k bytes), a bounding volume, 10 bytes, the 125-byte Visual tail, a RiverQuadTree (an object, 2 vec4, a
            // byte, n 61-byte nodes), a byte, m 49-byte WaterPatchDescs, 3 references (the notes: 290 + 61 n + 49 m, the
            // one sequential end on 36 of 36)
            0x996D_F94F => {
                self.take(14, "WaterComponent")?;
                self.expect("WaterMeshInstance", 0, 1)?;
                self.object("WaterMeshInstance", 0xC15B_678F)?;
                let k = self.count("WaterMeshInstance count", 64)?;
                self.take(k.saturating_mul(64), "WaterMeshInstance")?;
                self.take(40 + 10 + 125, "WaterComponent")?;
                self.object("RiverQuadTree", 0x92DB_01AE)?;
                self.take(33, "RiverQuadTree")?;
                let n = self.count("RiverQuadTree node count", 61)?;
                self.take(n.saturating_mul(61), "RiverQuadTreeNode")?;
                self.take(1, "WaterComponent")?;
                let m = self.count("WaterPatchDesc count", 49)?;
                self.take(m.saturating_mul(49), "WaterPatchDesc")?;
                self.take(30, "WaterComponent")?;
            }
            // AnimatedEntityComponent: 4 bytes, a count n, n 66-byte AnimationParameters, a byte (the notes: 9 + 66 n,
            // the one sequential end or the next component's header on 2,041 of 2,041)
            0x2996_CA50 => {
                self.take(4, "AnimatedEntityComponent")?;
                let n = self.count("AnimatedEntityComponent count", 66)?;
                self.take(n.saturating_mul(66), "AnimationParameters")?;
                self.take(1, "AnimatedEntityComponent")?;
            }
            // 0xE139F584 (unnamed): 4 bytes, an EventListener (an object, a byte, a count n: only 0 read, 13 bytes), a
            // flag byte, k ids (01, a u64), 9 bytes, a SoundInstance (00, an object, 52 bytes) (the notes: 113 + 9 k,
            // the one sequential end or the next component's header on 1,990 of 2,014; the rest hold objects in the
            // EventListener, without a rule)
            0xE139_F584 => {
                self.take(4, "component")?;
                self.object("EventListener", 0x2E8B_5553)?;
                self.take(1, "EventListener")?;
                if self.u32("EventListener count")? != 0 {
                    return Ok(None);
                }
                self.take(14, "EventListener")?;
                let k = self.count("component ids", 9)?;
                self.take(k.saturating_mul(9), "component ids")?;
                self.take(9, "component")?;
                self.expect("SoundInstance", 0, 1)?;
                self.object("SoundInstance", 0x7E42_F87F)?;
                self.take(52, "SoundInstance")?;
            }
            // BulkComponent (crowds): 4 bytes, BulkSettings (an object, a u32), a BulkRandomCompositionTemplate (00, an
            // object, an f32), t 36-byte BulkTemplates, a u32, an f32, c 45-byte members, 4 bytes, a 04-tagged object
            // and a byte, k BulkStates (an object, 9 bytes, a reference), 37 bytes, k SoundInstances (an object, 52
            // bytes), f f32, 17 bytes (the notes: the one sequential end on 2,481 of 2,481)
            0x3083_58A4 => {
                self.take(4, "BulkComponent")?;
                self.object("BulkSettings", 0x4404_F7CC)?;
                self.take(4, "BulkSettings")?;
                self.expect("BulkRandomCompositionTemplate", 0, 1)?;
                self.object("BulkRandomCompositionTemplate", 0x3E7C_FA88)?;
                self.take(4, "BulkRandomCompositionTemplate")?;
                let t = self.count("BulkTemplate count", 36)?;
                for _ in 0..t {
                    self.expect("BulkTemplate", 0, 1)?;
                    self.object("BulkTemplate", 0x3621_488A)?;
                    self.take(14, "BulkTemplate")?;
                    self.expect("BulkTemplate", 0, 1)?;
                    self.take(8, "BulkTemplate")?;
                }
                self.take(8, "BulkComponent")?;
                let c = self.count("BulkComponent member count", 45)?;
                for _ in 0..c {
                    self.object("BulkComponent member", 0x7A98_3CEC)?;
                    self.take(33, "BulkComponent member")?;
                }
                self.take(4, "BulkComponent")?;
                self.expect("BulkComponent object", 4, 1)?;
                self.object("BulkComponent object", 0xEACA_451C)?;
                self.take(1, "BulkComponent")?;
                let k = self.count("BulkState count", 31)?;
                for _ in 0..k {
                    self.object("BulkState", 0x7888_7331)?;
                    self.take(19, "BulkState")?;
                }
                self.take(37, "BulkComponent")?;
                for _ in 0..k {
                    self.object("SoundInstance", 0x7E42_F87F)?;
                    self.take(52, "SoundInstance")?;
                }
                let f = self.count("BulkComponent floats", 4)?;
                self.take(f.saturating_mul(4), "BulkComponent floats")?;
                self.take(17, "BulkComponent")?;
            }
            // FXComponent: 4 bytes, an object, 6 bytes, a reference (its FX), an f32, an object, a u32, n parameters
            // (a u32 id, a u32, a u16, a u16 kind, a u32, the value by its kind: 0x0A an f32, 0x0D a vec4; another
            // kind has no rule), 11 bytes (the notes: 67 + the parameters', the one sequential end or the next
            // component's header on every one of the 1,089 bodies reached)
            0xD2BF_93A6 => {
                self.take(4, "FXComponent")?;
                self.object("FXComponent", 0x1159_8A66)?;
                self.take(20, "FXComponent")?;
                self.object("FXComponent", 0x32D3_3700)?;
                self.take(4, "FXComponent")?;
                let n = self.count("FXComponent parameter count", 16)?;
                for _ in 0..n {
                    self.take(10, "FXComponent parameter")?;
                    let kind = u16::from_le_bytes(self.array::<2>("FXComponent parameter kind")?);
                    self.take(4, "FXComponent parameter")?;
                    match kind {
                        0x0A => self.take(4, "FXComponent parameter value")?,
                        0x0D => self.take(16, "FXComponent parameter value")?,
                        _ => return Ok(None),
                    };
                }
                self.take(11, "FXComponent")?;
            }
            // 0x341877D6 (unnamed): 4 bytes, a count n, n 109-byte items, each from a 04-tagged 0x30C186C4 object (the
            // notes: 8 + 109 n, the one sequential end on 190 of 190; the rest of an item, 0x518AFC74 and IsLocked
            // objects with references, not named)
            0x3418_77D6 => {
                self.take(4, "component")?;
                let n = self.count("component count", 109)?;
                for _ in 0..n {
                    self.expect("component item", 4, 1)?;
                    self.object("component item", 0x30C1_86C4)?;
                    self.take(96, "component item")?;
                }
            }
            // 0xEEF3F11F (unnamed): 9 bytes (the notes: 00 01 00 00, a u32 0, 03 on 483 of 483)
            0xEEF3_F11F => {
                self.take(9, "component")?;
            }
            // BreakableComponent: 4 bytes, a u32 (only 0 read), k ids (01, a u64), m ids, a 102-byte audio blocks (an
            // AudioEventSeed object first: 00, a u64, 0x561E06FC), 20 bytes (the notes: k 3, m 1, a 1 on all 1,070,
            // 178 bytes, the next component's header there on 1,070 of 1,070; other counts read as counts, not seen)
            0x44F8_FD99 => {
                self.take(4, "BreakableComponent")?;
                if self.u32("BreakableComponent")? != 0 {
                    return Ok(None);
                }
                for what in ["BreakableComponent ids", "BreakableComponent ids"] {
                    let k = self.count(what, 9)?;
                    for _ in 0..k {
                        self.expect(what, 1, 1)?;
                        self.u64(what)?;
                    }
                }
                let a = self.count("BreakableComponent audio count", 102)?;
                for _ in 0..a {
                    self.expect("AudioEventSeed", 0, 1)?;
                    self.object("AudioEventSeed", 0x561E_06FC)?;
                    self.take(89, "BreakableComponent audio")?;
                }
                self.take(20, "BreakableComponent")?;
            }
            // GameplayContextDisablerComponent: 4 bytes, a condition tree, m (a byte 00 or 01, a u64) (the notes: the
            // one sequential end on 99 of 99 measured, the next component's header on the other 2,216 of 2,315)
            0xBB7F_5B1D => {
                self.take(4, "GameplayContextDisablerComponent")?;
                if !self.condition(0)? {
                    return Ok(None);
                }
                let m = self.count("GameplayContextDisablerComponent count", 9)?;
                for _ in 0..m {
                    let what = "GameplayContextDisablerComponent item";
                    let at = self.at;
                    let found = self.u8(what)?;
                    if found > 1 {
                        return Err(Error::Value {
                            what,
                            at,
                            found: u64::from(found),
                            read: "0 or 1",
                        });
                    }
                    self.u64(what)?;
                }
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

    /// The StaticDecalComponents' Visuals: each a LODSelector made for its one placement (docs/entity-grammar.md §9.1).
    pub fn decals(&self) -> impl Iterator<Item = &Visual> {
        self.components.iter().filter_map(|c| match &c.body {
            ComponentBody::StaticDecal(v) => Some(v),
            _ => None,
        })
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
        let at = c.at;
        let tag = c.u8("component tag")?;
        if !matches!(tag, 0 | 4) {
            return Err(Error::Value {
                what: "component tag",
                at,
                found: u64::from(tag),
                read: "4 or 0",
            });
        }
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
        components.push(Component { tag, id, kind, body });
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
