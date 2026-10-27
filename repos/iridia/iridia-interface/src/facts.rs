//! Small, owned facts recorded by a resolver while it already has a lifted program. No bytecode or IR is retained.
use crate::{D3dBinding, OutputBinding, ResourceId, Stage, Surface, Varying};
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

/// A resolver's measured facts about one program: what it reads and writes and how large it is. `H` is the handle the
/// consumer keeps the program by (`None` until [`ProgramFacts::bound`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgramFacts<H> {
    pub name: String,
    pub hash: Option<String>,
    pub stage: Stage,
    pub shader: Option<H>,
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
impl<H> ProgramFacts<H> {
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
    pub fn bound(mut self, shader: H, outputs: &[OutputBinding]) -> Self {
        self.shader = Some(shader);
        self.outputs = outputs.to_vec();
        self.surface = Surface::read(self.outputs.iter().map(|output| (output.lanes, &output.role)));
        self
    }
}
