//! `select-variant`: the choice of one variant of a [`ShaderBinding`] that a source states and leaves to its consumer.

use std::collections::BTreeSet;

use crate::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use crate::world::{Component, World};

use super::ShaderBinding;

/// `select-variant{field=NAME,value=N}` (both required: the choice is the caller's): in every [`ShaderBinding`] that
/// has stages keyed by `field` ([`super::VariantKey`]), the stages of the variant `value` stay, without their key, and
/// the stages of the other variants of that field are left out, counted. The binding then holds one stage of each kind
/// under no key, as a consumer that draws one program of a stage reads it. A stage with no key or a key of another
/// field is left as it is; a binding with no stage keyed by `field`, or none of the value `value` (a source numbers a
/// binding's variants from 0, so bindings hold different numbers of them), is left as it is and counted. A `value`
/// that no binding of the world carries, or a `field` none uses, is an error naming the fields and values the world
/// does carry, and changes nothing. A program only the stages left out drew (its `core.shader` entity, which nothing
/// in the world refers to any more) is despawned, counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectVariant {
    pub field: String,
    pub value: u32,
}

/// `select-variant`'s declaration ([`crate::pass::PassRegistry::add`]).
pub const SELECT_VARIANT: PassSpec = PassSpec {
    name: "select-variant",
    options: &[
        OptionSpec::required(
            "field",
            "dialect.variant",
            "the field of the variant keys to choose by (VariantKey::field), e.g. dialect.variant",
        ),
        OptionSpec::required(
            "value",
            "0",
            "the value of that field whose stages are kept: the caller's choice, which the source does not make",
        ),
    ],
    make: select_variant,
};

fn select_variant(g: &Given) -> Result<Box<dyn Pass>, String> {
    let field = g.required("field").trim();
    if field.is_empty() {
        return Err("field is empty".to_owned());
    }
    let value = g.required("value");
    let value = value
        .trim()
        .parse()
        .map_err(|_| format!("value={value} is not a number from 0"))?;
    Ok(Box::new(SelectVariant {
        field: field.to_owned(),
        value,
    }))
}

/// The values as a range when they are consecutive from their first, else a list.
fn describe(values: &BTreeSet<u32>) -> String {
    match (values.first(), values.last()) {
        (Some(&first), Some(&last)) if (last - first) as usize + 1 == values.len() && values.len() > 1 => {
            format!("{first} to {last}")
        }
        _ => values.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
    }
}

impl Pass for SelectVariant {
    fn name(&self) -> &'static str {
        "select-variant"
    }
    fn options(&self) -> Options {
        Options::from([
            ("field".to_owned(), self.field.clone()),
            ("value".to_owned(), self.value.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[ShaderBinding::NAME]
    }
    /// The stages of the other variants are taken out of the binding in place, and the chosen ones lose their key.
    fn modifies(&self) -> &[&'static str] {
        &[ShaderBinding::NAME]
    }
    /// The programs only the left-out stages drew.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut fields: BTreeSet<&str> = BTreeSet::new();
        let mut values: BTreeSet<u32> = BTreeSet::new();
        for (_, binding) in w.iter::<ShaderBinding>() {
            for key in binding.stages.iter().filter_map(|s| s.variant.as_ref()) {
                fields.insert(key.field.as_str());
                if key.field == self.field {
                    values.insert(key.value);
                }
            }
        }
        if values.is_empty() {
            return Err(format!(
                "no stage of any binding is keyed by {}; the fields keyed are {}",
                self.field,
                if fields.is_empty() {
                    "none".to_owned()
                } else {
                    fields.into_iter().collect::<Vec<_>>().join(", ")
                }
            ));
        }
        if !values.contains(&self.value) {
            return Err(format!(
                "no stage of any binding has {}={}; the values are {}",
                self.field,
                self.value,
                describe(&values)
            ));
        }
        let chosen: Vec<_> = w.iter::<ShaderBinding>().map(|(e, b)| (e, b.clone())).collect();
        // the programs of the stages left out, despawned below where nothing refers to them any more
        let mut left_out = BTreeSet::new();
        for (entity, mut binding) in chosen {
            let keyed = |s: &super::StageBinding| s.variant.as_ref().is_some_and(|k| k.field == self.field);
            if !binding.stages.iter().any(keyed) {
                log.count("bindings with no stage keyed by the field, left as they are", 1);
                continue;
            }
            if !binding.stages.iter().any(|s| {
                s.variant
                    .as_ref()
                    .is_some_and(|k| k.field == self.field && k.value == self.value)
            }) {
                log.count(
                    "bindings with no stage of the chosen value (they hold fewer variants), left as they are",
                    1,
                );
                continue;
            }
            let before = binding.stages.len();
            left_out.extend(
                binding
                    .stages
                    .iter()
                    .filter(|s| {
                        s.variant
                            .as_ref()
                            .is_some_and(|k| k.field == self.field && k.value != self.value)
                    })
                    .map(|s| s.shader),
            );
            binding.stages.retain(|s| {
                s.variant
                    .as_ref()
                    .is_none_or(|k| k.field != self.field || k.value == self.value)
            });
            for stage in &mut binding.stages {
                if stage.variant.as_ref().is_some_and(|k| k.field == self.field) {
                    stage.variant = None;
                }
            }
            log.count("bindings with one variant chosen", 1);
            log.count(
                "stages left out: another variant of the field",
                (before - binding.stages.len()) as u64,
            );
            w.insert(entity, binding);
        }
        let unreferenced = w.unreferenced();
        let gone: Vec<_> = left_out.intersection(&unreferenced).copied().collect();
        for &e in &gone {
            w.despawn(e);
        }
        log.count(
            "shader programs only the left-out stages drew, despawned",
            gone.len() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::StageBinding;
    use crate::shader::VariantKey;
    use iridia_interface::Stage;

    fn key(field: &str, value: u32) -> Option<VariantKey> {
        Some(VariantKey {
            field: field.to_owned(),
            value,
            parts: Vec::new(),
        })
    }

    fn stage(w: &mut World, stage: Stage, variant: Option<VariantKey>) -> StageBinding {
        StageBinding {
            variant,
            ..StageBinding::new(stage, w.spawn())
        }
    }

    /// A binding with a vertex and a pixel stage for each of `values` of `field`.
    fn binding(w: &mut World, field: &str, values: &[u32]) -> crate::Entity {
        let mut stages = Vec::new();
        for &v in values {
            stages.push(stage(w, Stage::Vertex, key(field, v)));
            stages.push(stage(w, Stage::Pixel, key(field, v)));
        }
        let e = w.spawn();
        w.insert(
            e,
            ShaderBinding {
                stages,
                ..ShaderBinding::default()
            },
        );
        e
    }

    fn run(w: &mut World, field: &str, value: &str) -> Result<PassLog, String> {
        let options = Options::from([
            ("field".to_owned(), field.to_owned()),
            ("value".to_owned(), value.to_owned()),
        ]);
        let mut log = PassLog::default();
        SELECT_VARIANT.instance(&options)?.run(w, &mut log)?;
        Ok(log)
    }

    #[test]
    fn the_chosen_variants_stages_stay_without_their_key() {
        let mut w = World::new();
        let a = binding(&mut w, "dialect.variant", &[0, 1, 2]);
        let kept: Vec<_> = w.get::<ShaderBinding>(a).unwrap().stages[2..4]
            .iter()
            .map(|s| s.shader)
            .collect();
        let log = run(&mut w, "dialect.variant", "1").unwrap();
        let after = w.get::<ShaderBinding>(a).unwrap();
        assert_eq!(after.stages.len(), 2);
        assert_eq!(after.stages.iter().map(|s| s.shader).collect::<Vec<_>>(), kept);
        assert!(after.stages.iter().all(|s| s.variant.is_none()));
        assert_eq!(after.stages[0].stage, Stage::Vertex);
        assert_eq!(log.counts["bindings with one variant chosen"], 1);
        assert_eq!(log.counts["stages left out: another variant of the field"], 4);
    }

    /// The programs only the left-out stages drew are despawned; one another binding still draws stays.
    #[test]
    fn programs_only_the_left_out_stages_drew_are_despawned() {
        let mut w = World::new();
        let a = binding(&mut w, "dialect.variant", &[0, 1, 2]);
        let stages = w.get::<ShaderBinding>(a).unwrap().stages.clone();
        let (shared, gone) = (
            stages[0].shader,
            [stages[1].shader, stages[4].shader, stages[5].shader],
        );
        // another binding draws the first left-out program, unkeyed
        let other = w.spawn();
        let mut drawn = stage(&mut w, Stage::Vertex, None);
        drawn.shader = shared;
        w.insert(
            other,
            ShaderBinding {
                stages: vec![drawn],
                ..ShaderBinding::default()
            },
        );
        let log = run(&mut w, "dialect.variant", "1").unwrap();
        assert!(w.is_alive(shared));
        assert!(gone.iter().all(|&e| !w.is_alive(e)));
        assert!(stages[2..4].iter().all(|s| w.is_alive(s.shader)));
        assert_eq!(
            log.counts["shader programs only the left-out stages drew, despawned"],
            3
        );
    }

    #[test]
    fn a_binding_without_the_value_or_the_field_is_left_as_it_is_and_counted() {
        let mut w = World::new();
        let big = binding(&mut w, "dialect.variant", &[0, 1, 2]);
        let small = binding(&mut w, "dialect.variant", &[0]);
        let other = binding(&mut w, "dialect.layout", &[1, 2]);
        let before = (
            w.get::<ShaderBinding>(small).unwrap().clone(),
            w.get::<ShaderBinding>(other).unwrap().clone(),
        );
        let log = run(&mut w, "dialect.variant", "2").unwrap();
        assert_eq!(w.get::<ShaderBinding>(big).unwrap().stages.len(), 2);
        assert_eq!(w.get::<ShaderBinding>(small).unwrap(), &before.0);
        assert_eq!(w.get::<ShaderBinding>(other).unwrap(), &before.1);
        assert_eq!(
            log.counts["bindings with no stage of the chosen value (they hold fewer variants), left as they are"],
            1
        );
        assert_eq!(
            log.counts["bindings with no stage keyed by the field, left as they are"],
            1
        );
    }

    #[test]
    fn stages_with_no_key_or_another_field_stay() {
        let mut w = World::new();
        let e = binding(&mut w, "dialect.variant", &[0, 1]);
        let plain = stage(&mut w, Stage::Pixel, None);
        let foreign = stage(&mut w, Stage::Vertex, key("dialect.layout", 7));
        let mut b = w.get::<ShaderBinding>(e).unwrap().clone();
        b.stages.push(plain);
        b.stages.push(foreign);
        w.insert(e, b);
        run(&mut w, "dialect.variant", "0").unwrap();
        let after = w.get::<ShaderBinding>(e).unwrap();
        assert_eq!(after.stages.len(), 4);
        assert_eq!(
            after
                .stages
                .iter()
                .filter_map(|s| s.variant.as_ref())
                .map(|k| (k.field.as_str(), k.value))
                .collect::<Vec<_>>(),
            [("dialect.layout", 7)]
        );
    }

    #[test]
    fn a_value_or_field_no_stage_carries_is_refused_naming_what_is_there() {
        let mut w = World::new();
        let a = binding(&mut w, "dialect.variant", &[0, 1, 2, 3]);
        let b = binding(&mut w, "dialect.variant", &[1, 3]);
        let before = (
            w.get::<ShaderBinding>(a).unwrap().clone(),
            w.get::<ShaderBinding>(b).unwrap().clone(),
        );
        assert_eq!(
            run(&mut w, "dialect.variant", "4").unwrap_err(),
            "no stage of any binding has dialect.variant=4; the values are 0 to 3"
        );
        assert_eq!(
            run(&mut w, "dialect.layout", "0").unwrap_err(),
            "no stage of any binding is keyed by dialect.layout; the fields keyed are dialect.variant"
        );
        // nothing changed
        assert_eq!(w.get::<ShaderBinding>(a).unwrap(), &before.0);
        assert_eq!(w.get::<ShaderBinding>(b).unwrap(), &before.1);
        // the values of a world with gaps are listed
        let mut gaps = World::new();
        binding(&mut gaps, "dialect.variant", &[1, 3]);
        assert_eq!(
            run(&mut gaps, "dialect.variant", "2").unwrap_err(),
            "no stage of any binding has dialect.variant=2; the values are 1, 3"
        );
        // a world with no keyed stage at all
        let mut none = World::new();
        assert!(
            run(&mut none, "dialect.variant", "0")
                .unwrap_err()
                .contains("the fields keyed are none")
        );
    }

    #[test]
    fn both_options_are_required_and_the_value_is_a_number() {
        let missing = SELECT_VARIANT.instance(&Options::new()).err().unwrap();
        assert!(missing.contains("field is required"), "{missing}");
        let only_field = Options::from([("field".to_owned(), "dialect.variant".to_owned())]);
        assert!(
            SELECT_VARIANT
                .instance(&only_field)
                .err()
                .unwrap()
                .contains("value is required")
        );
        let bad = Options::from([
            ("field".to_owned(), "dialect.variant".to_owned()),
            ("value".to_owned(), "two".to_owned()),
        ]);
        assert!(
            SELECT_VARIANT
                .instance(&bad)
                .err()
                .unwrap()
                .contains("value=two is not a number from 0")
        );
    }
}
