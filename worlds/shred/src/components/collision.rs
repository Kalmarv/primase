//! SHRED's Havok rigid bodies beside core's collision: the body's own data, opaque source surface identities and the
//! shapes of bodies that move (those are not static collision and stay out of `core.collision`/`core.collider`).
use primase_core::{
    Collider, Collision, Component, Entity, Geometry, Linear, Resource, Surfaces, World, json::JsonError,
    world::Linearity,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Havok's `hkpMotion::MotionType` of a fixed body (the others in the data are 2, sphere inertia, and 3, box inertia).
pub use havok_formats::packfile::physics::MOTION_FIXED;

/// One `hkpRigidBody` (`shred.rigid_body`), on each entity made from one of its leaf shapes. The entity's transform is
/// the body's; a fixed body's shape is `core.collision`/`core.collider`, any other's a [`DynamicShape`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigidBody {
    pub name: String,
    pub motion_type: u8,
    pub response_type: u8,
    pub friction: f32,
    pub restitution: f32,
    /// The body's collision filter word (its collidable's `collisionFilterInfo`), kept whole: 0, 2, 7, 11, 15 or 18 on
    /// the disc.
    pub collision_filter_info: u32,
}

impl RigidBody {
    pub fn is_fixed(&self) -> bool {
        self.motion_type == MOTION_FIXED
    }
}

impl Component for RigidBody {
    const NAME: &'static str = "shred.rigid_body";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// The shape of a body that is not fixed, in the body's space like `core.collision`/`core.collider`
/// (`shred.dynamic_shape`).
#[derive(Clone, Debug, PartialEq)]
pub enum DynamicShape {
    Soup(Collision),
    Primitive(Collider),
}

impl Component for DynamicShape {
    const NAME: &'static str = "shred.dynamic_shape";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, l: &Linear) -> Linearity<Self> {
        match self {
            Self::Soup(c) => match c.apply_linear(l) {
                Linearity::Applied(c) => Linearity::Applied(Self::Soup(c)),
                Linearity::Invariant => Linearity::Invariant,
                Linearity::Drop => Linearity::Drop,
                Linearity::Unsupported => Linearity::Unsupported,
            },
            Self::Primitive(c) => match c.apply_linear(l) {
                Linearity::Applied(c) => Linearity::Applied(Self::Primitive(c)),
                Linearity::Invariant => Linearity::Invariant,
                Linearity::Drop => Linearity::Drop,
                Linearity::Unsupported => Linearity::Unsupported,
            },
        }
    }
    fn payload_bytes(&self) -> u64 {
        match self {
            Self::Soup(c) => c.payload_bytes(),
            Self::Primitive(c) => c.payload_bytes(),
        }
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        match self {
            Self::Soup(c) => c.verify(e, w),
            Self::Primitive(c) => c.verify(e, w),
        }
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        match self {
            Self::Soup(c) => Ok(Some(
                json!({"soup": {"positions": c.positions.len(), "triangles": c.triangles.len()}}),
            )),
            Self::Primitive(c) => Ok(Some(json!({"primitive": c.to_json()?}))),
        }
    }
}

/// A source material identity. Flag meanings are unknown; no physical behavior is inferred from them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CollisionSurface {
    pub database_id: u16,
    pub database_name: String,
    /// `a & !(0x3F << 3)`: the unmodified bits outside the database surface field.
    pub flags: u32,
}

/// `shred.collision_surfaces`: source identities by `core.surfaces` index. A triangle's existing surface index
/// carries this association through edits of either `core.collision` or [`DynamicShape::Soup`]. Destination passes
/// may rename/classify core surfaces; the original database name and bits stay here. Zero-flag database rows keep
/// their original indices; distinct nonzero `(database_id, flags)` pairs have distinct additional rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CollisionSurfaces(pub BTreeMap<u16, CollisionSurface>);

impl Resource for CollisionSurfaces {
    const NAME: &'static str = "shred.collision_surfaces";
    fn verify(&self, w: &World) -> Result<(), String> {
        let surfaces = w.resource::<Surfaces>().ok_or("missing core.surfaces")?;
        let mut pairs = std::collections::BTreeSet::new();
        for (&index, source) in &self.0 {
            if usize::from(index) >= surfaces.0.len() {
                return Err(format!("source surface {index} past core.surfaces"));
            }
            if source.flags & (0x3f << 3) != 0 || source.database_id > 0x3f {
                return Err(format!("source surface {index} has invalid surface/flag bits"));
            }
            if !pairs.insert((source.database_id, source.flags)) {
                return Err(format!("source surface {index} repeats a material identity"));
            }
        }
        Ok(())
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
