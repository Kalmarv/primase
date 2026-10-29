//! Explicit, generic material feature specialisations, retained alongside the original lift.
use crate::shader::{LiftedShader, ShaderBinding, StageBinding};
use crate::{Component, Entity, Geometry, World};
use std::sync::Arc;
/// A material's explicit request to specialise its source programs on their stated feature words.
/// A producer may write this only from the exact lift and the material's binding. Data constants
/// remain uniforms; consumers keep the original shader and lift as provenance.
#[derive(Clone, Debug, Default)]
pub struct ShaderSpecialisations(pub Vec<StageSpecialisation>);
/// One stage's specialised copy and the complete audit of its feature decisions.
#[derive(Clone, Debug)]
pub struct StageSpecialisation {
    pub shader: Entity,
    pub program: Option<Arc<iridia::Program>>,
    pub words: std::collections::BTreeMap<(iridia_interface::ResourceId, u32), u32>,
    pub removed: Vec<iridia_interface::ResourceId>,
    pub folded: usize,
    pub refusal: Option<String>,
}
impl ShaderSpecialisations {
    /// Specialise only on the known words deciding a fully stated branch/select. A missing exact
    /// lift or a refused transformation is a recorded refusal, never a guessed program.
    pub fn of(world: &World, binding: &ShaderBinding) -> Self {
        Self(
            binding
                .stages
                .iter()
                .map(|stage| Self::stage(world, stage))
                .collect(),
        )
    }
    fn stage(world: &World, stage: &StageBinding) -> StageSpecialisation {
        let result = world
            .get::<LiftedShader>(stage.shader)
            .ok_or_else(|| "source has not supplied an exact lift".to_owned())
            .and_then(|source| iridia::specialise::specialise(&source.0, &stage.known_words()));
        match result {
            Ok(result) => StageSpecialisation {
                shader: stage.shader,
                program: Some(Arc::new(result.program)),
                words: result.words,
                removed: result.unread,
                folded: result.folded,
                refusal: None,
            },
            Err(why) => StageSpecialisation {
                shader: stage.shader,
                program: None,
                words: Default::default(),
                removed: vec![],
                folded: 0,
                refusal: Some(why),
            },
        }
    }
}
impl Component for ShaderSpecialisations {
    const NAME: &'static str = "core.shader_specialisations";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.iter().map(|s| s.shader));
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, crate::JsonError> {
        crate::json::to_value(&self.0.iter().map(|s|serde_json::json!({
            "shader":s.shader,"words":s.words.iter().map(|((resource,offset),word)|(*resource,*offset,*word)).collect::<Vec<_>>(),
            "resources_removed":s.removed,"expressions_folded":s.folded,"refusal":s.refusal,
        })).collect::<Vec<_>>()).map(Some)
    }
    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        let binding = world
            .get::<ShaderBinding>(entity)
            .ok_or("specialisations have no source binding")?;
        for (index, stage) in self.0.iter().enumerate() {
            let source = binding
                .stages
                .iter()
                .find(|s| s.shader == stage.shader)
                .ok_or("specialisation names an unbound shader")?;
            if self.0.iter().take(index).any(|s| s.shader == stage.shader) {
                return Err("duplicate stage specialisation".into());
            }
            let known = source.known_words();
            if stage.words.iter().any(|(k, v)| known.get(k) != Some(v)) {
                return Err("specialisation freezes a word the binding does not state".into());
            }
            if let Some(program) = &stage.program {
                program.validate().map_err(|e| format!("{e:?}"))?;
            }
        }
        Ok(())
    }
    fn apply_linear(&self, _: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}
