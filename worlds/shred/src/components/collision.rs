//! SHRED's Havok rigid bodies beside core's collision: the body's own data, the opaque per-triangle flag bits and the
//! shapes of bodies that move (those are not static collision and stay out of `core.collision`/`core.collider`).
use primase_core::{
    Collider, Collision, Component, Entity, Geometry, Linear, World, json::JsonError, world::Linearity,
};
use serde::Serialize;
use serde_json::{Value, json};

/// Havok's `hkpMotion::MotionType` of a fixed body (the others in the data are 2, sphere inertia, and 3, box inertia).
pub const MOTION_FIXED: u8 = 5;

/// One `hkpRigidBody` (`shred.rigid_body`), on each entity made from one of its leaf shapes. The entity's transform is
/// the body's; a fixed body's shape is `core.collision`/`core.collider`, any other's a [`DynamicShape`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigidBody {
    pub name: String,
    pub motion_type: u8,
    pub response_type: u8,
    pub friction: f32,
    pub restitution: f32,
    /// Havok's collision filter word, kept whole.
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

/// The bits of each triangle's material word outside the surface id (`a & !(0x3F << 3)`), one per triangle of the
/// entity's `core.collision` or [`DynamicShape::Soup`] (`shred.collision_flags`). Their meanings are not established;
/// they are carried whole and unnamed.
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionFlags(pub Vec<u32>);

impl Component for CollisionFlags {
    const NAME: &'static str = "shred.collision_flags";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        self.0.len() as u64 * 4
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let triangles = match (w.get::<Collision>(e), w.get::<DynamicShape>(e)) {
            (Some(c), None) => c.triangles.len(),
            (None, Some(DynamicShape::Soup(c))) => c.triangles.len(),
            _ => return Err("flags on an entity without exactly one triangle soup".into()),
        };
        if self.0.len() == triangles {
            Ok(())
        } else {
            Err(format!("{} flag words for {triangles} triangles", self.0.len()))
        }
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({"triangles": self.0.len()})))
    }
}
