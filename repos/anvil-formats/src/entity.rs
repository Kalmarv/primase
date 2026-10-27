//! An Entity resource's body read in order from its first byte to its last: its matrix, its components (each stepped
//! over by its type's length rule, a Visual read through to its mesh instances), its scale and bounds, and its tail.
//! What the fields mean beyond that grammar is not read. A component of a type without a length rule is kept as its
//! stored bytes ([`ComponentBody::Unread`]): the components are delimited by their headers instead (see
//! [`StoredEntity::parse`]), so no body is refused for a component type it doesn't know.
//!
//! The body (little-endian): `01`, then `03`, or `00` and an inline Hierarchy (a u64, 0x788BAA0D, 64 bytes, and a
//! HierarchyHandle: a u64, 0x709FB9D4, a u32 and 9 bytes); 16 f32 (a column-major matrix: on every Entity measured its
//! last row is 0 0 0 1 and its upper 3x3 has determinant 1); a u32 count and that many components, each a tag (`04`, or
//! `00`), a u64 id, a u32 type and a body by the type's rule ([`Component`]); 43 bytes; a u32; an f32 (the uniform
//! scale); a bounding volume (a u64, its type 0x4AEC3476, 6 f32 min and max, a u32); an EntityDescriptor (a u64,
//! 0x60121A9E, 13 bytes); a DataLayerFilter (a u64, 0xDB1D406E, a u32 count k, k actions each a u64, 0xE31593E1, `00`,
//! a DataLayer ClassID and a u32, then a u32 count m and m links, each a tag `00` or `01` and a u64); then `03` ending
//! the body, or `00`, a u64, GameStateData 0x7313743E, a u32, and two length-prefixed byte runs ending the body.
//!
//! An EntityGroup's body is an Entity's, then a u32 count c, c references to its members, a u32 count m, m links (as
//! the DataLayerFilter's) and a u32 (the Riders Republic notes: docs/world.md §7, from the end on 48,755 of 48,803
//! USNAT groups; the census measures it read from the start). What a member or a link is to its group is not read.
//!
//! The grammar is from the Riders Republic placement notes (docs/entity-grammar.md, consumed to the last byte on 8 of
//! 8 fixtures and 97% of 5,200 sampled USNAT Entities); a census measures it on every
//! Entity of the game. Fixed component sizes and the 125-byte Visual tail are observations on every body measured,
//! not stored lengths.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::fields::{BodyError, LE, body_error, count, end, expect, object, reference};
use crate::reference::{self, Reference};
use primase_foundations::bytes::{Field, View};

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
/// A GrassVisual's type: a Visual of one grass model (the registry's class, a Visual and one byte).
pub const GRASS_VISUAL: u32 = 0x8FA8_0D8D;
/// A GrassComponent's type: the grass system's setup (a Visual, the library it names, its settings and its models).
pub const GRASS_COMPONENT: u32 = 0xCB28_F06D;
/// A SkeletonComponent's type: the CRC-32 of its name.
pub const SKELETON_COMPONENT: u32 = 0x71FD_A747;
/// An AnimComponent's type: the CRC-32 of its name.
pub const ANIM_COMPONENT: u32 = 0x8C8A_A19D;
/// An AnimTrackDefinition's type, and a WindForce's: the SkeletonComponent's embedded objects.
const ANIM_TRACK_DEFINITION: u32 = 0xA803_7D53;
const WIND_FORCE: u32 = 0xE7F9_A244;
/// A GrassVisualInstance's type, and a GrassInstance's (the GrassComponent's).
const GRASS_VISUAL_INSTANCE: u32 = 0xD456_9683;
const GRASS_INSTANCE: u32 = 0x9C95_4A88;
/// A GrassSettings' type.
const GRASS_SETTINGS: u32 = 0x3BE0_5B93;

/// Why an Entity body can't be read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Entity {0}")]
    Body(#[from] BodyError),
    #[error("Entity component {index}: type {kind:08X} has no length rule")]
    Component { index: usize, kind: u32 },
    #[error(
        "Entity component {index}: type {kind:08X} has no length rule and the tail is not found after it"
    )]
    NoTail { index: usize, kind: u32 },
    #[error(
        "Entity component {index}: type {kind:08X} has no length rule and {found} component headers follow, the count says {want}"
    )]
    Headers {
        index: usize,
        kind: u32,
        found: usize,
        want: usize,
    },
    #[error("Entity Visual: instance data type {0:08X} isn't read")]
    Instance(u32),
}
body_error!(Error, Error::Body);

impl Error {
    /// Why without where ([`BodyError::reason`]): the same for every body that fails alike.
    pub fn reason(&self) -> String {
        match self {
            Self::Body(e) => e.reason(),
            Self::Component { kind, .. } => format!("type {kind:08X} has no length rule"),
            Self::NoTail { kind, .. } => {
                format!("type {kind:08X} has no length rule and the tail is not found after it")
            }
            Self::Headers { kind, .. } => {
                format!(
                    "type {kind:08X} has no length rule and the component headers following aren't the count's"
                )
            }
            Self::Instance(_) => self.to_string(),
        }
    }
}

/// Why a component's rule refused the bytes within its extent.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The rule's read failed.
    #[error("{0}")]
    Rule(Error),
    /// The rule read to `at`, not to the extent's `end`.
    #[error("its rule ends at {at}, not at the extent's end {end}")]
    Ends { at: usize, end: usize },
}

impl Refusal {
    /// Why without where: the same for every component refused alike (they are counted by it).
    pub fn reason(&self) -> String {
        match self {
            Self::Rule(e) => e.reason(),
            Self::Ends { .. } => "its rule doesn't end at the extent's end".to_owned(),
        }
    }
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
    /// The 16 bytes after them, as stored: an `InstanceShaderConstants` pointer (`03`), `IsFake`, `IsNearFake`,
    /// `FakeMeshIndex` (u32) and the `CompiledMeshInstance` pointer's tag and id (registry-named-fields §9).
    pub reserved: [u8; 16],
    /// The CompiledMeshInstance's 17 bytes after its type, as stored.
    pub compiled: [u8; 17],
    /// Its material-info entries: (the owner's id, a source Material, a replacement Material).
    pub material_infos: Vec<[u64; 3]>,
    /// One bounding volume per instance.
    pub instance_bounds: Vec<BoundingVolume>,
}

impl MeshInstance {
    /// `IsFake`: set on exactly the 1,031 `MAD_USNAT_FarFake_<n>_Cell` Entities of Riders Republic, the far terrain's
    /// cells (docs/placement.md §19.4).
    pub fn is_fake(&self) -> bool {
        self.reserved[1] != 0
    }
    /// `IsNearFake`.
    pub fn is_near_fake(&self) -> bool {
        self.reserved[2] != 0
    }
    /// `FakeMeshIndex` (the cell number `n` on every FarFake cell).
    pub fn fake_mesh_index(&self) -> u32 {
        let [_, _, _, a, b, c, d, ..] = self.reserved;
        u32::from_le_bytes([a, b, c, d])
    }
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

/// A GrassVisual component as stored: the Visual's head and the model's Mesh, the GrassVisualInstance (a MeshInstanceData
/// with no instance matrices on all 46 of Riders Republic's) with the u16 array it stores, the Visual's tail and the
/// model's number. The grass instances themselves are not stored here (docs/grass.md: the engine creates them at run
/// time).
#[derive(Clone, Debug, PartialEq)]
pub struct GrassVisual {
    pub head: [u8; 4],
    /// The model's Mesh (a Mesh directly, not a LODSelector).
    pub object: Option<Reference>,
    /// The GrassVisualInstance's id.
    pub id: u64,
    pub instance: MeshInstance,
    /// The GrassVisualInstance's u16 array: 0 to 2 (or 3) after four times the model's number on the 46 models (read
    /// by the registry as an array of u16; the Mesh's ranges by value).
    pub ranges: Vec<u16>,
    /// Its 125-byte Visual tail, as stored.
    pub tail: Vec<u8>,
    /// `GrassModelID`: the model's number (0 to 45, in the order of the setup's ManagedEntities).
    pub model: u8,
}

/// A GrassComponent as stored (the one of `SETUP_GrassUS_NAT`): the Visual part (head, the object it names, the
/// GrassInstance with a MeshInstanceData's fields and the bytes after them, the tail), the library reference, the
/// GrassSettings as stored and the ManagedEntities' ids. Read over its extent (the settings' run isn't assigned to the
/// registry's field names, so it has no length rule: see [`StoredEntity::parse`]).
#[derive(Clone, Debug, PartialEq)]
pub struct GrassSetup {
    pub head: [u8; 4],
    pub object: Option<Reference>,
    pub id: u64,
    pub instance: MeshInstance,
    /// The bytes between the MeshInstanceData's fields and the Visual tail, as stored (none on both copies: the
    /// GrassInstance stores nothing there).
    pub instance_extra: Vec<u8>,
    pub tail: Vec<u8>,
    /// `GrassPatternLibraryReference`: the library (a GrassPatternLibrary).
    pub library: Option<Reference>,
    /// The inline GrassSettings object's id.
    pub settings_id: u64,
    /// Its body as stored: WorldExtent, WorldMinZ and WorldMaxZ (u32), the WorldBoundingBox (a BoundingVolume), then
    /// `GrassVisibilityDistance` (a u16), 16 f32, four tables of 16 u16 (the LOD start distances by model class)
    /// and the run of floats and bytes the notes did not assign to field names (docs/grass.md §2.1).
    pub settings: Vec<u8>,
    /// ManagedEntities: the GrassVisual Entities' ids, in order (a model's number is its position).
    pub entities: Vec<u64>,
}

impl GrassSetup {
    /// The GrassSettings' `GrassVisibilityDistance` (metres: 250 on both copies), after its three u32 and its
    /// BoundingVolume.
    pub fn visibility_distance(&self) -> Option<u16> {
        let at: usize = 12 + 8 + 4 + 24 + 4;
        self.settings
            .get(at..at.checked_add(2)?)
            .and_then(|w| <[u8; 2]>::try_from(w).ok())
            .map(u16::from_le_bytes)
    }
}

/// A SkeletonComponent as stored, read over its extent: the rig an Entity's meshes bind to. Its grammar, by the registry's
/// serialised fields in the order the 146 bodies of Riders Republic hold them (56 bytes with no addon and no track, 10
/// more for each addon, 20 more for each track): the head, the main Skeleton's reference, the addon Skeletons' (a u32
/// count and that many references), the ExpressionPack's reference, the AnimTrackDefinitions (a u32 count and that many
/// objects) and the embedded WindForce.
#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonBinding {
    /// The 4 bytes before the references: `00 01 00 00` on 134 of the bodies, `01 01 00 00` on 12.
    pub head: [u8; 4],
    /// The main Skeleton (the registry's `BC9C5810`, a Skeleton).
    pub main: Option<Reference>,
    /// The addon Skeletons (`408196BB`, an array of Skeleton references), in order.
    pub addons: Vec<Reference>,
    /// `ExpressionPack`.
    pub expression_pack: Option<Reference>,
    /// The AnimTrackDefinitions (`43845470`): each an embedded object's id and the 8 bytes after its type.
    pub tracks: Vec<(u64, [u8; 8])>,
    /// The embedded WindForce's id and its 12 bytes after its type.
    pub wind_force: (u64, [u8; 12]),
}

/// An AnimComponent as stored, read over its extent: the Animations an Entity plays. Its grammar, on all 70 of Riders
/// Republic's (51 list none): a head of 4 bytes (`00 01 00 00`), a u32 count and that many references (the Animation
/// resources, 10 bytes each, a null one included: `MAD_ICON` lists 31, the first null), then the rest, as stored (a blending time
/// of 0.2 s among it, and a bare u64 that names one of the Animations again).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimBinding {
    /// The 4 bytes before the count.
    pub head: [u8; 4],
    /// The Animations (`Animations`, an array of references: none where the element is null), in order.
    pub animations: Vec<Option<Reference>>,
    /// The bytes after them to the component's end.
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
    /// A GrassVisual: one grass model.
    GrassVisual(GrassVisual),
    /// A GrassComponent: the grass system's setup, read over its extent.
    GrassSetup(Box<GrassSetup>),
    /// A SkeletonComponent: the rig an Entity's meshes bind to, read over its extent.
    Skeleton(Box<SkeletonBinding>),
    /// An AnimComponent: the Animations an Entity plays, read over its extent.
    Anim(Box<AnimBinding>),
    /// A type stepped over by its length rule, its bytes kept as stored.
    Raw(Vec<u8>),
    /// A component whose bytes no rule here reads, kept as stored: its type has no length rule, its extent the
    /// headers' (an Entity's components are delimited by their tag, id and type headers: see [`StoredEntity::parse`]).
    Unread(Vec<u8>),
    /// A component of a type read here whose rule refused the bytes within its extent, kept as stored with why.
    Refused {
        bytes: Vec<u8>,
        why: Refusal,
    },
}

/// `CollisionLayerBits` slots by the registry's names.
pub mod layer {
    pub const STATIC: u32 = 0;
    pub const DYNAMIC: u32 = 2;
    pub const DEBRIS: u32 = 3;
    pub const DETECTION_ZONE: u32 = 4;
    pub const MAIN_CHARACTER: u32 = 5;
    pub const CHARACTER: u32 = 6;
    pub const CHARACTER_NO_INTERACTIVE: u32 = 7;
    pub const WATER_LEVEL: u32 = 13;
    pub const TARGETABLE: u32 = 14;
    /// `TerrainLod0`; `TerrainLod1` to `4` follow.
    pub const TERRAIN_LOD0: u32 = 16;
}

/// `QueryLayerBits` slots by the registry's names (the others are unnamed or unset in the data).
pub mod query {
    pub const STATIC: u32 = 0;
    pub const DYNAMIC: u32 = 2;
    pub const CAMERA_COLLISION: u32 = 3;
    pub const CAMERA_OCCLUSION: u32 = 5;
    pub const CHARACTER: u32 = 9;
    pub const WATER_LEVEL: u32 = 10;
    pub const PARTICLE: u32 = 11;
    pub const RAYCAST: u32 = 19;
    pub const OCCLUSION_HEIGHT_TEST: u32 = 20;
    pub const TARGETING: u32 = 21;
    /// `TerrainLod0`; `TerrainLod1` to `4` follow.
    pub const TERRAIN_LOD0: u32 = 25;
}

/// A RigidBody's `CollisionFilterInfo` as the game's registry names it (registry-named-fields §9, §10): its
/// `QueryFlags` (a `QueryLayerBits`: 34 bit fields, each stored as a u64 0 or 1, from the RigidBodyComponent's offset
/// 68) and its two `CollisionLayerBits` (24 each, from 352 and 556), as bit masks by slot, and its `IsGround` (797).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollisionFilter {
    pub query: u64,
    pub layers: [u32; 2],
    pub ground: bool,
}

impl CollisionFilter {
    /// Whether `QueryFlags` slot `slot` ([`query`]) is set.
    pub fn queries(&self, slot: u32) -> bool {
        self.query.checked_shr(slot).is_some_and(|q| q & 1 == 1)
    }

    /// The filter of a RigidBodyComponent's `bytes` (from its offset 0). Refused where a slot holds other than 0 or 1
    /// or the body is short.
    pub fn parse(bytes: &[u8]) -> Result<CollisionFilter, Error> {
        let slots = |at: usize, n: usize| -> Result<u64, Error> {
            let mut bits = 0u64;
            for k in 0..n {
                let from = k
                    .checked_mul(8)
                    .and_then(|o| o.checked_add(at))
                    .unwrap_or(usize::MAX);
                let word = View::new(bytes).u64(from, LE).field("RigidBody bit slot")?;
                match word {
                    0 => {}
                    1 => {
                        bits |= 1u64
                            .checked_shl(u32::try_from(k).unwrap_or(u32::MAX))
                            .unwrap_or(0)
                    }
                    found => {
                        return Err(BodyError::Value {
                            what: "RigidBody bit slot",
                            at: from,
                            found,
                            read: "0 or 1",
                        }
                        .into());
                    }
                }
            }
            Ok(bits)
        };
        let ground = match View::new(bytes).u8(797).field("RigidBody IsGround")? {
            0 => false,
            1 => true,
            found => {
                return Err(BodyError::Value {
                    what: "RigidBody IsGround",
                    at: 797,
                    found: u64::from(found),
                    read: "0 or 1",
                }
                .into());
            }
        };
        Ok(CollisionFilter {
            query: slots(68, 34)?,
            // 24 slots: within u32
            layers: [
                u32::try_from(slots(352, 24)?).unwrap_or(u32::MAX),
                u32::try_from(slots(556, 24)?).unwrap_or(u32::MAX),
            ],
            ground,
        })
    }
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
    /// The ids of the links after the DataLayerFilter's actions (a u32 count, then each a `00` and a u64): the UI
    /// Entities' children or targets on the bodies measured that have any, none on the others.
    pub links: Vec<u64>,
    pub tail: Tail,
    /// The inline Hierarchy object that follows the first byte where the second is `00` (a pointer: `03` is none), as
    /// stored from its id: its id, type `0x788BAA0D`, 64 bytes and a HierarchyHandle (id, `0x709FB9D4`, a u32 and a
    /// 9-byte link). On 830 bodies of Riders Republic (605 Entities, 225 groups): sequence and checkpoint groups and
    /// some props. What it means to the Entity's placement isn't read.
    pub hierarchy: Option<Vec<u8>>,
}

/// Whether a u32 is the type of one of the game's component classes (the caller's registry: see
/// [`StoredEntity::parse`]).
type ComponentTypes<'a> = &'a dyn Fn(u32) -> bool;

/// The body's cursor ([`primase_foundations::bytes::Cursor`]), with this record's grammar as methods, and the component types it
/// delimits components by.
struct Cursor<'a> {
    c: primase_foundations::bytes::Cursor<'a>,
    types: ComponentTypes<'a>,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8], types: ComponentTypes<'a>) -> Self {
        Cursor {
            c: primase_foundations::bytes::Cursor::new(b),
            types,
        }
    }
    /// A cursor over `b` at `at`, refused past its end (naming `what`).
    fn at(
        b: &'a [u8],
        at: usize,
        what: &'static str,
        types: ComponentTypes<'a>,
    ) -> Result<Self, primase_foundations::bytes::FieldError> {
        let mut c = Cursor::new(b, types);
        c.seek(at).field(what)?;
        Ok(c)
    }
}

impl<'a> std::ops::Deref for Cursor<'a> {
    type Target = primase_foundations::bytes::Cursor<'a>;
    fn deref(&self) -> &Self::Target {
        &self.c
    }
}

impl std::ops::DerefMut for Cursor<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.c
    }
}

impl Cursor<'_> {
    /// A GameplayContextDisabler's condition node (04, a u64, its type): an operator (0x025AD7F9: a byte, an op
    /// 0, 1 or 2, a count n, n child nodes) or a leaf (0x0FA689EF: 2 bytes, a u64). False where its type is neither
    /// or it nests deeper than the notes saw (2).
    fn condition(&mut self, depth: usize) -> Result<bool, Error> {
        expect(self, "condition node", 4, 1)?;
        self.u64(LE).field("condition node")?;
        match self.u32(LE).field("condition node type")? {
            0x025A_D7F9 if depth < 2 => {
                self.take(1).field("condition operator")?;
                let at = self.position();
                let op = self.u32(LE).field("condition operator")?;
                if op > 2 {
                    return Err(BodyError::Value {
                        what: "condition operator",
                        at,
                        found: u64::from(op),
                        read: "0, 1 or 2",
                    }
                    .into());
                }
                let n = count(self, "condition operator count", 23)?;
                for _ in 0..n {
                    if !self.condition(depth.saturating_add(1))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            0x0FA6_89EF => {
                self.take(10).field("condition leaf")?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
    fn matrix(&mut self) -> Result<[f32; 16], Error> {
        let mut m = [0.0; 16];
        for x in &mut m {
            *x = self.f32(LE).field("matrix")?;
        }
        Ok(m)
    }
    fn matrices(&mut self) -> Result<Vec<[f32; 16]>, Error> {
        let n = count(self, "instance count", 64)?;
        (0..n).map(|_| self.matrix()).collect()
    }
    fn bounds(&mut self) -> Result<BoundingVolume, Error> {
        let id = object(self, "bounding volume", BOUNDING_VOLUME)?;
        let mut f = [0.0f32; 6];
        for x in &mut f {
            *x = self.f32(LE).field("bounding volume")?;
        }
        let [a, b, c, d, e, g] = f;
        Ok(BoundingVolume {
            id,
            min: [a, b, c],
            max: [d, e, g],
            word: self.u32(LE).field("bounding volume")?,
        })
    }
    fn mesh_instance(&mut self, id: u64) -> Result<MeshInstance, Error> {
        let instances = self.matrices()?;
        let bounds = self.bounds()?;
        let reserved = self.array::<16>().field("MeshInstanceData")?;
        expect(self, "CompiledMeshInstance", 0x4368_101B, 4)?;
        let compiled = self.array::<17>().field("CompiledMeshInstance")?;
        let a = count(self, "material-info count", 41)?;
        let mut material_infos = Vec::with_capacity(a);
        for _ in 0..a {
            expect(self, "material info", 0, 1)?;
            object(self, "material info", 0x995B_FBF5)?;
            expect(self, "material info", 2, 1)?;
            let owner = self.u64(LE).field("material info")?;
            expect(self, "material info", 0, 1)?;
            let source = self.u64(LE).field("material info")?;
            let at = self.position();
            if !matches!(self.array::<2>().field("material info")?, [1, 1 | 4]) {
                return Err(BodyError::Value {
                    what: "material info replacement tag",
                    at,
                    found: 0,
                    read: "01 01 or 01 04",
                }
                .into());
            }
            let replacement = self.u64(LE).field("material info")?;
            material_infos.push([owner, source, replacement]);
        }
        let b = count(self, "instance bounds count", 40)?;
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
        let start = self.position();
        self.take(8).field("Visual tail")?;
        expect(self, "Visual tail handle", 3, 1)?;
        self.take(116).field("Visual tail")?;
        Ok(self
            .bytes()
            .get(start..self.position())
            .unwrap_or_default()
            .to_vec())
    }
    /// A Visual's head, the object it names and its instance's id and type (which must be `kind`): the start of a
    /// grass Visual, whose instance has the MeshInstanceData's fields.
    fn grass_head(&mut self, kind: u32) -> Result<([u8; 4], Option<Reference>, u64), Error> {
        let head = self.array::<4>().field("GrassVisual")?;
        let named = reference(self)?;
        expect(self, "GrassVisual instance tag", 0, 1)?;
        let id = object(self, "GrassVisual instance", kind)?;
        Ok((head, named, id))
    }
    /// A GrassVisual's body: [`GrassVisual`].
    fn grass_visual(&mut self) -> Result<GrassVisual, Error> {
        let (head, object, id) = self.grass_head(GRASS_VISUAL_INSTANCE)?;
        let instance = self.mesh_instance(id)?;
        let n = count(self, "GrassVisualInstance range count", 2)?;
        let ranges = (0..n)
            .map(|_| {
                Ok(u16::from_le_bytes(
                    self.array::<2>().field("GrassVisualInstance range")?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        // GrassImposterProperties: a null pointer on all 46
        expect(self, "GrassImposterProperties pointer", 3, 1)?;
        let tail = self.visual_tail()?;
        let model = self.u8().field("GrassModelID")?;
        Ok(GrassVisual {
            head,
            object,
            id,
            instance,
            ranges,
            tail,
            model,
        })
    }
    /// An AnimComponent's body over the extent that ends at `end`: [`AnimBinding`].
    fn anim_binding(&mut self, end: usize) -> Result<AnimBinding, Error> {
        let head = self.array::<4>().field("AnimComponent")?;
        let n = count(self, "AnimComponent animation count", reference::LENGTH)?;
        let mut animations = Vec::with_capacity(n);
        for _ in 0..n {
            animations.push(reference(self)?);
        }
        let tail = self.take_to(end).field("AnimComponent")?.to_vec();
        Ok(AnimBinding {
            head,
            animations,
            tail,
        })
    }
    /// A SkeletonComponent's body: [`SkeletonBinding`].
    fn skeleton_binding(&mut self) -> Result<SkeletonBinding, Error> {
        let head = self.array::<4>().field("SkeletonComponent")?;
        let main = reference(self)?;
        let n = count(self, "SkeletonComponent addon count", reference::LENGTH)?;
        let mut addons = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.position();
            addons.push(reference(self)?.ok_or(BodyError::Value {
                what: "SkeletonComponent addon",
                at,
                found: 0,
                read: "a reference to a Skeleton",
            })?);
        }
        let expression_pack = reference(self)?;
        let m = count(self, "SkeletonComponent track count", 20)?;
        let mut tracks = Vec::with_capacity(m);
        for _ in 0..m {
            let id = object(self, "AnimTrackDefinition", ANIM_TRACK_DEFINITION)?;
            tracks.push((id, self.array::<8>().field("AnimTrackDefinition")?));
        }
        let wind_id = object(self, "WindForce", WIND_FORCE)?;
        let wind_force = (wind_id, self.array::<12>().field("WindForce")?);
        Ok(SkeletonBinding {
            head,
            main,
            addons,
            expression_pack,
            tracks,
            wind_force,
        })
    }
    /// A GrassComponent's body over the extent that ends at `end`: [`GrassSetup`]. The settings' run has no length
    /// rule, so the library reference and the settings object are found from the Visual tail's end, and the
    /// ManagedEntities (a u32 count and that many 9-byte links, the body's last field) from the end.
    fn grass_setup(&mut self, end: usize) -> Result<GrassSetup, Error> {
        let (head, named, id) = self.grass_head(GRASS_INSTANCE)?;
        let instance = self.mesh_instance(id)?;
        // the settings object's type, 12 bytes after the library reference, 125 after the Visual tail's start
        let settings_type = (self.position()..end)
            .find(|&at| self.word_at(at) == Some(GRASS_SETTINGS))
            .ok_or(BodyError::Value {
                what: "GrassSettings",
                at: self.position(),
                found: 0,
                read: "an object of type 0x3BE05B93",
            })?;
        let settings_id_at = settings_type.saturating_sub(8);
        let library_at = settings_id_at.saturating_sub(reference::LENGTH);
        let tail_at = library_at.saturating_sub(125);
        let extra = self.take_to(tail_at).field("GrassInstance")?.to_vec();
        let tail = self.visual_tail()?;
        let library = reference(self)?;
        let settings_id = object(self, "GrassSettings", GRASS_SETTINGS)?;
        let body_at = self.position();
        // ManagedEntities: the count whose links fill the rest of the body
        let links_at = (body_at..end)
            .find(|&at| {
                let Some(k) = self.word_at(at).and_then(|k| usize::try_from(k).ok()) else {
                    return false;
                };
                k.checked_mul(9)
                    .and_then(|bytes| bytes.checked_add(at)?.checked_add(4))
                    == Some(end)
                    && (0..k).all(|j| {
                        at.checked_add(4)
                            .and_then(|a| a.checked_add(j.checked_mul(9)?))
                            .and_then(|a| self.bytes().get(a))
                            .is_some_and(|&t| matches!(t, 0 | 1))
                    })
            })
            .ok_or(BodyError::Value {
                what: "GrassComponent ManagedEntities",
                at: body_at,
                found: 0,
                read: "a count of 9-byte links ending the body",
            })?;
        let settings = self.bytes().get(body_at..links_at).unwrap_or_default().to_vec();
        self.seek(links_at).field("GrassComponent ManagedEntities")?;
        let k = count(self, "ManagedEntities count", 9)?;
        let mut entities = Vec::with_capacity(k);
        for _ in 0..k {
            self.link_tag("ManagedEntities link")?;
            entities.push(self.u64(LE).field("ManagedEntities link")?);
        }
        Ok(GrassSetup {
            head,
            object: named,
            id,
            instance,
            instance_extra: extra,
            tail,
            library,
            settings_id,
            settings,
            entities,
        })
    }
    fn visual(&mut self) -> Result<Visual, Error> {
        let head = self.array::<4>().field("Visual")?;
        let object = reference(self)?;
        let at = self.position();
        match self.u8().field("Visual instance tag")? {
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
                return Err(BodyError::Value {
                    what: "Visual instance tag",
                    at,
                    found: u64::from(found),
                    read: "0, or 3 (none)",
                }
                .into());
            }
        }
        let id = self.u64(LE).field("Visual instance")?;
        let kind = self.u32(LE).field("Visual instance type")?;
        let (instance, tail) = match kind {
            LOD_SELECTOR_INSTANCE => {
                let instances = self.matrices()?;
                let bounds = self.bounds()?;
                let selector = self.array::<10>().field("LODSelectorInstance selector")?;
                let mut slots = Vec::with_capacity(5);
                for _ in 0..5 {
                    slots.push(match self.u8().field("slot tag")? {
                        3 => Slot::None,
                        0 => {
                            let slot_id = self.u64(LE).field("slot")?;
                            match self.u32(LE).field("slot type")? {
                                MESH_INSTANCE_DATA => Slot::Mesh(self.mesh_instance(slot_id)?),
                                IMPOSTOR_INSTANCE_DATA => {
                                    let instances = self.matrices()?;
                                    let bounds = self.bounds()?;
                                    Slot::Impostor(ImpostorInstance {
                                        id: slot_id,
                                        instances,
                                        bounds,
                                        reference: self.array::<10>().field("impostor reference")?,
                                    })
                                }
                                other => return Err(Error::Instance(other)),
                            }
                        }
                        tag => {
                            return Err(BodyError::Value {
                                what: "slot tag",
                                at: self.position().saturating_sub(1),
                                found: u64::from(tag),
                                read: "3 or 0",
                            }
                            .into());
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
                let bytes = self.take(311).field("LightInstance")?.to_vec();
                (Instance::Raw { kind, id, bytes }, Vec::new())
            }
            SPLASH_FX_INSTANCE_DATA => {
                let start = self.position();
                self.matrices()?;
                self.bounds()?;
                self.take(10).field("SplashFXInstanceData reference")?;
                let bytes = self
                    .bytes()
                    .get(start..self.position())
                    .unwrap_or_default()
                    .to_vec();
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
        let start = self.position();
        match kind {
            // RigidBodyComponent: 826 bytes, a handle (03, or 05 and a u64), a u32 0, a PhysicsActivityZone object and its
            // ActivityZoneDesc array (a count and that many 27-byte items: an id, 0x56F0AAEA, a u32 enum, two f32 and
            // three bytes; the registry's class; count 0 on every body but one, an NPC puppet's with 2)
            0xC177_D702 => {
                self.take(826).field("RigidBodyComponent")?;
                match self.u8().field("RigidBodyComponent handle")? {
                    3 => {}
                    5 => {
                        self.u64(LE).field("RigidBodyComponent handle")?;
                    }
                    found => {
                        return Err(BodyError::Value {
                            what: "RigidBodyComponent handle",
                            at: self.position().saturating_sub(1),
                            found: u64::from(found),
                            read: "3 or 5",
                        }
                        .into());
                    }
                }
                expect(self, "RigidBodyComponent", 0, 4)?;
                object(self, "RigidBodyComponent", 0x18D5_28B6)?;
                let zones = count(self, "PhysicsActivityZone count", 27)?;
                self.take(zones.saturating_mul(27)).field("PhysicsActivityZone")?;
            }
            // AmbientEffectGeneratorComponent: 4 bytes, a count c, 87 c bytes, 4 bytes
            0x713F_0C7D => {
                self.take(4).field("component")?;
                let c = count(self, "component count", 87)?;
                self.take(c.saturating_mul(87)).field("component")?;
                self.take(4).field("component")?;
            }
            // MaterialOverrider: 4 bytes, a count d, d OverrideDefinitions, 10 bytes
            0x78A1_3BF7 => {
                self.take(4).field("MaterialOverrider")?;
                let d = count(self, "MaterialOverrider count", 31)?;
                for _ in 0..d {
                    object(self, "OverrideDefinition", 0x49FC_D7BF)?;
                    self.take(19).field("OverrideDefinition")?;
                }
                self.take(10).field("MaterialOverrider")?;
            }
            // GuidanceSystem: 4 bytes, N GuidanceObjects (an object, 29 bytes), 12 bytes, a counted run of 2-byte
            // items, an EdgeFilter (40), a Partitioner (46 + 2k, its u16 p at +12 and u32 k at +14), p + 1 PartNodes
            // (an object, 12 bytes), a byte
            0x55AF_1C3E => {
                self.take(4).field("GuidanceSystem")?;
                let n = count(self, "GuidanceSystem object count", 41)?;
                for _ in 0..n {
                    object(self, "GuidanceObject", 0x344F_A659)?;
                    self.take(29).field("GuidanceObject")?;
                }
                self.take(12).field("GuidanceSystem")?;
                let m = count(self, "GuidanceSystem run count", 2)?;
                self.take(m.saturating_mul(2)).field("GuidanceSystem run")?;
                self.take(40).field("EdgeFilter")?;
                self.take(12).field("Partitioner")?;
                let p = usize::from(u16::from_le_bytes(self.array::<2>().field("Partitioner")?));
                let k = count(self, "Partitioner count", 2)?;
                self.take(k.saturating_mul(2).saturating_add(28))
                    .field("Partitioner")?;
                for _ in 0..=p {
                    object(self, "PartNode", 0x5374_195C)?;
                    self.take(12).field("PartNode")?;
                }
                self.take(1).field("GuidanceSystem")?;
            }
            // HoudiniDecalInstancerComponent: 212 bytes, an object, R references, S items (00, an object, 3 bytes, a
            // count k, 100 k bytes), 46 bytes
            // StaticDDecalComponent (0x5B413085) and its subclass HoudiniDecalInstancerComponent, which stores nothing more:
            // the same body
            0x4148_CD56 | 0x5B41_3085 => {
                self.take(212).field("HoudiniDecalInstancerComponent")?;
                object(self, "HoudiniDecalInstancerComponent", 0xB43F_ECF1)?;
                let r = count(self, "HoudiniDecalInstancerComponent references", 10)?;
                self.take(r.saturating_mul(10))
                    .field("HoudiniDecalInstancerComponent references")?;
                let items = count(self, "HoudiniDecalInstancerComponent items", 20)?;
                for _ in 0..items {
                    expect(self, "HoudiniDecalInstancerComponent item", 0, 1)?;
                    object(self, "HoudiniDecalInstancerComponent item", 0x1ADB_43B0)?;
                    self.take(3).field("HoudiniDecalInstancerComponent item")?;
                    let k = count(self, "HoudiniDecalInstancerComponent item count", 100)?;
                    self.take(k.saturating_mul(100))
                        .field("HoudiniDecalInstancerComponent item")?;
                }
                self.take(46).field("HoudiniDecalInstancerComponent")?;
            }
            // StartingGridComponent: 4 bytes, a count n, n 78-byte objects (GameplayPositionWithLocal: the notes'
            // 8 + 78 n on 1,362 of 1,362)
            0x1725_EC54 => {
                self.take(4).field("component")?;
                let n = count(self, "component count", 78)?;
                self.take(n.saturating_mul(78)).field("component")?;
            }
            // QuestInteractiveComponent: 89 bytes, a null handle 03, an EventListener's 13 bytes (the notes: 532 of 532; a 120-byte form with
            // another byte there is seen 4 times, without a rule)
            0x733E_6F5A => {
                self.take(89).field("component")?;
                if self.u8().field("component handle")? != 3 {
                    return Ok(None);
                }
                self.take(13).field("component")?;
            }
            // UICheckpointController: 51 bytes (the notes: the next component's header exactly there on 18,259 of
            // 18,259)
            0x4CB6_2343 => {
                self.take(51).field("UICheckpointController")?;
            }
            // CheckpointComponent: 4 bytes, a count k, k inline objects (00, a u64, a type, its body), 226 bytes; each
            // object's size by its type, observed on every body measured (the notes: 18,085 of 18,395 groups holding
            // it then parse to their end); a type outside the table has no rule
            0x69D5_E169 => {
                self.take(4).field("CheckpointComponent")?;
                let k = count(self, "CheckpointComponent count", 41)?;
                for _ in 0..k {
                    expect(self, "CheckpointComponent object", 0, 1)?;
                    self.u64(LE).field("CheckpointComponent object")?;
                    let size: usize = match self.u32(LE).field("CheckpointComponent object type")? {
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
                    self.take(size.saturating_sub(13))
                        .field("CheckpointComponent object")?;
                }
                self.take(226).field("CheckpointComponent")?;
            }
            // FakeWaterMeshComponent: 4 bytes, a reference (its FakeWaterComponentChunk), a FakeWaterMeshInstance
            // (00, an object, a count k and 64 k bytes), a bounding volume, 10 bytes, the 125-byte Visual tail, 3 vec4,
            // a u32, a vertex count V, a u32, V 8-byte vertices (4 f16), an index count I, a u32, I u16 indices (the
            // notes: 274 + 8 V + 2 I, the one sequential end on 512 of 512)
            0x961F_3B5D => {
                self.take(14).field("FakeWaterMeshComponent")?;
                expect(self, "FakeWaterMeshInstance", 0, 1)?;
                object(self, "FakeWaterMeshInstance", 0xC345_7872)?;
                let k = count(self, "FakeWaterMeshInstance count", 64)?;
                self.take(k.saturating_mul(64)).field("FakeWaterMeshInstance")?;
                self.take(40 + 10 + 125 + 48 + 4)
                    .field("FakeWaterMeshComponent")?;
                let v = count(self, "FakeWaterMeshComponent vertex count", 8)?;
                self.take(4).field("FakeWaterMeshComponent")?;
                self.take(v.saturating_mul(8))
                    .field("FakeWaterMeshComponent vertices")?;
                let i = count(self, "FakeWaterMeshComponent index count", 2)?;
                self.take(4).field("FakeWaterMeshComponent")?;
                self.take(i.saturating_mul(2))
                    .field("FakeWaterMeshComponent indices")?;
            }
            // AutoFakeOrchestratorComponent: 4 bytes, n SubMeshFakeIndexSpans (an object, a count m, m FakeIndexSpans
            // of an object and two u32), a count p and 8 p bytes, 9 bytes (the notes: the one sequential end on 4,052
            // of 4,052)
            0xD4B4_4556 => {
                self.take(4).field("AutoFakeOrchestratorComponent")?;
                let n = count(self, "AutoFakeOrchestratorComponent count", 16)?;
                for _ in 0..n {
                    object(self, "SubMeshFakeIndexSpans", 0x2AFD_2E35)?;
                    let m = count(self, "SubMeshFakeIndexSpans count", 20)?;
                    for _ in 0..m {
                        object(self, "FakeIndexSpan", 0xE747_72BA)?;
                        self.take(8).field("FakeIndexSpan")?;
                    }
                }
                let p = count(self, "AutoFakeOrchestratorComponent run", 8)?;
                self.take(p.saturating_mul(8))
                    .field("AutoFakeOrchestratorComponent run")?;
                self.take(9).field("AutoFakeOrchestratorComponent")?;
            }
            // DominoComponent: 4 bytes, a u32, an AIScriptInstance (00, an object), 2 bytes, a reference (its script),
            // a count n, n parameters, 21 bytes (the notes: 2,384 of 2,626 bodies end there); a parameter's value by its
            // kind (W's low byte), a value kind or embedded class outside the rule has none
            0xBE71_1F06 => {
                self.take(8).field("DominoComponent")?;
                expect(self, "AIScriptInstance", 0, 1)?;
                object(self, "AIScriptInstance", 0x5EFD_B48C)?;
                self.take(12).field("DominoComponent")?;
                let n = count(self, "DominoComponent parameter count", 82)?;
                for _ in 0..n {
                    expect(self, "DominoScriptInstanceParameter", 0, 1)?;
                    object(self, "DominoScriptInstanceParameter", 0xEE14_E8BE)?;
                    expect(self, "DominoDataProviderData", 0, 1)?;
                    object(self, "DominoDataProviderData", 0x295E_9B7D)?;
                    object(self, "DominoDynamic", 0x1A24_BE34)?;
                    expect(self, "DominoDynamic", 3, 4)?;
                    object(self, "DominoType", 0x86F5_9438)?;
                    let kind = self.u32(LE).field("DominoType kind")? & 0xFF;
                    let class = self.u32(LE).field("DominoType class")?;
                    self.take(4).field("DominoType")?;
                    expect(self, "Value", 0xDCB6_7730, 4)?;
                    expect(self, "Value class", u64::from(class), 4)?;
                    self.take(8).field("Value")?;
                    match kind {
                        0x1C => {
                            self.take(10).field("Value reference")?;
                        }
                        0x12 => {
                            expect(self, "Value object", 0, 1)?;
                            self.u64(LE).field("Value object")?;
                        }
                        0x1D => {
                            let m = count(self, "Value list", 9)?;
                            for _ in 0..m {
                                expect(self, "Value list item", 0, 1)?;
                                self.u64(LE).field("Value list item")?;
                            }
                        }
                        0x00 => {
                            self.take(1).field("Value")?;
                        }
                        0x16 => {
                            self.u64(LE).field("Value embedded object")?;
                            let size = match self.u32(LE).field("Value embedded class")? {
                                0x7E42_F87F => 52,
                                0xB7DE_E7C7 => 84,
                                _ => return Ok(None),
                            };
                            self.take(size).field("Value embedded object")?;
                        }
                        _ => return Ok(None),
                    }
                    self.take(13).field("DominoScriptInstanceParameter")?;
                }
                self.take(21).field("DominoComponent")?;
            }
            // WaterComponent: 4 bytes, a reference (its WaterMesh), a WaterMeshInstance (00, an object, a count k and
            // 64 k bytes), a bounding volume, 10 bytes, the 125-byte Visual tail, a RiverQuadTree (an object, 2 vec4, a
            // byte, n 61-byte nodes), a byte, m 49-byte WaterPatchDescs, 3 references (the notes: 290 + 61 n + 49 m, the
            // one sequential end on 36 of 36)
            0x996D_F94F => {
                self.take(14).field("WaterComponent")?;
                expect(self, "WaterMeshInstance", 0, 1)?;
                object(self, "WaterMeshInstance", 0xC15B_678F)?;
                let k = count(self, "WaterMeshInstance count", 64)?;
                self.take(k.saturating_mul(64)).field("WaterMeshInstance")?;
                self.take(40 + 10 + 125).field("WaterComponent")?;
                object(self, "RiverQuadTree", 0x92DB_01AE)?;
                self.take(33).field("RiverQuadTree")?;
                let n = count(self, "RiverQuadTree node count", 61)?;
                self.take(n.saturating_mul(61)).field("RiverQuadTreeNode")?;
                self.take(1).field("WaterComponent")?;
                let m = count(self, "WaterPatchDesc count", 49)?;
                self.take(m.saturating_mul(49)).field("WaterPatchDesc")?;
                self.take(30).field("WaterComponent")?;
            }
            // AnimatedEntityComponent: 4 bytes, a count n, n 66-byte AnimationParameters, a byte (the notes: 9 + 66 n,
            // the one sequential end or the next component's header on 2,041 of 2,041)
            0x2996_CA50 => {
                self.take(4).field("AnimatedEntityComponent")?;
                let n = count(self, "AnimatedEntityComponent count", 66)?;
                self.take(n.saturating_mul(66)).field("AnimationParameters")?;
                self.take(1).field("AnimatedEntityComponent")?;
            }
            // 0xE139F584 (unnamed): 4 bytes, an EventListener (an object, a byte, a count n: only 0 read, 13 bytes), a
            // flag byte, k ids (01, a u64), 9 bytes, a SoundInstance (00, an object, 52 bytes) (the notes: 113 + 9 k,
            // the one sequential end or the next component's header on 1,990 of 2,014; the rest hold objects in the
            // EventListener, without a rule)
            0xE139_F584 => {
                self.take(4).field("component")?;
                object(self, "EventListener", 0x2E8B_5553)?;
                self.take(1).field("EventListener")?;
                if self.u32(LE).field("EventListener count")? != 0 {
                    return Ok(None);
                }
                self.take(14).field("EventListener")?;
                let k = count(self, "component ids", 9)?;
                self.take(k.saturating_mul(9)).field("component ids")?;
                self.take(9).field("component")?;
                expect(self, "SoundInstance", 0, 1)?;
                object(self, "SoundInstance", 0x7E42_F87F)?;
                self.take(52).field("SoundInstance")?;
            }
            // BulkComponent (crowds): 4 bytes, BulkSettings (an object, a u32), a BulkRandomCompositionTemplate (00, an
            // object, an f32), t 36-byte BulkTemplates, a u32, an f32, c 45-byte members, 4 bytes, a 04-tagged object
            // and a byte, k BulkStates (an object, 9 bytes, a reference), 37 bytes, k SoundInstances (an object, 52
            // bytes), f f32, 17 bytes (the notes: the one sequential end on 2,481 of 2,481)
            0x3083_58A4 => {
                self.take(4).field("BulkComponent")?;
                object(self, "BulkSettings", 0x4404_F7CC)?;
                self.take(4).field("BulkSettings")?;
                expect(self, "BulkRandomCompositionTemplate", 0, 1)?;
                object(self, "BulkRandomCompositionTemplate", 0x3E7C_FA88)?;
                self.take(4).field("BulkRandomCompositionTemplate")?;
                let t = count(self, "BulkTemplate count", 36)?;
                for _ in 0..t {
                    expect(self, "BulkTemplate", 0, 1)?;
                    object(self, "BulkTemplate", 0x3621_488A)?;
                    self.take(14).field("BulkTemplate")?;
                    expect(self, "BulkTemplate", 0, 1)?;
                    self.take(8).field("BulkTemplate")?;
                }
                self.take(8).field("BulkComponent")?;
                let c = count(self, "BulkComponent member count", 45)?;
                for _ in 0..c {
                    object(self, "BulkComponent member", 0x7A98_3CEC)?;
                    self.take(33).field("BulkComponent member")?;
                }
                self.take(4).field("BulkComponent")?;
                expect(self, "BulkComponent object", 4, 1)?;
                object(self, "BulkComponent object", 0xEACA_451C)?;
                self.take(1).field("BulkComponent")?;
                let k = count(self, "BulkState count", 31)?;
                for _ in 0..k {
                    object(self, "BulkState", 0x7888_7331)?;
                    self.take(19).field("BulkState")?;
                }
                self.take(37).field("BulkComponent")?;
                for _ in 0..k {
                    object(self, "SoundInstance", 0x7E42_F87F)?;
                    self.take(52).field("SoundInstance")?;
                }
                let f = count(self, "BulkComponent floats", 4)?;
                self.take(f.saturating_mul(4)).field("BulkComponent floats")?;
                self.take(17).field("BulkComponent")?;
            }
            // FXComponent: 4 bytes, an object, 6 bytes, a reference (its FX), an f32, an object, a u32, n parameters
            // (a u32 id, a u32, a u16, a u16 kind, a u32, the value by its kind: 0x0A an f32, 0x0D a vec4; another
            // kind has no rule), 11 bytes (the notes: 67 + the parameters', the one sequential end or the next
            // component's header on every one of the 1,089 bodies reached)
            0xD2BF_93A6 => {
                self.take(4).field("FXComponent")?;
                object(self, "FXComponent", 0x1159_8A66)?;
                self.take(20).field("FXComponent")?;
                object(self, "FXComponent", 0x32D3_3700)?;
                self.take(4).field("FXComponent")?;
                let n = count(self, "FXComponent parameter count", 16)?;
                for _ in 0..n {
                    self.take(10).field("FXComponent parameter")?;
                    let kind = u16::from_le_bytes(self.array::<2>().field("FXComponent parameter kind")?);
                    self.take(4).field("FXComponent parameter")?;
                    match kind {
                        0x0A => self.take(4).field("FXComponent parameter value")?,
                        0x0D => self.take(16).field("FXComponent parameter value")?,
                        _ => return Ok(None),
                    };
                }
                self.take(11).field("FXComponent")?;
            }
            // 0x341877D6 (unnamed): 4 bytes, a count n, n 109-byte items, each from a 04-tagged 0x30C186C4 object (the
            // notes: 8 + 109 n, the one sequential end on 190 of 190; the rest of an item, 0x518AFC74 and IsLocked
            // objects with references, not named)
            0x3418_77D6 => {
                self.take(4).field("component")?;
                let n = count(self, "component count", 109)?;
                for _ in 0..n {
                    expect(self, "component item", 4, 1)?;
                    object(self, "component item", 0x30C1_86C4)?;
                    self.take(96).field("component item")?;
                }
            }
            // 0xEEF3F11F (unnamed): 9 bytes (the notes: 00 01 00 00, a u32 0, 03 on 483 of 483)
            0xEEF3_F11F => {
                self.take(9).field("component")?;
            }
            // BreakableComponent: 4 bytes, a u32 (only 0 read), k ids (01, a u64), m ids, a 102-byte audio blocks (an
            // AudioEventSeed object first: 00, a u64, 0x561E06FC), 20 bytes (the notes: k 3, m 1, a 1 on all 1,070,
            // 178 bytes, the next component's header there on 1,070 of 1,070; other counts read as counts, not seen)
            0x44F8_FD99 => {
                self.take(4).field("BreakableComponent")?;
                if self.u32(LE).field("BreakableComponent")? != 0 {
                    return Ok(None);
                }
                for what in ["BreakableComponent ids", "BreakableComponent ids"] {
                    let k = count(self, what, 9)?;
                    for _ in 0..k {
                        expect(self, what, 1, 1)?;
                        self.u64(LE).field(what)?;
                    }
                }
                let a = count(self, "BreakableComponent audio count", 102)?;
                for _ in 0..a {
                    expect(self, "AudioEventSeed", 0, 1)?;
                    object(self, "AudioEventSeed", 0x561E_06FC)?;
                    self.take(89).field("BreakableComponent audio")?;
                }
                self.take(20).field("BreakableComponent")?;
            }
            // GameplayContextDisablerComponent: 4 bytes, a condition tree, m (a byte 00 or 01, a u64) (the notes: the
            // one sequential end on 99 of 99 measured, the next component's header on the other 2,216 of 2,315)
            0xBB7F_5B1D => {
                self.take(4).field("GameplayContextDisablerComponent")?;
                if !self.condition(0)? {
                    return Ok(None);
                }
                let m = count(self, "GameplayContextDisablerComponent count", 9)?;
                for _ in 0..m {
                    let what = "GameplayContextDisablerComponent item";
                    let at = self.position();
                    let found = self.u8().field(what)?;
                    if found > 1 {
                        return Err(BodyError::Value {
                            what,
                            at,
                            found: u64::from(found),
                            read: "0 or 1",
                        }
                        .into());
                    }
                    self.u64(LE).field(what)?;
                }
            }
            // OutOfBoundsVisualComponent (the notes: 169 on 716 of 716)
            0x2994_1A30 => {
                self.take(169).field("component")?;
            }
            // fixed sizes, observed on every body measured
            0x5139_1C93 => {
                self.take(12).field("component")?;
            }
            0x7F0A_4643 => {
                self.take(14).field("component")?;
            }
            0x2AA1_79AB => {
                self.take(17).field("component")?;
            }
            0xADF1_D5E6 => {
                self.take(450).field("component")?;
            }
            0xA8AB_FC79 => {
                self.take(46).field("component")?;
            }
            0x064C_1BFA => {
                self.take(24).field("component")?;
            }
            0x8167_852B => {
                self.take(426).field("component")?;
            }
            0xA703_3693 => {
                self.take(9).field("component")?;
            }
            _ => return Ok(None),
        }
        Ok(Some(
            self.bytes()
                .get(start..self.position())
                .unwrap_or_default()
                .to_vec(),
        ))
    }
}

impl StoredEntity {
    /// The body parsed. Its components are read by their types' rules where every type has one and the tail then ends
    /// the body. Otherwise they are delimited by their headers: the tail is found from the first component (the
    /// bounding volume's and descriptor's types where it puts them, then the rest of the body as the tail), and the
    /// headers between are the components' (a tag, an id and the type of a component class, `is_component_type`; where
    /// more than the count say, those with the first component's tag, or an id near the first's, as embedded objects
    /// begin alike), exactly as many as the count. A component of a type with a rule is then read by it over its
    /// extent; the others, and any whose rule ends elsewhere, are [`ComponentBody::Unread`]. On the six forges of
    /// Riders Republic every one of the 615,110 Entities and 56,837 EntityGroups reads so.
    ///
    /// `is_component_type` is the game's class registry as the caller has it: whether a u32 is the type of one of its
    /// component classes (the CRC-32 of the class name). Only bodies with a component that has no length rule here
    /// consult it.
    pub fn parse(body: &[u8], is_component_type: impl Fn(u32) -> bool) -> Result<StoredEntity, Error> {
        let mut c = Cursor::new(body, &is_component_type);
        let e = entity(&mut c, false)?;
        end(&c)?;
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
            ComponentBody::StaticDecal(_)
            | ComponentBody::RigidBody { .. }
            | ComponentBody::GrassVisual(_)
            | ComponentBody::GrassSetup(_)
            | ComponentBody::Skeleton(_)
            | ComponentBody::Anim(_)
            | ComponentBody::Raw(_)
            | ComponentBody::Unread(_)
            | ComponentBody::Refused { .. } => None,
        })
    }

    /// Its run's bool at `at` (the registry's Entity bit-field group in order: docs/placement.md §19, entity-grammar
    /// §8.26) set.
    fn run_bool(&self, at: usize) -> bool {
        self.run.get(at).is_some_and(|&b| b != 0)
    }
    /// `IsMovable`: its run's byte 2.
    pub fn is_movable(&self) -> bool {
        self.run_bool(2)
    }
    /// `IsPhantom`: its run's byte 3 (on 3,834 Entities of Riders Republic, every one a UI or HUD widget, a
    /// checkpoint letter, a collectible balloon or an editor helper: docs/placement.md §19.1).
    pub fn is_phantom(&self) -> bool {
        self.run_bool(3)
    }
    /// `IsHidden`: its run's byte 4 (on 200, all UI).
    pub fn is_hidden(&self) -> bool {
        self.run_bool(4)
    }
}

/// An EntityGroup body as stored: an Entity, then its members.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEntityGroup {
    pub entity: StoredEntity,
    /// Its member references, in stored order.
    pub members: Vec<Reference>,
    /// The ids of the links after the members (each a `00` and a u64), in stored order: the members' own ids in
    /// another order on the groups measured that have any (the UI and sequence groups).
    pub links: Vec<u64>,
    /// The u32 ending the body, as stored.
    pub word: u32,
}

impl StoredEntityGroup {
    /// The body parsed: the Entity grammar, a u32 count c, c references (none of them `03`), a u32 count m, m links
    /// (`00` and a u64) and a u32. `is_component_type` is the game's class registry, as for [`StoredEntity::parse`].
    pub fn parse(body: &[u8], is_component_type: impl Fn(u32) -> bool) -> Result<StoredEntityGroup, Error> {
        let mut c = Cursor::new(body, &is_component_type);
        let entity = entity(&mut c, true)?;
        let (members, links, word) = c.members()?;
        end(&c)?;
        Ok(StoredEntityGroup {
            entity,
            members,
            links,
            word,
        })
    }
}

/// An Entity's tail: what follows its components.
struct EntityTail {
    run: [u8; 43],
    word: u32,
    scale: f32,
    bounds: BoundingVolume,
    descriptor: [u8; 13],
    data_layers: Vec<(u64, u32)>,
    links: Vec<u64>,
    tail: Tail,
}

/// The components and the tail read by the types' rules, or the index and type of the first component with no rule.
type ByRules = Result<(Vec<Component>, EntityTail), (usize, u32)>;

/// The bytes of a component's header: a tag, a u64 id and a u32 type.
const COMPONENT_HEADER: usize = 13;
/// Where an Entity's bounding volume's type lies after its components (the 43-byte run, a u32 and the scale, then the
/// volume's id), and where its EntityDescriptor's (the volume's type, six f32 and a u32, then the descriptor's id).
const TAIL_BOUNDS_TYPE: usize = 43 + 4 + 4 + 8;
const TAIL_DESCRIPTOR_TYPE: usize = TAIL_BOUNDS_TYPE + 4 + 24 + 4 + 8;
/// An EntityDescriptor's type.
const ENTITY_DESCRIPTOR: u32 = 0x6012_1A9E;

impl Cursor<'_> {
    /// A component's body over the extent that ends at `end`, for the types read only so (no length rule); `None`
    /// where the type has none or the body does not read to `end`.
    fn by_extent(&mut self, kind: u32, end: usize) -> Option<Result<ComponentBody, Refusal>> {
        let ends = |at: usize| {
            if at == end {
                Ok(())
            } else {
                Err(Refusal::Ends { at, end })
            }
        };
        Some(match kind {
            GRASS_COMPONENT => self.grass_setup(end).map_err(Refusal::Rule).and_then(|g| {
                ends(self.position())?;
                Ok(ComponentBody::GrassSetup(Box::new(g)))
            }),
            SKELETON_COMPONENT => self.skeleton_binding().map_err(Refusal::Rule).and_then(|b| {
                ends(self.position())?;
                Ok(ComponentBody::Skeleton(Box::new(b)))
            }),
            ANIM_COMPONENT => self
                .anim_binding(end)
                .map(|b| ComponentBody::Anim(Box::new(b)))
                .map_err(Refusal::Rule),
            _ => return None,
        })
    }

    /// A link's tag byte, `00` or `01`.
    fn link_tag(&mut self, what: &'static str) -> Result<(), Error> {
        let at = self.position();
        match self.u8().field(what)? {
            0 | 1 => Ok(()),
            found => Err(BodyError::Value {
                what,
                at,
                found: u64::from(found),
                read: "0 or 1",
            }
            .into()),
        }
    }

    /// A component's header: its tag (4, or 0), id and type.
    fn component_header(&mut self) -> Result<(u8, u64, u32), Error> {
        let at = self.position();
        let tag = self.u8().field("component tag")?;
        if !matches!(tag, 0 | 4) {
            return Err(BodyError::Value {
                what: "component tag",
                at,
                found: u64::from(tag),
                read: "4 or 0",
            }
            .into());
        }
        Ok((
            tag,
            self.u64(LE).field("component")?,
            self.u32(LE).field("component type")?,
        ))
    }

    /// A component's body by its type's rule, from the cursor (`start`); `None` where the type has no rule (the
    /// cursor then is wherever the rule stopped).
    fn component_body(&mut self, kind: u32, start: usize) -> Result<Option<ComponentBody>, Error> {
        Ok(Some(match kind {
            VISUAL => ComponentBody::Visual(self.visual()?),
            STATIC_DECAL => ComponentBody::StaticDecal(self.visual()?),
            GRASS_VISUAL => ComponentBody::GrassVisual(self.grass_visual()?),
            RIGID_BODY => {
                let Some(bytes) = self.raw_component(kind)? else {
                    return Ok(None);
                };
                let at = start.saturating_add(SHAPE_AT);
                let shape = View::with_origin(&bytes, start)
                    .array::<{ reference::LENGTH }>(SHAPE_AT)
                    .field("RigidBodyComponent shape")?;
                let shape = reference::parse(shape).map_err(|source| BodyError::Reference { at, source })?;
                ComponentBody::RigidBody { shape, bytes }
            }
            _ => match self.raw_component(kind)? {
                Some(bytes) => ComponentBody::Raw(bytes),
                None => return Ok(None),
            },
        }))
    }

    /// The u32 at `at`, if the body holds it.
    fn word_at(&self, at: usize) -> Option<u32> {
        self.bytes()
            .get(at..at.checked_add(4)?)
            .and_then(|w| <[u8; 4]>::try_from(w).ok())
            .map(u32::from_le_bytes)
    }

    /// Whether a component's header lies at `at`: a tag 4 or 0 and the type of a component class.
    fn is_component_header(&self, at: usize) -> bool {
        self.bytes().get(at).is_some_and(|&t| matches!(t, 0 | 4))
            && at
                .checked_add(9)
                .and_then(|w| self.word_at(w))
                .is_some_and(self.types)
    }

    /// An Entity's tail from the cursor.
    fn entity_tail(&mut self) -> Result<EntityTail, Error> {
        let run = self.array::<43>().field("run")?;
        let word = self.u32(LE).field("word before the scale")?;
        let scale = self.f32(LE).field("scale")?;
        let bounds = self.bounds()?;
        object(self, "EntityDescriptor", ENTITY_DESCRIPTOR)?;
        let descriptor = self.array::<13>().field("EntityDescriptor")?;
        object(self, "DataLayerFilter", 0xDB1D_406E)?;
        let k = count(self, "DataLayerAction count", 25)?;
        let mut data_layers = Vec::with_capacity(k);
        for _ in 0..k {
            object(self, "DataLayerAction", 0xE315_93E1)?;
            expect(self, "DataLayerAction", 0, 1)?;
            data_layers.push((
                self.u64(LE).field("DataLayerAction")?,
                self.u32(LE).field("DataLayerAction")?,
            ));
        }
        let m = count(self, "DataLayerFilter link count", 9)?;
        let mut links = Vec::with_capacity(m);
        for _ in 0..m {
            self.link_tag("DataLayerFilter link")?;
            links.push(self.u64(LE).field("DataLayerFilter link")?);
        }
        let tail = match self.u8().field("tail")? {
            3 => Tail::End,
            0 => {
                let id = object(self, "GameStateData", 0x7313_743E)?;
                let word = self.u32(LE).field("GameStateData")?;
                let l1 = count(self, "GameStateData first run", 1)?;
                let first = self.take(l1).field("GameStateData first run")?.to_vec();
                let l2 = count(self, "GameStateData second run", 1)?;
                let second = self.take(l2).field("GameStateData second run")?.to_vec();
                Tail::GameState {
                    id,
                    word,
                    first,
                    second,
                }
            }
            found => {
                return Err(BodyError::Value {
                    what: "tail",
                    at: self.position().saturating_sub(1),
                    found: u64::from(found),
                    read: "3 or 0",
                }
                .into());
            }
        };
        Ok(EntityTail {
            run,
            word,
            scale,
            bounds,
            descriptor,
            data_layers,
            links,
            tail,
        })
    }

    /// An EntityGroup's members from the cursor: a count c, c references (none of them `03`), a count m, m links (`00`
    /// and a u64) and a u32.
    fn members(&mut self) -> Result<(Vec<Reference>, Vec<u64>, u32), Error> {
        let n = count(self, "member count", reference::LENGTH)?;
        let mut members = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.position();
            members.push(reference(self)?.ok_or(BodyError::Value {
                what: "member reference",
                at,
                found: 3,
                read: "a reference (tag 1)",
            })?);
        }
        let m = count(self, "member link count", 9)?;
        let mut links = Vec::with_capacity(m);
        for _ in 0..m {
            self.link_tag("member link")?;
            links.push(self.u64(LE).field("member link")?);
        }
        let word = self.u32(LE).field("last word")?;
        Ok((members, links, word))
    }

    /// Where an Entity's tail begins, searching from `from`: the first place where the bounding volume's and the
    /// descriptor's types lie where the tail puts them and the tail, then a group's members, end the body.
    fn tail_start(&self, from: usize, group: bool) -> Option<usize> {
        let last = self
            .bytes()
            .len()
            .checked_sub(TAIL_DESCRIPTOR_TYPE.saturating_add(4))?;
        (from..=last).find(|&at| {
            self.word_at(at.saturating_add(TAIL_BOUNDS_TYPE)) == Some(BOUNDING_VOLUME)
                && self.word_at(at.saturating_add(TAIL_DESCRIPTOR_TYPE)) == Some(ENTITY_DESCRIPTOR)
                && {
                    let mut tail = Cursor::new(self.bytes(), self.types);
                    tail.seek(at).is_ok()
                        && tail.entity_tail().is_ok()
                        && (!group || tail.members().is_ok())
                        && end(&tail).is_ok()
                }
        })
    }

    /// The u64 at `at`, if the body holds it.
    fn u64_at(&self, at: usize) -> Option<u64> {
        self.bytes()
            .get(at..at.checked_add(8)?)
            .and_then(|w| <[u8; 8]>::try_from(w).ok())
            .map(u64::from_le_bytes)
    }

    /// The Entity's `count` components, from the cursor (just after the count), delimited by their headers instead
    /// of read by their types' rules: the tail is found from the cursor, and the components' headers (the first one's tag, an
    /// number (see [`component_number`]) near the first component's and the type of a component class) between are taken,
    /// and there must be exactly `count`. Each component of a type with a rule is read by
    /// it over its extent and kept where the rule ends at the extent's end; any other, and any one whose rule ends
    /// elsewhere, keeps its bytes as stored ([`ComponentBody::Unread`]). The cursor ends at the tail. `blocked` is
    /// what stopped the reading by rules (the index of the component and its type), for the refusal.
    fn delimited_components(
        &mut self,
        count: usize,
        group: bool,
        blocked: (usize, u32),
    ) -> Result<Vec<Component>, Error> {
        let (index, kind) = blocked;
        let first = self.component_header()?;
        let first_end = self.position();
        let end = self
            .tail_start(first_end, group)
            .ok_or(Error::NoTail { index, kind })?;
        let mut heads: Vec<usize> = (first_end..end)
            .filter(|&at| {
                at.saturating_add(COMPONENT_HEADER) <= end
                    && self.is_component_header(at)
                    && self
                        .u64_at(at.saturating_add(1))
                        .and_then(component_number)
                        .is_some_and(|n| n > COMPONENT_NUMBER_LOW)
            })
            .collect();
        // more than the count says: embedded objects begin like components. Each of the first component's tag (embedded
        // objects have `00`) and of an id near the first's (a payload's bytes have another) narrows them, alone or
        // together, to the count where they can
        if heads.len().saturating_add(1) > count {
            let tag = |at: &usize| self.bytes().get(*at) == Some(&first.0);
            let near = |at: &usize| {
                let number = |id| component_number(id);
                self.u64_at(at.saturating_add(1))
                    .and_then(number)
                    .zip(number(first.1))
                    .is_some_and(|(n, f)| n.abs_diff(f) < COMPONENT_NUMBER_SPAN)
            };
            let want = count.saturating_sub(1);
            let by_tag: Vec<usize> = heads.iter().copied().filter(tag).collect();
            let by_near: Vec<usize> = heads.iter().copied().filter(near).collect();
            let both: Vec<usize> = by_tag.iter().copied().filter(near).collect();
            heads = [by_tag, by_near, both]
                .into_iter()
                .find(|narrowed| narrowed.len() == want)
                .unwrap_or(heads);
        }
        if heads.len().saturating_add(1) != count {
            return Err(Error::Headers {
                index,
                kind,
                found: heads.len(),
                want: count.saturating_sub(1),
            });
        }
        let mut out = Vec::with_capacity(count);
        let mut header = first;
        let mut body_start = first_end;
        for next in heads.iter().map(|&h| Some(h)).chain([None]) {
            let body_end = next.unwrap_or(end);
            let (tag, id, kind) = header;
            let bytes = Cursor::at(self.bytes(), body_start, "component", self.types)?
                .take_to(body_end)
                .field("component")?;
            // the component read within its extent
            let within = self.bytes().get(..body_end).unwrap_or(self.bytes());
            let mut part = Cursor::at(within, body_start, "component", self.types)?;
            let body = match part.component_body(kind, body_start) {
                Ok(Some(body)) if part.position() == body_end => body,
                _ => match Cursor::at(within, body_start, "component", self.types)?.by_extent(kind, body_end)
                {
                    Some(Ok(body)) => body,
                    Some(Err(why)) => ComponentBody::Refused {
                        bytes: bytes.to_vec(),
                        why,
                    },
                    None => ComponentBody::Unread(bytes.to_vec()),
                },
            };
            out.push(Component { tag, id, kind, body });
            if let Some(h) = next {
                let mut head = Cursor::at(self.bytes(), h, "component header", self.types)?;
                header = head.component_header()?;
                body_start = head.position();
            }
        }
        self.seek(end).field("component")?;
        Ok(out)
    }

    /// The Entity's `count` components read by their types' rules, from the cursor, and its tail: the inner `Err` has
    /// the index and type of the first component whose type has no rule. Where a rule reads wrongly, the tail after
    /// it doesn't end the body (or the group's members after the tail don't), which is refused as no rule is.
    fn by_rules(&mut self, count: usize, group: bool) -> Result<ByRules, Error> {
        let mut components = Vec::with_capacity(count);
        while components.len() < count {
            let (tag, id, kind) = self.component_header()?;
            let start = self.position();
            match self.component_body(kind, start)? {
                Some(body) => components.push(Component { tag, id, kind, body }),
                None => return Ok(Err((components.len(), kind))),
            }
        }
        let tail = self.entity_tail()?;
        let mut rest = Cursor::at(self.bytes(), self.position(), "Entity tail", self.types)?;
        if group {
            rest.members()?;
        }
        end(&rest)?;
        Ok(Ok((components, tail)))
    }
}

/// A component id as the 41-bit number it carries, where it is one: below 2^48 as it is, or with bit 63 set over the
/// number shifted left 20 bits and a small index below it (`0x9D5F4526B210000D` for `0x1D5F4526B21`, the 14th), as a
/// few components store it.
fn component_number(id: u64) -> Option<u64> {
    if id >> 48 == 0 {
        Some(id)
    } else if id >> 63 == 1 {
        Some((id & (u64::MAX >> 1)) >> 20)
    } else {
        None
    }
}
/// The least a component's number exceeds, and how far from its Entity's first component's it lies where there are more
/// headers than components: measured on every body read by rules (the least is above 2^33; a few components' numbers
/// differ from the first's by more than 2^40, which is why this narrows and never refuses). Bytes that look like a
/// header and lie outside them are a payload's (a field naming a component class after a zero tag and a small value).
const COMPONENT_NUMBER_LOW: u64 = 1 << 32;
const COMPONENT_NUMBER_SPAN: u64 = 1 << 40;
/// A Hierarchy's type (the inline object after an Entity's first byte), and a HierarchyHandle's.
const HIERARCHY: u32 = 0x788B_AA0D;
const HIERARCHY_HANDLE: u32 = 0x709F_B9D4;

/// An Entity's grammar from its first byte through its tail; `group` where the Entity is an EntityGroup's (its
/// members follow the tail).
fn entity(c: &mut Cursor<'_>, group: bool) -> Result<StoredEntity, Error> {
    expect(c, "first byte", 1, 1)?;
    let at = c.position();
    let hierarchy = match c.u8().field("second byte")? {
        3 => None,
        0 => {
            let start = c.position();
            object(c, "Hierarchy", HIERARCHY)?;
            c.take(64).field("Hierarchy")?;
            object(c, "HierarchyHandle", HIERARCHY_HANDLE)?;
            c.take(4 + 9).field("HierarchyHandle")?;
            Some(c.bytes().get(start..c.position()).unwrap_or_default().to_vec())
        }
        found => {
            return Err(BodyError::Value {
                what: "second byte",
                at,
                found: u64::from(found),
                read: "3 or 0",
            }
            .into());
        }
    };
    let matrix = c.matrix()?;
    let n = count(c, "component count", 13)?;
    let start = c.position();
    // the types' rules where they all read and the tail then ends the body, else the components' own headers delimit
    // them
    let (components, t) = match c.by_rules(n, group) {
        Ok(Ok(read)) => read,
        stopped => {
            let blocked = match &stopped {
                Ok(Err(blocked)) => *blocked,
                _ => (0, 0),
            };
            c.seek(start).field("Entity components")?;
            let components = c.delimited_components(n, group, blocked)?;
            (components, c.entity_tail()?)
        }
    };
    Ok(StoredEntity {
        matrix,
        components,
        run: t.run,
        word: t.word,
        scale: t.scale,
        bounds: t.bounds,
        descriptor: t.descriptor,
        data_layers: t.data_layers,
        links: t.links,
        tail: t.tail,
        hierarchy,
    })
}
