//! One initial evaluation of Sekiro's common event 100, from the installed script.
//! Unknown flags stay unknown. Conditions are combined before an undecidable branch
//! is refused; no story phase or hour-to-package table is supplied by this reader.
use std::collections::{BTreeMap, BTreeSet};

use fromsoft_formats::{Game, emedf, emevd::Script};
use primase_core::{
    World,
    state::{Domain, StateAssignment, StateAxis},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// An independently observed snapshot, named by the caller with its source.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagSet {
    pub name: String,
    pub source: String,
    pub flags: BTreeMap<u32, bool>,
}

/// Actions of the selected initialization path, not actions performed in a game.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub groups: BTreeMap<i32, bool>,
    pub envmap: Option<u32>,
    pub animations: BTreeMap<i32, i32>,
    pub gparam_sub_ids: BTreeMap<String, i32>,
    pub written_flags: BTreeMap<u32, bool>,
    pub instructions: Vec<usize>,
}

/// Source and exact installed script of a selection. Flags are not reconstructed
/// from its output; their independently obtained provenance belongs to the input.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub input: FlagSet,
    pub common_script_sha256: String,
    pub selection: Selection,
    pub sky: Option<String>,
    pub assignments: BTreeMap<String, u32>,
}
impl Report {
    pub fn new(input: FlagSet, script: &[u8], selection: Selection) -> Self {
        Self {
            input,
            common_script_sha256: Sha256::digest(script)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            selection,
            sky: None,
            assignments: BTreeMap::new(),
        }
    }
}

/// Join the enabled script groups to actual dome membership and the reader's
/// existing axis. Numeric group ids are not converted into model/state ordinals.
pub fn selected_sky(values: &[String], masks: &[u64]) -> Result<String, String> {
    let mask = masks.iter().fold(0_u64, |a, &b| a | b);
    if mask.count_ones() != 1 {
        return Err(format!(
            "common event 100: enabled dome membership selects {} sky values",
            mask.count_ones()
        ));
    }
    values
        .get(mask.trailing_zeros() as usize)
        .cloned()
        .ok_or_else(|| "common event 100: enabled dome has no sky axis value".into())
}

/// Use the ordinary save-snapshot resource, preserving unresolved axis defaults.
pub fn assign(world: &mut World, area: &str, report: &mut Report) -> Result<(), String> {
    for (_, axis) in world.iter::<StateAxis>() {
        if let Some(flag) = axis
            .key
            .strip_prefix("fromsoft.flag.")
            .and_then(|s| s.parse::<u32>().ok())
            && let Some(&on) = report.input.flags.get(&flag)
        {
            report.assignments.insert(axis.key.clone(), u32::from(on));
        }
        let wanted = if axis.key == super::sky::AXIS {
            report.sky.clone()
        } else if axis.key == format!("fromsoft.irradiance.{area}") {
            report.selection.envmap.map(|n| format!("package-{n:02}"))
        } else {
            None
        };
        if let Some(wanted) = wanted {
            let Domain::Enum(values) = &axis.domain else {
                return Err(format!("{} is not an enum axis", axis.key));
            };
            let index = values
                .iter()
                .position(|v| *v == wanted)
                .ok_or_else(|| format!("common event 100: {} has no value {wanted}", axis.key))?;
            report.assignments.insert(axis.key.clone(), index as u32);
        }
    }
    let mut assignment = world.remove_resource::<StateAssignment>().unwrap_or_default();
    assignment.0.extend(report.assignments.clone());
    world.insert_resource(assignment);
    Ok(())
}

#[derive(Clone, Debug)]
struct Truth {
    value: Option<bool>,
    missing: BTreeSet<u32>,
}
impl Truth {
    fn known(value: bool) -> Self {
        Self {
            value: Some(value),
            missing: BTreeSet::new(),
        }
    }
    fn flag(flags: &BTreeMap<u32, bool>, id: u32) -> Self {
        flags.get(&id).map_or_else(
            || Self {
                value: None,
                missing: BTreeSet::from([id]),
            },
            |&value| Self::known(value),
        )
    }
    fn not(mut self) -> Self {
        self.value = self.value.map(|v| !v);
        self
    }
    fn combine(mut self, other: Self, and: bool) -> Self {
        let settles = !and;
        if self.value == Some(settles) || other.value == Some(settles) {
            return Self::known(settles);
        }
        self.missing.extend(other.missing);
        self.value = self
            .value
            .zip(other.value)
            .map(|(a, b)| if and { a && b } else { a || b });
        self
    }
    fn branch(self, context: &str) -> Result<bool, String> {
        self.value
            .ok_or_else(|| format!("{context}: undecidable; flags not supplied: {:?}", self.missing))
    }
}

fn all_flags(flags: &BTreeMap<u32, bool>, first: u32, last: u32, on: bool) -> Truth {
    let mut next = u64::from(first);
    let mut missing = None;
    for (&id, &value) in flags.range(first..=last) {
        if value != on {
            return Truth::known(false);
        }
        if u64::from(id) > next && missing.is_none() {
            missing = Some(next as u32);
        }
        next = u64::from(id) + 1;
    }
    if next <= u64::from(last) && missing.is_none() {
        missing = Some(next as u32);
    }
    missing.map_or_else(
        || Truth::known(true),
        |id| Truth {
            value: None,
            missing: BTreeSet::from([id]),
        },
    )
}

fn bool_value(value: i64, what: &str) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!("{what}: unsupported boolean {value}")),
    }
}

/// Evaluate the installed event's ordered initialization, stopping before its
/// map-change wait. The subsequent restart is a future transition, not this snapshot.
pub fn evaluate(script: &Script, input: &FlagSet, map: [u8; 2]) -> Result<Selection, String> {
    if input.name.trim().is_empty() || input.source.trim().is_empty() {
        return Err("world-state flags need a nonempty name and source".into());
    }
    let event = script
        .events
        .iter()
        .find(|e| e.id == 100)
        .ok_or("common.emevd has no event 100")?;
    if !event.parameters.is_empty() {
        return Err("common.emevd event 100: parameter substitutions are not understood".into());
    }
    let typed = emedf::decode_event(Game::Sekiro, event);
    let mut labels = BTreeMap::new();
    for (at, instruction) in event.instructions.iter().enumerate() {
        if instruction.bank == 1014 && labels.insert(instruction.id, at).is_some() {
            return Err(format!(
                "common.emevd event 100: duplicate Label {}",
                instruction.id
            ));
        }
    }
    let mut flags = input.flags.clone();
    let mut conditions: BTreeMap<i8, Truth> = BTreeMap::new();
    let mut result = Selection::default();
    let mut pc = 0;
    let mut steps = 0;
    while let Some(instruction) = event.instructions.get(pc) {
        steps += 1;
        if steps > event.instructions.len().saturating_mul(2) {
            return Err("common.emevd event 100: initialization loop is not understood".into());
        }
        let decoded = typed[pc]
            .as_ref()
            .map_err(|e| format!("common.emevd event 100 instruction {pc}: {e:?}"))?;
        let context = format!(
            "common.emevd event 100 instruction {pc} {} ({}:{})",
            decoded.def.name, instruction.bank, instruction.id
        );
        let arg = |n: usize| -> Result<i64, String> {
            decoded
                .args
                .get(n)
                .and_then(|a| a.value.as_i64())
                .ok_or_else(|| format!("{context}: argument {n} is not an integer"))
        };
        let flag_id = |n: usize| -> Result<u32, String> {
            u32::try_from(arg(n)?).map_err(|_| format!("{context}: invalid flag id"))
        };
        let test_flag = |state: usize, kind: usize, id: usize| -> Result<Truth, String> {
            if arg(kind)? != 0 {
                return Err(format!(
                    "{context}: target event flag type {} is not understood",
                    arg(kind)?
                ));
            }
            let value = Truth::flag(&flags, flag_id(id)?);
            Ok(if bool_value(arg(state)?, &context)? {
                value
            } else {
                value.not()
            })
        };
        let condition = |number: i8| -> Result<Truth, String> {
            conditions
                .get(&number)
                .cloned()
                .ok_or_else(|| format!("{context}: condition group {number} is not defined"))
        };
        let jump = |label: i64| -> Result<usize, String> {
            i32::try_from(label)
                .ok()
                .and_then(|l| labels.get(&l).copied())
                .ok_or_else(|| format!("{context}: Label {label} is not defined"))
        };
        result.instructions.push(pc);
        let mut next = pc + 1;
        let mut predicate: Option<(i8, Truth)> = None;
        match (instruction.bank, instruction.id) {
            (1014, _) => {}
            (3, 0) => predicate = Some((arg(0)? as i8, test_flag(1, 2, 3)?)),
            (3, 1) => {
                if arg(2)? != 0 {
                    return Err(format!("{context}: non-event flag range is not understood"));
                }
                let (first, last) = (flag_id(3)?, flag_id(4)?);
                if first > last || last > 99_999_999 {
                    return Err(format!("{context}: invalid flag range {first}..{last}"));
                }
                let operation = arg(1)?;
                if !(0..=3).contains(&operation) {
                    return Err(format!(
                        "{context}: logical operation {operation} is not understood"
                    ));
                }
                let off = operation == 1 || operation == 2;
                let mut value = all_flags(&flags, first, last, !off);
                if operation >= 2 {
                    value = value.not();
                }
                predicate = Some((arg(0)? as i8, value));
            }
            (3, 8) => {
                let matches = decoded.args[2..4]
                    .iter()
                    .zip(map)
                    .all(|(want, actual)| want.value.as_byte().is_some_and(|v| v == 255 || v == actual));
                predicate = Some((
                    arg(0)? as i8,
                    Truth::known(matches == bool_value(arg(1)?, &context)?),
                ));
            }
            (0, 0) => {
                let value = condition(arg(2)? as i8)?;
                predicate = Some((
                    arg(0)? as i8,
                    if bool_value(arg(1)?, &context)? {
                        value
                    } else {
                        value.not()
                    },
                ));
            }
            (1003, 101) => {
                if test_flag(1, 2, 3)?.branch(&context)? {
                    next = jump(arg(0)?)?;
                }
            }
            (1000, 101) | (1000, 1) => {
                let value = condition(arg(2)? as i8)?;
                let value = if bool_value(arg(1)?, &context)? {
                    value
                } else {
                    value.not()
                };
                if value.branch(&context)? {
                    next = if instruction.id == 101 {
                        jump(arg(0)?)?
                    } else {
                        let skip =
                            usize::try_from(arg(0)?).map_err(|_| format!("{context}: negative skip"))?;
                        pc.checked_add(skip + 1)
                            .ok_or_else(|| format!("{context}: skip overflows"))?
                    };
                }
            }
            (1000, 103) => next = jump(arg(0)?)?,
            (2012, 1) => {
                let id =
                    i32::try_from(arg(0)?).map_err(|_| format!("{context}: map part id does not fit"))?;
                result.groups.insert(id, bool_value(arg(1)?, &context)?);
            }
            (2003, 2) => {
                let id = flag_id(0)?;
                let value = bool_value(arg(1)?, &context)?;
                flags.insert(id, value);
                result.written_flags.insert(id, value);
            }
            (2003, 18) => {
                result.animations.insert(
                    i32::try_from(arg(0)?).map_err(|_| format!("{context}: invalid entity"))?,
                    i32::try_from(arg(1)?).map_err(|_| format!("{context}: invalid animation"))?,
                );
            }
            (2003, 80) => {
                result.envmap =
                    Some(u32::try_from(arg(0)?).map_err(|_| format!("{context}: invalid envmap"))?)
            }
            (2003, 88) => {
                if !matches!(decoded.args.get(3).map(|a| a.value), Some(emedf::Value::F32(0.0))) {
                    return Err(format!("{context}: timed GPARAM blending is not understood"));
                }
                result.gparam_sub_ids.insert(
                    format!("m{:02}_{:02}", arg(0)?, arg(1)?),
                    i32::try_from(arg(2)?).map_err(|_| format!("{context}: invalid GPARAM sub id"))?,
                );
            }
            (12, 4) => {
                if result.envmap.is_none() || !result.groups.values().any(|&on| on) {
                    return Err(format!(
                        "{context}: map-change wait reached without an initial sky/envmap selection"
                    ));
                }
                return Ok(result);
            }
            _ => {
                return Err(format!(
                    "{context}: instruction is not understood by the world-state selector"
                ));
            }
        }
        if let Some((group, value)) = predicate {
            if !(-15..=15).contains(&group) {
                return Err(format!("{context}: condition group {group} is not understood"));
            }
            if group == 0 {
                return Err(format!(
                    "{context}: blocking MAIN condition before selection is not understood"
                ));
            }
            let previous = conditions
                .remove(&group)
                .unwrap_or_else(|| Truth::known(group > 0));
            conditions.insert(group, previous.combine(value, group > 0));
        }
        pc = next;
    }
    Err("common.emevd event 100: initialization ended before the map-change wait".into())
}
