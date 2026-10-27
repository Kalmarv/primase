//! Small, owned facts recorded by a resolver while it already has a lifted program. No bytecode or IR is retained.
use super::{D3dBinding, OutputBinding, ResourceId, Stage, Surface, Varying};
use crate::{Component, Entity, Geometry, World};
use serde::Serialize;

/// The source's counting unit. DXBC counts decoded instructions including control flow, excluding declarations;
/// DXIL counts LLVM instructions
/// in the functions the lifter reads. Neither is an executed instruction count or an IR expression count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionUnit {
    DxbcInstructions,
    DxilInstructions,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct InstructionCount {
    pub count: u64,
    pub unit: InstructionUnit,
}

/// Just the declared resource's source address; bulk buffer layouts/default constants remain on core.shader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProgramResourceFacts {
    pub id: ResourceId,
    pub name: String,
    pub d3d: Option<D3dBinding>,
}

/// `core.program_facts`: a resolver's measured facts, also stored on the program entity for reuse across materials.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgramFacts {
    pub name: String,
    pub hash: Option<String>,
    pub stage: Stage,
    pub shader: Option<Entity>,
    pub instruction_count: Option<InstructionCount>,
    /// Used signature elements, including the original register/mask provenance. Not a guess from semantic names.
    pub inputs: Vec<Varying>,
    /// Resource declarations retain names/register/space, not layouts or bytecode. Not a resource liveness claim.
    pub resources: Vec<ProgramResourceFacts>,
    pub outputs: Vec<OutputBinding>,
    pub surface: Surface,
    /// A program that was not found/selected/lifted has a reason, never a zero instruction count.
    pub not_lifted: Option<String>,
}
impl ProgramFacts {
    pub fn unavailable(
        name: impl Into<String>,
        stage: Stage,
        hash: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            hash,
            stage,
            shader: None,
            instruction_count: None,
            inputs: Vec::new(),
            resources: Vec::new(),
            outputs: Vec::new(),
            surface: Surface::default(),
            not_lifted: Some(reason.into()),
        }
    }
    /// Material output roles are resolved after the lift; the surface is always the shared core interpretation.
    pub fn bound(mut self, shader: Entity, outputs: &[OutputBinding]) -> Self {
        self.shader = Some(shader);
        self.outputs = outputs.to_vec();
        self.surface = Surface::read(self.outputs.iter().map(|output| (output.lanes, &output.role)));
        self
    }
}
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
