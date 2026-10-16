//! Authored placement optionality, independent of any consumer's filtering policy.

use crate::{Component, Entity, Instance, JsonError, Mesh, MeshFile, Transform, World};
use serde::Serialize;
use serde_json::Value;

/// How optional a placement is (`core.priority`): 0 is required and higher values
/// are more optional. Absence uses the required-placement behavior.
///
/// The dimensionless range is 0-255. This component does not filter placements;
/// a consumer with narrower storage must reject values it cannot preserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Priority(pub u8);

impl Component for Priority {
    const NAME: &'static str = "core.priority";

    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }

    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        if world.has::<Instance>(entity)
            || (world.has::<Transform>(entity)
                && (world.has::<Mesh>(entity) || world.has::<MeshFile>(entity)))
        {
            Ok(())
        } else {
            Err("priority on an entity that is not placed".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_preserves_full_range_and_requires_a_placement() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Priority(255));
        assert_eq!(world.verify_components().len(), 1);
        world.insert(entity, Mesh::default());
        assert_eq!(world.verify_components().len(), 1);
        world.insert(entity, Transform::IDENTITY);
        assert!(world.verify_components().is_empty());
        for value in [0, 15, 16, 255] {
            assert_eq!(Priority(value).to_json().unwrap(), Some(Value::from(value)));
        }
    }
}
