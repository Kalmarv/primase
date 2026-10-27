//! The program facts a resolver records, as components: [`iridia_interface::ProgramFacts`] bound to its `core.shader`
//! entity.
use serde::Serialize;

use crate::{Component, Entity, Geometry, World};

/// `core.program_facts`: a resolver's measured facts, also stored on the program entity for reuse across materials.
pub type ProgramFacts = iridia_interface::ProgramFacts<Entity>;

impl Component for ProgramFacts {
    const NAME: &'static str = "core.program_facts";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, crate::json::JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.shader);
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.not_lifted.is_some() && self.instruction_count.is_some() {
            return Err("an unlifted program claims an instruction count".into());
        }
        Ok(())
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}
/// `core.material_programs`: the selected/resolved program facts on each material, including unresolved programs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MaterialPrograms(pub Vec<ProgramFacts>);
impl Component for MaterialPrograms {
    const NAME: &'static str = "core.material_programs";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, crate::json::JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for program in &self.0 {
            program.references(out);
        }
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        for program in &self.0 {
            program.verify(e, w)?;
        }
        Ok(())
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}
