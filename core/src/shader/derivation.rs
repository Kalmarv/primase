//! `core.shader_derivation`: a program derived from a material's source stage to compute one quantity of its output.
use std::sync::Arc;

use iridia_interface::{Label, RenderState};

use crate::shader::{ConstantBinding, InputBinding, Shader, StageBinding, TextureSource};
use crate::{Component, Entity, Geometry, World};

/// `core.shader_derivation`, on a material entity: a program a pass derived from one of the material's source stages
/// to compute one quantity that stage's output contains (an albedo inside a lit colour), for a consumer that draws that
/// quantity. It is not a [`super::ShaderSpecialisations`] stage: a specialisation is the source program folded on the
/// binding's stated words and still computes the source's outputs, where a derivation computes something else, cut out
/// of the source program; what it cut (lighting, fog, tone mapping) is in its notes and what it computes is its stage's
/// outputs. The material's `core.shader_binding` and the source `core.shader` stay as they are, as provenance. A
/// derivation its pass could not make keeps the reason instead (`result` is `Err`), so a consumer does not draw the
/// source program in its place.
#[derive(Clone, Debug)]
pub struct ShaderDerivation {
    /// The derivation's name and version, its producer's (`<game>.generic-albedo.v1`).
    pub name: String,
    pub result: Result<DerivedStage, String>,
}

/// A derived program and how it is drawn.
#[derive(Clone, Debug)]
pub struct DerivedStage {
    /// The binding the derived program is drawn with: `shader` names the source program it was derived from, the
    /// inputs and outputs are the derived program's, the constants and textures the material's.
    pub stage: StageBinding,
    /// The render state it is drawn with.
    pub state: RenderState,
    pub program: Arc<iridia::Program>,
    /// What the derivation cut from the source program or leaves unresolved, and which source it was derived from.
    pub notes: Vec<String>,
    /// A per-vertex program supplying one of `program`'s inputs, when the derivation needs one.
    pub vertex_input: Option<DerivedInput>,
}

/// A program derived from a source vertex stage that supplies one float input of a derived pixel program (at most four
/// lanes): evaluated once per vertex on the mesh's streams, then interpolated as the source vertex program's output is.
#[derive(Clone, Debug)]
pub struct DerivedInput {
    /// The derived pixel program's input it supplies.
    pub target: Label,
    /// The derivation's name and version, its producer's.
    pub name: String,
    pub program: Arc<iridia::Program>,
    /// What the derivation cut from the source program or leaves unresolved, and which source it was derived from.
    pub notes: Vec<String>,
    /// Its inputs by role.
    pub inputs: Vec<InputBinding>,
    /// Its constants by role.
    pub constants: Vec<ConstantBinding>,
}

impl Component for ShaderDerivation {
    const NAME: &'static str = "core.shader_derivation";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        if let Ok(derived) = &self.result {
            out.push(derived.stage.shader);
            for t in &derived.stage.textures {
                if let TextureSource::Texture { texture, .. } = &t.source {
                    out.push(*texture);
                }
            }
        }
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, crate::JsonError> {
        let result = match &self.result {
            Ok(d) => serde_json::json!({
                "shader": d.stage.shader,
                "variant": d.stage.variant,
                "outputs": d.stage.outputs,
                "notes": d.notes,
                "vertex_input": d.vertex_input.as_ref().map(|v| serde_json::json!({
                    "target": v.target, "name": v.name, "notes": v.notes,
                })),
            }),
            Err(why) => serde_json::json!({ "refusal": why }),
        };
        crate::json::to_value(&serde_json::json!({ "name": self.name, "result": result })).map(Some)
    }
    fn verify(&self, _entity: Entity, world: &World) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("derivation names no derivation".into());
        }
        let Ok(derived) = &self.result else {
            return Ok(());
        };
        if world.get::<Shader>(derived.stage.shader).is_none() {
            return Err("derivation names a source stage whose shader entity has no core.shader".into());
        }
        if derived.program.stage != derived.stage.stage {
            return Err("derived program is not a program of its stage's kind".into());
        }
        derived.program.validate().map_err(|e| format!("{e:?}"))?;
        if let Some(input) = &derived.vertex_input {
            if input.name.is_empty() {
                return Err("derived input names no derivation".into());
            }
            if !derived
                .program
                .interface
                .inputs
                .iter()
                .any(|v| v.label == input.target)
            {
                return Err("derived input supplies no input of the derived program".into());
            }
            input.program.validate().map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    }
    fn apply_linear(&self, _: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}
