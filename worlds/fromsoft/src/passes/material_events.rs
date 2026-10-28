//! Material defaults and the proven facts of MaterialEx event routing. This pass deliberately
//! emits no MaterialParameter channels: event 198's value-at-time envelope is not established.
//! In particular, payload bytes 8/28/32 are NOT named target/fade/duration here.
use crate::materials::{LayeredMaterial, Material, ParameterValue};
use fromsoft_formats::Game;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Clip, ClipEvents, Component, EventValue, EventValueData, MaterialParameters, World};
// Preserve the public data paths; their definitions belong to components, not to this pass.
pub use crate::components::material_events::{MaterialExRow, MaterialExRows};
use crate::sekiro::source::{MaterialShader, ShaderBundle};

pub const PIXEL_KILL_KEY: u32 = 4_002_817_437;
pub const PIXEL_KILL_ID1_KEY: u32 = 1_642_328_750;
const PREFIX: &str = "material_ex.resolved.";
const REFUSAL: &str = "fromsoft.material_events: ";
pub const ALL_MATERIALS_INFERENCE: &str = "inferred MaterialEx material_id -1 targets all materials of the event's character (reported routing only; no channel applied)";

fn parameter_value(
    parameter: &fromsoft_formats::metaparam::MtdParam,
    stated: Option<ParameterValue>,
) -> Result<Vec<f32>, String> {
    let width = match parameter.value_type {
        0 | 1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        kind => {
            return Err(format!(
                "{}: type {kind} is not a core Values(1..=4) default; source record retained",
                parameter.name
            ));
        }
    };
    let value = match stated {
        Some(ParameterValue::Bool(value)) => vec![if value { 1. } else { 0. }],
        Some(ParameterValue::Int(values)) => values
            .into_iter()
            .map(|value| {
                let converted = value as f32;
                if f64::from(converted) != f64::from(value) {
                    Err(format!(
                        "{}: integer {value} is not exactly representable as core f32",
                        parameter.name
                    ))
                } else {
                    Ok(converted)
                }
            })
            .collect::<Result<_, _>>()?,
        Some(ParameterValue::Float(values)) => values,
        None => parameter.default[..width].to_vec(),
    };
    if value.len() != width || value.iter().any(|value| !value.is_finite()) {
        return Err(format!(
            "{}: material parameter has non-finite lanes or a width different from its metaparam",
            parameter.name
        ));
    }
    Ok(value)
}

fn field(name: impl Into<String>, value: EventValueData) -> EventValue {
    EventValue {
        name: name.into(),
        value,
    }
}
fn facts(typed: &[EventValue], game: Game, rows: Option<&MaterialExRows>) -> (Vec<EventValue>, String, bool) {
    let mut fields = Vec::new();
    let mut inferred_all = false;
    let ids: Vec<_> = typed
        .iter()
        .filter(|field| field.name == "material_ex.row_id")
        .filter_map(|field| match &field.value {
            EventValueData::I64(id) => i32::try_from(*id).ok(),
            _ => None,
        })
        .collect();
    let reason = if game != Game::EldenRing {
        "Sekiro event 198 row layout and envelope are unproven; raw event retained".to_owned()
    } else if ids.len() != 1
        || typed
            .iter()
            .filter(|field| field.name == "material_ex.row_id")
            .count()
            != 1
    {
        "reader supplied no unique typed MaterialEx row id; raw event retained".into()
    } else {
        let id = ids[0];
        match rows {
            None => "MaterialEx rows were not read; routing remains raw".into(),
            Some(rows) if rows.refusal.is_some() => format!(
                "MaterialEx rows unavailable: {}",
                rows.refusal.as_deref().unwrap_or("unread")
            ),
            Some(rows) => match rows.rows.get(&id) {
                None => format!("event 198 names absent MaterialEx row {id}"),
                Some(row) => {
                    fields.push(field(
                        format!("{PREFIX}parameter"),
                        EventValueData::Text(row.parameter.clone()),
                    ));
                    fields.push(field(
                        format!("{PREFIX}material_id"),
                        EventValueData::I64(i64::from(row.material_id)),
                    ));
                    fields.push(field(
                        format!("{PREFIX}row_bytes"),
                        EventValueData::Bytes(row.bytes.clone()),
                    ));
                    for (lane, value) in ["r", "g", "b", "a", "i"].iter().zip(row.rgba_i) {
                        fields.push(field(
                            format!("{PREFIX}row_{lane}"),
                            EventValueData::F64(f64::from(value)),
                        ));
                    }
                    if row.id == 10300
                        && row.parameter == "C[PixelKill]_Int"
                        && row.material_id == -1
                        && row.rgba_i[4] == 1.
                    {
                        inferred_all = true;
                        fields.push(field(
                            format!("{PREFIX}override_key"),
                            EventValueData::I64(i64::from(PIXEL_KILL_KEY)),
                        ));
                        fields.push(field(format!("{PREFIX}routing"),EventValueData::Text("inferred: all materials of the event's character (-1), via the metaparam override key".into())));
                        "row 10300 -> C[PixelKill]_Int, I=1; material -1 routing is inferred; event 198 value-at-time envelope is unproven (bytes 8/28/32); no channel applied".into()
                    } else {
                        format!(
                            "MaterialEx row {id} retained; parameter application/envelope is unproven; no channel applied"
                        )
                    }
                }
            },
        }
    };
    (fields, reason, inferred_all)
}

#[derive(Debug)]
pub struct LowerFromSoftMaterialEvents {
    pub game: Game,
}
pub const LOWER_FROMSOFT_MATERIAL_EVENTS: PassSpec = PassSpec {
    name: "lower-fromsoft-material-events",
    options: &[OptionSpec::required(
        "game",
        "elden",
        "sekiro or elden; only the observed ER MaterialEx v4 rows are interpreted",
    )],
    make: |g| {
        Ok(Box::new(LowerFromSoftMaterialEvents {
            game: super::clip_events::game_option(g)?,
        }))
    },
};
impl Pass for LowerFromSoftMaterialEvents {
    fn name(&self) -> &'static str {
        "lower-fromsoft-material-events"
    }
    fn options(&self) -> Options {
        Options::from([("game".into(), super::clip_events::game_text(self.game).into())])
    }
    // Defaults-only material reads and clip-only decode staging are both supported.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    fn provides(&self) -> &[&'static str] {
        &[MaterialParameters::NAME, ClipEvents::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[MaterialParameters::NAME, ClipEvents::NAME]
    }
    fn approximates(&self) -> &'static str {
        ALL_MATERIALS_INFERENCE
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut defaults = Vec::new();
        for (entity, shader) in world.iter::<MaterialShader>() {
            let Some(bundle) = world.get::<ShaderBundle>(shader.0) else {
                log.omit("material rest defaults: referenced shader bundle absent", 1);
                continue;
            };
            if bundle.game != self.game {
                continue;
            }
            let Some(metadata) = &bundle.parameters else {
                log.omit("material rest defaults: shader metaparam absent", 1);
                continue;
            };
            let view = world
                .get::<Material>(entity)
                .and_then(Material::view)
                .or_else(|| world.get::<LayeredMaterial>(entity).map(LayeredMaterial::view));
            let Some(view) = view else {
                log.omit("material defaults: source definition absent", 1);
                continue;
            };
            let mut parameters = MaterialParameters::default();
            for parameter in &metadata.mtd_params {
                let values = match parameter_value(parameter, view.definition.parameter(&parameter.name)) {
                    Ok(values) => values,
                    Err(reason) => {
                        log.omit(&reason, 1);
                        continue;
                    }
                };
                if let Some(previous) = parameters.0.insert(parameter.name.clone(), values.clone())
                    && previous != values
                {
                    return Err(format!(
                        "{entity:?}: duplicate material parameter {:?} has different defaults",
                        parameter.name
                    ));
                }
                // Only these two logical-name hashes have been verified. Other nonzero keys
                // remain in the source metadata; they are not assigned guessed logical names.
                let logical = match parameter.key {
                    PIXEL_KILL_KEY => Some("C[PixelKill]_Int"),
                    PIXEL_KILL_ID1_KEY => Some("C[PixelKill][ID1]_Int"),
                    _ => None,
                };
                if parameter.key != 0 && logical.is_none() {
                    log.omit(
                        &format!(
                            "unmapped MaterialEx override key {} for {}; physical rest default retained",
                            parameter.key, parameter.name
                        ),
                        1,
                    );
                }
                if let Some(logical) = logical {
                    if values.len() != 1 {
                        return Err(format!("{entity:?}: PixelKill key is not scalar"));
                    }
                    if let Some(previous) = parameters.0.insert(logical.into(), values.clone())
                        && previous != values
                    {
                        return Err(format!("{entity:?}: conflicting defaults for {logical}"));
                    }
                }
            }
            parameters.verify(entity, world)?;
            defaults.push((entity, parameters));
        }
        let mut events = Vec::new();
        for (entity, source) in world.iter::<ClipEvents>() {
            if !world.has::<Clip>(entity) || !source.events.iter().any(|event| event.kind == 198) {
                continue;
            }
            let mut lowered = source.clone();
            lowered.refusals.retain(|reason| !reason.starts_with(REFUSAL));
            for (index, _) in source
                .events
                .iter()
                .enumerate()
                .filter(|(_, event)| event.kind == 198)
            {
                lowered.events[index]
                    .values
                    .retain(|field| !field.name.starts_with(PREFIX));
                let (fields, reason, inferred_all) = facts(
                    &lowered.events[index].values,
                    self.game,
                    world.resource::<MaterialExRows>(),
                );
                lowered.events[index].values.extend(fields);
                lowered.refusals.push(format!("{REFUSAL}event {index}: {reason}"));
                if inferred_all {
                    log.approximate(ALL_MATERIALS_INFERENCE, 1);
                }
            }
            lowered.verify(entity, world)?;
            events.push((entity, lowered));
        }
        for (entity, parameters) in defaults {
            log.count("core material rest parameters", parameters.0.len() as u64);
            world.insert(entity, parameters);
        }
        for (entity, events) in events {
            for reason in events
                .refusals
                .iter()
                .filter(|reason| reason.starts_with(REFUSAL))
            {
                log.omit(reason, 1);
            }
            world.insert(entity, events);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(world: &mut World) -> primase_core::pass::RunReport {
        use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_pass_checks(true)
            .run(
                world,
                &Pipeline {
                    passes: vec![Box::new(LowerFromSoftMaterialEvents {
                        game: Game::EldenRing,
                    })],
                },
                &mut report,
            )
            .unwrap();
        report
    }
    /// Hand-authored typed reader facts, not a second decoder of the row bytes.
    fn rows() -> MaterialExRows {
        MaterialExRows {
            rows: std::collections::BTreeMap::from([(
                10300,
                MaterialExRow {
                    id: 10300,
                    parameter: "C[PixelKill]_Int".into(),
                    material_id: -1,
                    rgba_i: [0., 0., 0., 0., 1.],
                    bytes: vec![9, 8, 7],
                },
            )]),
            refusal: None,
        }
    }
    fn typed() -> Vec<EventValue> {
        vec![
            field("material_ex.row_id", EventValueData::I64(10300)),
            field("material_ex.raw_08_f32", EventValueData::F64(0.5)),
        ]
    }
    #[test]
    fn row_routing_is_typed_but_event_payload_words_are_not_assigned_envelope_meanings() {
        let rows = rows();
        let (fields, reason, inferred) = facts(&typed(), Game::EldenRing, Some(&rows));
        assert!(
            fields
                .iter()
                .any(|f| f.name == "material_ex.resolved.override_key"
                    && f.value == EventValueData::I64(i64::from(PIXEL_KILL_KEY)))
        );
        assert!(inferred);
        assert!(reason.contains("no channel applied"));
        assert!(facts(&typed(), Game::Sekiro, Some(&rows)).1.contains("unproven"));
        assert!(
            facts(&[], Game::EldenRing, Some(&rows))
                .1
                .contains("no unique typed")
        );
    }
    #[test]
    fn material_value_overrides_metadata_default_without_turning_a_kill_amount_into_a_boolean() {
        let parameter = fromsoft_formats::metaparam::MtdParam {
            name: "float_2".into(),
            start: 45,
            sequence_index: 0,
            value_type: 1,
            not_shader_input: false,
            srgb: false,
            colour: false,
            key: PIXEL_KILL_KEY,
            default: [0.; 5],
        };
        assert_eq!(parameter_value(&parameter, None).unwrap(), [0.]);
        assert_eq!(
            parameter_value(&parameter, Some(ParameterValue::Float(vec![0.5]))).unwrap(),
            [0.5]
        );
        assert_eq!(
            parameter_value(&parameter, Some(ParameterValue::Float(vec![1.]))).unwrap(),
            [1.]
        );
    }
    #[test]
    fn clip_only_staging_keeps_every_event_and_refuses_application_idempotently() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert_resource(rows());
        world.insert(
            entity,
            Clip {
                name: "bloom".into(),
                duration: 12.,
                channels: vec![],
                extrapolation: primase_core::Extrapolation::Hold,
                root_motion: None,
                additive: None,
            },
        );
        // Opaque bytes deliberately do not encode a row id. Only the typed input may route it.
        let bytes = vec![42, 17, 23];
        world.insert(
            entity,
            ClipEvents {
                origin: Some("hand-authored typed fixture".into()),
                refusals: vec![],
                events: vec![primase_core::ClipEvent {
                    kind: 198,
                    name: None,
                    naming: None,
                    start: 0.5,
                    end: Some(3.47),
                    values: typed(),
                    bytes: bytes.clone(),
                }],
            },
        );
        let report = run(&mut world);
        let first = world.get::<ClipEvents>(entity).unwrap().clone();
        assert_eq!(report.steps[0].log.approximations[ALL_MATERIALS_INFERENCE], 1);
        run(&mut world);
        assert_eq!(&first, world.get::<ClipEvents>(entity).unwrap());
        assert_eq!(first.events[0].bytes, bytes);
        assert_eq!(first.events[0].start, 0.5);
        assert!(world.get::<Clip>(entity).unwrap().channels.is_empty());
        assert!(
            first
                .refusals
                .iter()
                .any(|reason| reason.contains("value-at-time envelope is unproven"))
        );
        assert!(
            first.events[0].values.contains(&typed()[1]),
            "reader's raw typed field survives enrichment"
        );
    }
    #[test]
    fn lowering_writes_physical_and_verified_logical_defaults_from_the_material() {
        let mut world = World::new();
        let shader = world.spawn();
        let material = world.spawn();
        let parameter = fromsoft_formats::metaparam::MtdParam {
            name: "float_2".into(),
            start: 45,
            sequence_index: 0,
            value_type: 1,
            not_shader_input: false,
            srgb: false,
            colour: false,
            key: PIXEL_KILL_KEY,
            default: [0.; 5],
        };
        let metadata = fromsoft_formats::metaparam::Metaparam {
            constant_buffer_size: 192,
            shader_id: 0,
            flags: 0,
            gxmd_key: 0,
            ca_param_block_size: 0,
            ca_param_components: 0,
            unknown_30: 0,
            unknown_40: 0,
            unknown_50: 0,
            textures: vec![],
            passes: vec![],
            ca_params: vec![],
            mtd_params: vec![parameter],
        };
        world.insert(
            shader,
            ShaderBundle {
                name: "fixture".into(),
                game: Game::EldenRing,
                programs: vec![],
                parameters: Some(metadata),
            },
        );
        world.insert(material, MaterialShader(shader));
        world.insert(
            material,
            LayeredMaterial {
                mtd: "fixture".into(),
                bindings: Default::default(),
                definition: fromsoft_formats::mtd::MaterialDefinition {
                    shader: "fixture".into(),
                    description: String::new(),
                    textures: vec![],
                    block_lengths: vec![],
                    padding: vec![],
                    parameters: vec![fromsoft_formats::mtd::Parameter {
                        name: "float_2".into(),
                        value: fromsoft_formats::mtd::Value::Float(vec![0.5]),
                    }],
                },
            },
        );
        run(&mut world);
        let defaults = world.get::<MaterialParameters>(material).unwrap();
        assert_eq!(defaults.0["float_2"], [0.5]);
        assert_eq!(defaults.0["C[PixelKill]_Int"], [0.5]);
        defaults.verify(material, &world).unwrap();
    }
}
