//! Explicit, generic material feature specialisations, retained alongside the original lift.
use crate::shader::{LiftedShader, ShaderBinding, StageBinding};
use crate::{Component, Entity, Geometry, World};
use iridia_interface::ResourceId;
use std::collections::{BTreeMap, BTreeSet};
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
    pub program: Option<Arc<iridia_ir::Program>>,
    pub words: BTreeMap<(ResourceId, u32), u32>,
    pub removed: Vec<ResourceId>,
    pub folded: usize,
    pub refusal: Option<String>,
}
impl ShaderSpecialisations {
    /// Specialise only on the known words deciding a fully stated branch/select. A missing exact
    /// lift or a refused transformation is a recorded refusal, never a guessed program.
    pub fn of(world: &World, binding: &ShaderBinding) -> Self {
        Self::shared(world, binding, &mut SpecialisationCache::default())
    }
    /// The specialised program of the stage that draws with `shader`, where its specialisation was not refused.
    pub fn program(&self, shader: Entity) -> Option<&Arc<iridia_ir::Program>> {
        self.0
            .iter()
            .find(|s| s.shader == shader)
            .and_then(|s| s.program.as_ref())
    }
    /// [`ShaderSpecialisations::of`], sharing `cache` with the other materials of the same world: a stage whose
    /// program and selected feature words (each with its value) are another material's gets that material's program.
    pub fn shared(world: &World, binding: &ShaderBinding, cache: &mut SpecialisationCache) -> Self {
        Self(
            binding
                .stages
                .iter()
                .map(|stage| Self::stage(world, stage, cache))
                .collect(),
        )
    }
    fn stage(world: &World, stage: &StageBinding, cache: &mut SpecialisationCache) -> StageSpecialisation {
        let refused = |why: &str| StageSpecialisation {
            shader: stage.shader,
            program: None,
            words: Default::default(),
            removed: vec![],
            folded: 0,
            refusal: Some(why.to_owned()),
        };
        let Some(source) = world.get::<LiftedShader>(stage.shader) else {
            return refused("source has not supplied an exact lift");
        };
        let known = stage.known_words();
        // which words decide a branch or select depends on which are known, not on their values
        let keys: BTreeSet<_> = known.keys().copied().collect();
        let selected = match cache.selected.entry((stage.shader, keys)).or_insert_with(|| {
            iridia_passes::feature_words(&source.0, &known)
                .map(|w| w.into_keys().collect())
                .map_err(|e| e.to_string())
        }) {
            Ok(selected) => selected,
            Err(why) => return refused(why),
        };
        let words: Words = known
            .into_iter()
            .filter(|(key, _)| selected.contains(key))
            .collect();
        match cache
            .programs
            .entry((stage.shader, words))
            .or_insert_with_key(|(_, words)| {
                iridia_passes::specialise(&source.0, words)
                    .map(|result| Specialised {
                        program: Arc::new(result.program),
                        words: result.words,
                        removed: result.unread,
                        folded: result.folded,
                    })
                    .map_err(|e| e.to_string())
            }) {
            Ok(shared) => StageSpecialisation {
                shader: stage.shader,
                program: Some(shared.program.clone()),
                words: shared.words.clone(),
                removed: shared.removed.clone(),
                folded: shared.folded,
                refusal: None,
            },
            Err(why) => refused(why),
        }
    }
}

/// A uniform word, by its buffer and byte offset.
type Word = (ResourceId, u32);
type Words = BTreeMap<Word, u32>;

/// One specialised program and its audit, shared by every stage specialised on the same words.
#[derive(Debug)]
struct Specialised {
    program: Arc<iridia_ir::Program>,
    words: Words,
    removed: Vec<ResourceId>,
    folded: usize,
}

/// The specialisations made for the materials of one world, so that materials of one program and the same selected
/// feature words (each with its value) share one specialised program rather than each holding a copy.
#[derive(Debug, Default)]
pub struct SpecialisationCache {
    /// Per program and set of known words, the words that decide a branch or select (or why none can be chosen).
    selected: BTreeMap<(Entity, BTreeSet<Word>), Result<BTreeSet<Word>, String>>,
    /// Per program and selected words with their values, its specialisation (or why it was refused).
    programs: BTreeMap<(Entity, Words), Result<Specialised, String>>,
}
impl SpecialisationCache {
    /// The specialised programs made so far.
    pub fn programs(&self) -> usize {
        self.programs.values().filter(|p| p.is_ok()).count()
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
                iridia_ir::validate(program).map_err(|e| format!("{e:?}"))?;
            }
        }
        Ok(())
    }
    fn apply_linear(&self, _: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::{ConstantBinding, ConstantRole};
    use iridia_interface::{ResourceKind, Stage};

    /// A pixel program whose first constant word switches between a texture sample scaled by the second word and a
    /// coordinate.
    fn program() -> iridia_ir::Program {
        iridia_mojo::ir::lift(
            "uniform vec4 ps_uniforms_vec4[1];\nuniform sampler2D ps_s0;\nvarying vec4 vTexCoord0;\nvec4 ps_r0;\nvoid main() {\nps_r0 = texture2D(ps_s0,vTexCoord0.xy);\nif (ps_uniforms_vec4[0].x != 0.0) {\nps_r0 = ps_r0 * ps_uniforms_vec4[0].yyyy;\n} else {\nps_r0 = vTexCoord0;\n}\ngl_FragColor = ps_r0;\n}",
            Stage::Pixel,
            &iridia_mojo::Hints {
                target: "fixture".into(),
                constants: vec![iridia_mojo::Constant {
                    name: "SwitchAndScale".into(),
                    offset: 0,
                    bytes: 16,
                    width: 4,
                }],
                samplers: vec![iridia_mojo::Sampler {
                    glsl_name: "ps_s0".into(),
                    name: "image".into(),
                    slot: 0,
                }],
                attributes: vec![],
            },
        )
        .unwrap()
    }

    /// A material binding `shader` with the switch and scale words stated.
    fn binding(shader: Entity, buffer: ResourceId, switch: f32, scale: f32) -> ShaderBinding {
        ShaderBinding {
            stages: vec![StageBinding {
                constants: vec![ConstantBinding {
                    resource: buffer,
                    offset: 0,
                    size: 8,
                    name: None,
                    role: ConstantRole::Material(vec![switch.to_bits(), scale.to_bits()]),
                    lines: None,
                }],
                ..StageBinding::new(Stage::Pixel, shader)
            }],
            state: Default::default(),
        }
    }

    /// Materials of one program share its specialisation exactly when every selected word has the same value: a
    /// word that decides nothing (the scale) does not split them, and the switch with another value does.
    #[test]
    fn materials_share_a_specialisation_only_when_the_selected_words_are_equal() {
        let source = program();
        let buffer = source
            .interface
            .resources
            .iter()
            .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
            .unwrap()
            .id;
        let mut world = World::new();
        let shader = world.spawn();
        world.insert(shader, LiftedShader(Arc::new(source)));
        let mut cache = SpecialisationCache::default();
        let mut of = |switch: f32, scale: f32| {
            let set =
                ShaderSpecialisations::shared(&world, &binding(shader, buffer, switch, scale), &mut cache);
            let [stage] = &set.0[..] else { panic!("one stage") };
            assert_eq!(stage.refusal, None);
            (stage.program.clone().unwrap(), stage.words.clone())
        };
        let (on, on_words) = of(1.0, 2.0);
        let (on_again, _) = of(1.0, 3.0);
        let (off, off_words) = of(0.0, 2.0);
        // the switch alone is selected, with its value
        assert_eq!(on_words, Words::from([((buffer, 0), 1f32.to_bits())]));
        assert_eq!(off_words, Words::from([((buffer, 0), 0f32.to_bits())]));
        assert!(Arc::ptr_eq(&on, &on_again));
        assert!(!Arc::ptr_eq(&on, &off));
        // the switch off leaves the sample unread: the two programs differ
        let textures = |p: &iridia_ir::Program| {
            p.interface
                .resources
                .iter()
                .filter(|r| matches!(r.kind, ResourceKind::Texture { .. }))
                .count()
        };
        assert_eq!((textures(&on), textures(&off)), (1, 0));
        assert_eq!(cache.programs(), 2);
    }

    /// A stage whose program has no exact lift is refused, by reason.
    #[test]
    fn a_stage_without_an_exact_lift_is_refused() {
        let mut world = World::new();
        let shader = world.spawn();
        let set = ShaderSpecialisations::of(&world, &binding(shader, ResourceId(0), 1.0, 1.0));
        assert_eq!(
            set.0[0].refusal.as_deref(),
            Some("source has not supplied an exact lift")
        );
        assert!(set.0[0].program.is_none());
    }
}
