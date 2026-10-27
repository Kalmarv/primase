//! The program facts a resolver records, as components: [`ProgramFacts`] bound to its `core.shader` entity.
use iridia_interface::{D3dBinding, InstructionCount, Interface, ResourceId, Stage, Varying};
use serde::Serialize;

use super::roles::OutputBinding;
use super::surface::Surface;
use crate::{Component, Entity, Geometry, World};

/// Just the declared resource's source address; bulk buffer layouts and default constants stay in the lifted program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProgramResourceFacts {
    /// The resource's id in the program's interface.
    pub id: ResourceId,
    /// The name the source gives the resource (`ResourceProvenance::name`).
    pub name: String,
    /// The D3D register class, index and space the resource is bound at; `None` when the source has no such address.
    pub d3d: Option<D3dBinding>,
}

/// `core.program_facts`: a resolver's measured facts about one program, what it reads and writes and how large it is,
/// also stored on the program entity for reuse across materials.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgramFacts {
    /// The name the consumer knows the program by.
    pub name: String,
    /// The `sha256:` hash of the program's bytes, as `Provenance::source_hash` gives it; `None` when the program was not
    /// read.
    pub hash: Option<String>,
    /// The stage the program runs in.
    pub stage: Stage,
    /// The consumer's handle to the program; `None` until [`ProgramFacts::bound`] is called.
    pub shader: Option<Entity>,
    /// The program's size, or `None` for a program that was not lifted or whose lifter does not count.
    pub instruction_count: Option<InstructionCount>,
    /// Used signature elements, including the original register/mask provenance. Not a guess from semantic names.
    pub inputs: Vec<Varying>,
    /// Resource declarations retain names/register/space, not layouts or bytecode. Not a resource liveness claim.
    pub resources: Vec<ProgramResourceFacts>,
    /// The roles of the program's outputs; empty until [`ProgramFacts::bound`] is called.
    pub outputs: Vec<OutputBinding>,
    /// The outputs read by their roles as a lit surface; the default surface until [`ProgramFacts::bound`] is called.
    pub surface: Surface,
    /// A program that was not found/selected/lifted has a reason, never a zero instruction count.
    pub not_lifted: Option<String>,
}
impl ProgramFacts {
    /// The facts of a lifted program, named `name`, of its stage, the hash of its bytes, its instruction count and its
    /// interface (the inputs it uses and the source address of each resource); no output roles until
    /// [`ProgramFacts::bound`]. Nothing is parsed or lifted again.
    pub fn lifted(
        name: impl Into<String>,
        stage: Stage,
        hash: Option<String>,
        instruction_count: Option<InstructionCount>,
        interface: &Interface,
    ) -> Self {
        Self {
            name: name.into(),
            hash,
            stage,
            shader: None,
            instruction_count,
            inputs: interface
                .inputs
                .iter()
                .filter(|input| input.used)
                .cloned()
                .collect(),
            resources: interface
                .resources
                .iter()
                .map(|resource| ProgramResourceFacts {
                    id: resource.id,
                    name: resource.provenance.name.clone(),
                    d3d: resource.provenance.d3d,
                })
                .collect(),
            outputs: Vec::new(),
            surface: Surface::default(),
            not_lifted: None,
        }
    }
    /// The facts of a program that has none: no inputs, resources, outputs or instruction count, and `reason` in
    /// `not_lifted`.
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
    /// Output roles are resolved after the lift; the surface is always the shared reading ([`Surface::read`]).
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
