//! The area's draw-param file: the graphics environment (lighting, fog, tone, sky) the game applies there, a GPARAM
//! ([`crate::gparam`]) in `/param/drawparam/`, kept as stored for `lower-fromsoft-environment` to interpret.
//!
//! Where it is. Both games keep the files of a map in `/param/drawparam/<mNN_NN>_<id>.gparam.dcx` (the map's first two
//! blocks, then a four-digit ID: Sekiro's `m11_01_0720`; Elden Ring's `m10_00_0100`, and for its open-world tiles the
//! shared `m60_00_0010`, `m61_00_0050`: no file carries a tile's coordinates). The archive's name dictionaries list
//! those names and no others of the set (Elden Ring adds `_weatherbase`, `_weatheroutdoor`, `_commonevent` and
//! `_capturemap` layers, which are not read).
//!
//! Which file. The MSB does not name one: a part's graphics config (`msb::PartGparam`) holds *value IDs* inside the
//! groups of the file in force, and a collision part's scene config holds four signed bytes that Elden Ring's
//! SoulsFormats names the hundreds digits of the files to use (`GparamSubID_Base` and three overrides) and Sekiro's does
//! not explain. How the engine turns those and the area's events and weather into one file is not established, so the
//! file is the caller's choice ([`DrawParams`]) from the ones the archive holds ([`available`]); the scene bytes are
//! counted in the read's notices.
//!
//! Which values. A file holds, for each field, one value per value ID (and time of day). The area's parts name the ID
//! of four groups (`LightSet ParamEditor`, `FogParamEditor`, `LightScattering : ParamEditor`, `Env Map:Editor`): the ID
//! most of them name is read. Every other group has no part-level ID: its one value set, else the set of ID 0, is read,
//! and a group with several sets and no ID 0 is not selected. FogVolume sets are all retained as unresolved
//! local-volume source data; they are never selected through the part's FogParamEditor slot.
use super::super::source::{DrawField, DrawGroup, DrawParam, SetChosen};
use crate::{
    Archive, Game, Result,
    dcx::Decoder,
    gparam::{Gparam, Unread},
    msb::PartGparam,
};
use primase_core::convert::{Notice, NoticeKind};
use std::collections::BTreeMap;

const LIGHT_SET: &str = "LightSet ParamEditor";
const FOG: &str = "FogParamEditor";
/// Local injected-volume parameters, not the part's `FogParamEditor` selector. The recorded
/// `tests-a0b6470ed/draw_param_census.log` has m10_00 sets 100/101 without a nonnegative part fog id;
/// m12_02 has sets 77..94. SoulsFormats MSBS/MSBE region/event definitions establish no replacement
/// selector, and Smithbox's Fog Volume annotations describe local volume injection. No stored join
/// from a placed volume to one of these sets is established. Retain every set and time sample as
/// `SetChosen::Unresolved`; neither an equal numeric part id nor the presence of id 0 proves selection.
const FOG_VOLUME: &str = "FogVolume ParamEditor";
const LIGHT_SCATTERING: &str = "LightScattering : ParamEditor";
const ENV_MAP: &str = "Env Map:Editor";

/// What a read does about the area's draw-param files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DrawParams {
    /// None read (default).
    #[default]
    None,
    /// The file with this stem (`m11_01_0720`), one of [`available`].
    File(String),
}

/// What the read of the draw-param file found and left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Whether a file was asked for.
    pub asked: bool,
    /// The stems the archive holds for the area's map: what a caller may ask for.
    pub available: Vec<String>,
    /// The file read, its GPARAM version, the groups kept and the fields in them.
    pub file: Option<String>,
    pub version: u32,
    pub groups: usize,
    pub fields: usize,
    /// Asked for and not among [`Report::available`].
    pub missing: Option<String>,
    /// Asked for and not readable: why.
    pub unreadable: Option<String>,
    /// Parts naming another value set than the one read, by `<group key> <id>`.
    pub other_ids: BTreeMap<String, usize>,
    /// Value IDs parts name that the group does not store, by `<group key> <id>`, and the parts naming them.
    pub absent_ids: BTreeMap<String, usize>,
    /// Groups no part names an ID for, read at ID 0.
    pub zero_id_groups: Vec<String>,
    /// Groups with no value set chosen (several sets and no ID 0, or the named ID absent), and their fields.
    pub unchosen_groups: Vec<String>,
    pub unchosen_fields: usize,
    /// The unchosen groups that store fields, by key: the value IDs they store and how many fields they hold, so a
    /// rule that leaves a group can be read against the IDs it left. (A group that stores no field is in
    /// [`Report::unchosen_groups`] and [`Report::empty_groups`], not here.)
    pub unchosen_sets: BTreeMap<String, (Vec<i32>, usize)>,
    /// Groups of the file that store no field at all: a file's list of groups is the same shells whatever it holds, and
    /// a file that stores nothing (Sekiro's `m20_00_0010`) has every one of them empty.
    pub empty_groups: usize,
    /// The size of the decoded file, bytes.
    pub bytes: usize,
    /// Fields of a kept group that store nothing for the chosen ID.
    pub fields_without_set: usize,
    /// Further values stored for a chosen ID, at other times of day: not read.
    pub time_variants: usize,
    /// What the file stores that the GPARAM reader does not interpret.
    pub unread: Unread,
    /// The scene configs of the collision parts, by their four bytes: parts storing them.
    pub scenes: BTreeMap<String, usize>,
    /// The four value-set slots every kept or collision part's graphics config stores, as `<slot> <id>` (as stored,
    /// negative for none): the parts naming it. What a group's selector can be read against.
    pub slots: BTreeMap<String, usize>,
    /// The game's layered files (`_weatherbase` ...) exist and are not read.
    pub layers_not_read: bool,
    /// The explicit static preview hour, if requested. None keeps the first stored value.
    pub hour: Option<f32>,
}

/// The stem the files of an area's map share: `m11_01` for `m11_01_00_00`; Elden Ring's open-world tiles share
/// `m60_00` and `m61_00`. `None` for a name that is not an area's.
fn map_stem(game: Game, area: &str) -> Option<String> {
    let first = area.get(..3)?;
    let second = area.get(4..6)?;
    Some(match (game, first) {
        (Game::EldenRing, "m60" | "m61") => format!("{first}_00"),
        _ => format!("{first}_{second}"),
    })
}

fn path(stem: &str) -> String {
    format!("/param/drawparam/{stem}.gparam.dcx")
}

/// The draw-param files the archive holds for `area`'s map, by stem, in ID order: every `<map>_<0000..9999>`.
pub fn available(archive: &Archive, game: Game, area: &str) -> Vec<String> {
    let Some(map) = map_stem(game, area) else {
        return Vec::new();
    };
    (0..10_000)
        .map(|id| format!("{map}_{id:04}"))
        .filter(|stem| archive.contains(&path(stem)))
        .collect()
}

/// The value ID a part's graphics config gives a group, for the four groups that have one.
fn part_id(group: &str) -> Option<fn(&PartGparam) -> i32> {
    match group {
        LIGHT_SET => Some(|c| c.light_set),
        FOG => Some(|c| c.fog),
        LIGHT_SCATTERING => Some(|c| c.light_scattering),
        ENV_MAP => Some(|c| c.env_map),
        _ => None,
    }
}

/// The value ID most parts name (the lowest of those that tie), the parts naming it and the others named. A negative ID
/// names none.
struct Dominant {
    id: i32,
    parts: usize,
    others: BTreeMap<i32, usize>,
}

fn dominant(ids: impl Iterator<Item = i32>) -> Option<Dominant> {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for id in ids.filter(|id| *id >= 0) {
        *counts.entry(id).or_default() += 1;
    }
    let (&id, &parts) = counts
        .iter()
        .max_by_key(|(id, n)| (**n, std::cmp::Reverse(**id)))?;
    counts.remove(&id);
    Some(Dominant {
        id,
        parts,
        others: counts,
    })
}

/// The groups of `gparam` at the value sets `configs` (the area's parts) name, counted into `report`.
pub(super) fn select(gparam: &Gparam, stem: &str, configs: &[PartGparam], report: &mut Report) -> DrawParam {
    let mut groups = Vec::new();
    for group in &gparam.groups {
        let ids = group.ids();
        let named = part_id(&group.key).and_then(|of| dominant(configs.iter().map(of)));
        let pick = match named {
            Some(d) => {
                for (other, parts) in &d.others {
                    *report
                        .other_ids
                        .entry(format!("{} {other}", group.key))
                        .or_default() += parts;
                }
                if ids.contains(&d.id) {
                    Some((d.id, SetChosen::ByParts))
                } else {
                    *report
                        .absent_ids
                        .entry(format!("{} {}", group.key, d.id))
                        .or_default() += d.parts;
                    None
                }
            }
            None if group.key == FOG_VOLUME => None,
            None if ids.len() == 1 => Some((ids[0], SetChosen::OnlySet)),
            None if ids.contains(&0) => {
                report.zero_id_groups.push(group.key.clone());
                Some((0, SetChosen::ZeroId))
            }
            None => None,
        };
        if group.fields.is_empty() {
            report.empty_groups += 1;
        }
        if pick.is_none() {
            report.unchosen_groups.push(group.key.clone());
            report.unchosen_fields += group.fields.len();
            if !group.fields.is_empty() {
                report
                    .unchosen_sets
                    .insert(group.key.clone(), (ids.clone(), group.fields.len()));
            }
        }
        let picks: Vec<_> = match pick {
            Some(pick) => vec![pick],
            None if group.key == FOG_VOLUME => ids.iter().map(|&id| (id, SetChosen::Unresolved)).collect(),
            None => Vec::new(),
        };
        for (id, chosen) in picks {
        let mut fields = Vec::new();
        for field in &group.fields {
            let at: Vec<_> = field.indices_of(id).collect();
            if at.is_empty() {
                report.fields_without_set += 1;
                continue;
            }
            if chosen != SetChosen::Unresolved {
                report.time_variants += at.len().saturating_sub(1);
            }
            let keep = if chosen == SetChosen::Unresolved { at.len() } else { 1 };
            for index in at.into_iter().take(keep) {
            let Some(value) = field.values.get(index) else {
                report.fields_without_set += 1;
                continue;
            };
            fields.push(DrawField {
                key: field.key.clone(),
                name: field.name.clone(),
                value,
                time: field.times.get(index).copied(),
            });
            }
        }
        report.groups += 1;
        report.fields += fields.len();
        groups.push(DrawGroup {
            key: group.key.clone(),
            name: group.name.clone(),
            id,
            chosen,
            fields,
        });
        }
    }
    report.file = Some(stem.to_owned());
    report.version = gparam.version;
    report.unread = gparam.unread;
    DrawParam {
        file: stem.to_owned(),
        version: gparam.version,
        groups,
    }
}

/// Evaluate already selected groups at the caller's hour; unselected local-volume sets stay as source samples.
fn sample_hour(gparam: &Gparam, source: &mut DrawParam, hour: f32) -> std::result::Result<(), String> {
    if !hour.is_finite() || !(0.0..=24.0).contains(&hour) { return Err("hour must be finite and in 0..=24".into()); }
    for group in &mut source.groups {
        if group.chosen == SetChosen::Unresolved { continue; }
        let stored = gparam.groups.iter().find(|stored| stored.key == group.key)
            .ok_or_else(|| format!("{}: source group missing while sampling hour",group.key))?;
        for field in &mut group.fields {
            let stored = stored.field(&field.key).ok_or_else(|| format!("{}: source field missing",field.key))?;
            field.value = stored.sample_at_hour(group.id,hour)?
                .ok_or_else(|| format!("{}: selected value id {} missing",field.key,group.id))?;
            field.time = Some(hour.rem_euclid(24.0));
        }
    }
    Ok(())
}

/// Read what `choice` asks for. `configs` are the graphics configs of the parts the read keeps and of the area's
/// collision parts.
pub(super) fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    area: &str,
    choice: &DrawParams,
    configs: &[PartGparam],
    hour: Option<f32>,
) -> Result<(Option<DrawParam>, Report)> {
    if hour.is_some_and(|hour| !hour.is_finite() || !(0.0..=24.0).contains(&hour) || *choice == DrawParams::None) {
        return Err(crate::invalid("GPARAM",0,"hour requires a draw-param file and a finite value in 0..=24"));
    }
    let DrawParams::File(stem) = choice else {
        return Ok((None, Report::default()));
    };
    let mut report = Report {
        asked: true,
        available: available(archive, game, area),
        layers_not_read: game == Game::EldenRing,
        hour,
        ..Report::default()
    };
    for scene in configs.iter().filter_map(|c| c.scene) {
        *report.scenes.entry(format!("{scene:?}")).or_default() += 1;
    }
    for c in configs {
        for (slot, id) in [
            ("light_set", c.light_set),
            ("fog", c.fog),
            ("light_scattering", c.light_scattering),
            ("env_map", c.env_map),
        ] {
            *report.slots.entry(format!("{slot} {id}")).or_default() += 1;
        }
    }
    if !report.available.contains(stem) {
        report.missing = Some(stem.clone());
        return Ok((None, report));
    }
    let bytes = archive.read(&path(stem), decoder)?;
    report.bytes = bytes.len();
    let gparam = match Gparam::read(&bytes) {
        Ok(gparam) => gparam,
        Err(e) => {
            report.unreadable = Some(e.to_string());
            return Ok((None, report));
        }
    };
    let mut source = select(&gparam, stem, configs, &mut report);
    if let Some(hour) = hour {
        if let Err(why) = sample_hour(&gparam,&mut source,hour) {
            report.unreadable = Some(why);
            return Ok((None,report));
        }
        report.time_variants = 0;
    }
    Ok((Some(source), report))
}

impl Report {
    /// What the read of the draw-param file left out or chose, as notices.
    pub fn notices(&self) -> Vec<Notice> {
        let stage = "fromsoft reader";
        let mut out = Vec::new();
        if !self.asked {
            return out;
        }
        let listed = |kind, reason: &str, items: Vec<String>| {
            let count = items.len() as u64;
            items
                .into_iter()
                .take(8)
                .fold(Notice::new(stage, kind, reason, count), |n, i| n.example(i))
        };
        if let Some(stem) = &self.missing {
            out.push(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "draw-param file asked for that the archive does not hold for the area's map",
                    1,
                )
                .example(stem.clone()),
            );
        }
        if let Some(why) = &self.unreadable {
            out.push(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "draw-param file that could not be read (no environment)",
                    1,
                )
                .example(why.clone()),
            );
        }
        let Some(file) = &self.file else {
            return out;
        };
        if let Some(hour) = self.hour {
            out.push(Notice::new(stage,NoticeKind::Approximation,
                "explicit hour preview: GPARAM floats/vectors linearly interpolated in stored units over a 24-hour wrap; discrete fields hold the preceding key; this interpolation and raw angle interpolation are preview policies, not a measured engine curve",
                1).example(format!("{file}, hour {hour}")));
        }
        let others: Vec<String> = self.available.iter().filter(|s| *s != file).cloned().collect();
        if !others.is_empty() {
            out.push(listed(
                NoticeKind::Selection,
                "draw-param files of the area's map other than the one read (how the game chooses one is not established)",
                others,
            ));
        }
        if !self.scenes.is_empty() {
            out.push(self.scenes.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "collision parts storing a scene config (four bytes that choose draw-param files), not used to choose",
                    self.scenes.values().sum::<usize>() as u64,
                ),
                |n, (bytes, parts)| n.example(format!("{parts} x {bytes}")),
            ));
        }
        if !self.other_ids.is_empty() {
            out.push(self.other_ids.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "parts naming another value set than the one read (the one most parts name is read)",
                    self.other_ids.values().sum::<usize>() as u64,
                ),
                |n, (set, parts)| n.example(format!("{set}: {parts} parts")),
            ));
        }
        if !self.absent_ids.is_empty() {
            out.push(self.absent_ids.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "value sets parts name that the draw-param file does not store (the group is not read; a later file of a map stores only what it changes, so where it stores no field in the group the base file holds the set)",
                    self.absent_ids.values().sum::<usize>() as u64,
                ),
                |n, (set, parts)| n.example(format!("{set}: {parts} parts")),
            ));
        }
        if !self.zero_id_groups.is_empty() {
            out.push(listed(
                NoticeKind::Approximation,
                "draw-param groups no part names a value set for, read at value ID 0",
                self.zero_id_groups.clone(),
            ));
        }
        let fog_sets = self.unchosen_sets.get(FOG_VOLUME);
        if let Some((ids, _)) = fog_sets {
            out.push(Notice::new(stage, NoticeKind::Unsupported,
                "FogVolume source sets and every time sample retained in fromsoft.draw_param with selection Unresolved; no established placed-volume selector, so none is applied to the scene",
                ids.len() as u64).example(format!("{FOG_VOLUME}: {ids:?}")));
        }
        let unread_fields = self.unchosen_fields.saturating_sub(fog_sets.map_or(0, |(_, fields)| *fields));
        if unread_fields > 0 {
            // counted in fields, named by group (a group that stores no field has none to leave: counted below)
            out.push(
                self.unchosen_groups
                    .iter()
                    .filter(|g| g.as_str() != FOG_VOLUME && self.unchosen_sets.contains_key(*g))
                    .take(8)
                    .fold(
                        Notice::new(
                            stage,
                            NoticeKind::Unsupported,
                            "draw-param groups with no value set to read, their fields not read",
                            unread_fields as u64,
                        ),
                        |n, group| n.example(group.clone()),
                    ),
            );
        }
        if self.empty_groups > 0 {
            out.push(Notice::new(
                stage,
                NoticeKind::Selection,
                if self.groups == 0 && self.fields == 0 {
                    "draw-param groups the file stores no field in: the file stores no value at all (an overlay with nothing in it, not a read that chose nothing)"
                } else {
                    "draw-param groups the file stores no field in (nothing to read in them)"
                },
                self.empty_groups as u64,
            ));
        }
        for (count, reason) in [
            (
                self.fields_without_set,
                "draw-param fields of a read group that store nothing for its value set",
            ),
            (
                self.time_variants,
                "draw-param values stored for a chosen value ID at other times of day, not read",
            ),
            (
                self.unread.extras,
                "draw-param extras (records tying value IDs of groups together), not interpreted",
            ),
            (
                usize::from(self.unread.data_bytes > 0),
                "draw-param opaque block, not interpreted",
            ),
            (
                usize::from(self.layers_not_read),
                "draw-param layers of the game's weather and events (`_weatherbase`, `_weatheroutdoor`, `_commonevent`), not read",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, NoticeKind::Unsupported, reason, count as u64));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gparam::{Field, Group, Value, Values};

    #[test]
    fn selected_environment_fields_sample_the_requested_hour() {
        let gparam=Gparam {version:5,unread:Unread::default(),groups:vec![group(LIGHT_SET,vec![
            Field {times:vec![6.0,18.0],..field("Brightness",&[7,7],Values::F32(vec![2.0,8.0]))}]) ]};
        let mut source=select(&gparam,"fixture",&[config(7,-1,-1)],&mut Report::default());
        sample_hour(&gparam,&mut source,12.0).unwrap();
        let light=source.group(LIGHT_SET).unwrap();
        assert_eq!(light.value("Brightness"),Some(&Value::Float(5.0)));
        assert_eq!(light.fields[0].time,Some(12.0));
    }

    fn field(key: &str, ids: &[i32], values: Values) -> Field {
        Field {
            key: key.into(),
            name: key.into(),
            ids: ids.to_vec(),
            times: vec![0.0; ids.len()],
            values,
        }
    }

    fn group(key: &str, fields: Vec<Field>) -> Group {
        Group {
            key: key.into(),
            name: key.into(),
            fields,
        }
    }

    fn config(light_set: i32, fog: i32, env_map: i32) -> PartGparam {
        PartGparam {
            part: 0,
            light_set,
            fog,
            light_scattering: -1,
            env_map,
            scene: None,
        }
    }

    fn file() -> Gparam {
        Gparam {
            version: 5,
            groups: vec![
                // sets 3 and 7; the part majority names 7; a time-of-day twin of set 7
                group(
                    LIGHT_SET,
                    vec![
                        field("Sharpness", &[3, 7, 7], Values::F32(vec![1.0, 2.0, 3.0])),
                        field("Colour", &[3, 7], Values::Vec4(vec![[0.; 4], [1., 0.5, 0.25, 1.]])),
                    ],
                ),
                // a single set, no part names it
                group(FOG, vec![field("Dist", &[0], Values::F32(vec![4.0]))]),
                // the parts name set 9, which the group does not store
                group(ENV_MAP, vec![field("Id", &[1, 2], Values::U32(vec![5, 6]))]),
                // no part-level ID: the set of ID 0
                group(
                    "Tone Map[Yebis]",
                    vec![field("Exposure", &[0, 1], Values::F32(vec![0.5, 2.0]))],
                ),
                // no part-level ID and no ID 0: not read
                group("Bloom", vec![field("Threshold", &[2, 5], Values::F32(vec![1.0, 2.0]))]),
            ],
            unread: Unread {
                extras: 2,
                data_bytes: 8,
            },
        }
    }

    #[test]
    fn the_value_set_most_parts_name_is_read_and_every_other_choice_is_counted() {
        let configs = [config(7, 0, 9), config(7, 0, 9), config(3, 0, 9), config(-1, -1, -1)];
        let mut report = Report::default();
        let source = select(&file(), "m11_01_0000", &configs, &mut report);
        assert_eq!((source.file.as_str(), source.version), ("m11_01_0000", 5));
        let light = source.group(LIGHT_SET).unwrap();
        assert_eq!((light.id, light.chosen), (7, SetChosen::ByParts));
        assert_eq!(light.value("Sharpness"), Some(&Value::Float(2.0)));
        assert_eq!(light.value("Colour"), Some(&Value::Vec4([1., 0.5, 0.25, 1.])));
        assert_eq!(report.other_ids, BTreeMap::from([(format!("{LIGHT_SET} 3"), 1)]));
        // the second value stored for set 7, at another time of day
        assert_eq!(report.time_variants, 1);
        let fog = source.group(FOG).unwrap();
        assert_eq!((fog.id, fog.chosen), (0, SetChosen::ByParts), "parts name ID 0 of the fog");
        let tone = source.group("Tone Map[Yebis]").unwrap();
        assert_eq!((tone.id, tone.chosen), (0, SetChosen::ZeroId));
        assert_eq!(tone.value("Exposure"), Some(&Value::Float(0.5)));
        assert_eq!(report.zero_id_groups, ["Tone Map[Yebis]"]);
        // set 9 is named by three parts and not stored; the group with several sets and no 0 is left
        assert!(source.group(ENV_MAP).is_none());
        assert_eq!(report.absent_ids, BTreeMap::from([(format!("{ENV_MAP} 9"), 3)]));
        assert!(source.group("Bloom").is_none());
        assert_eq!(report.unchosen_groups, [ENV_MAP, "Bloom"]);
        assert_eq!(report.unchosen_fields, 2);
        assert_eq!((report.groups, report.fields), (3, 4));
        assert_eq!(report.unread, Unread { extras: 2, data_bytes: 8 });
        assert_eq!(report.file.as_deref(), Some("m11_01_0000"));
    }

    #[test]
    fn a_group_with_one_set_is_read_whatever_the_parts_name_and_a_tie_takes_the_lowest() {
        let only = Gparam {
            version: 5,
            groups: vec![group("Fog volume", vec![field("A", &[4], Values::F32(vec![1.0]))])],
            unread: Unread::default(),
        };
        let mut report = Report::default();
        let source = select(&only, "x", &[config(1, 1, 1)], &mut report);
        let g = source.group("Fog volume").unwrap();
        assert_eq!((g.id, g.chosen), (4, SetChosen::OnlySet));
        // the parts' tie
        let d = dominant([5, 2, 5, 2, -1, -1, -1].into_iter()).unwrap();
        assert_eq!((d.id, d.parts, d.others), (2, 2, BTreeMap::from([(5, 2)])));
        assert!(dominant([-1, -1].into_iter()).is_none() && dominant(std::iter::empty()).is_none());
    }

    #[test]
    fn fog_volume_keeps_every_set_and_time_without_inventing_a_part_selector() {
        // Elden m10_00_0000: the fog volume stores sets 100 and 101 and no ID 0
        let fog_volume = Gparam {
            version: 5,
            groups: vec![group(
                FOG_VOLUME,
                vec![Field { times: vec![0.0,12.0,0.0],
                    ..field("Density", &[100, 100, 101], Values::F32(vec![1.0, 2.0, 3.0])) }],
            )],
            unread: Unread::default(),
        };
        for fog in [-1,90,101] {
            let mut report = Report { asked:true,..Default::default() };
            let source = select(&fog_volume,"m10_00_0000",&[config(-1,fog,-1)],&mut report);
            assert!(source.group(FOG_VOLUME).is_none());
            assert_eq!(source.groups.iter().map(|g|(g.id,g.chosen)).collect::<Vec<_>>(),
                [(100,SetChosen::Unresolved),(101,SetChosen::Unresolved)]);
            assert_eq!(source.groups[0].fields.iter().map(|f|(&f.value,f.time)).collect::<Vec<_>>(),
                [(&Value::Float(1.0),Some(0.0)),(&Value::Float(2.0),Some(12.0))]);
            assert_eq!(source.groups[1].value("Density"),Some(&Value::Float(3.0)));
            assert!(report.other_ids.is_empty() && report.absent_ids.is_empty());
            assert_eq!(report.unchosen_sets[FOG_VOLUME],(vec![100,101],1));
            assert!(report.notices().iter().any(|n|n.reason.contains("selection Unresolved")));
        }
    }

    #[test]
    fn a_field_with_nothing_for_the_set_is_counted() {
        let g = Gparam {
            version: 5,
            groups: vec![group(
                LIGHT_SET,
                vec![
                    field("Has", &[1], Values::F32(vec![1.0])),
                    field("Lacks", &[2], Values::F32(vec![1.0])),
                ],
            )],
            unread: Unread::default(),
        };
        let mut report = Report::default();
        let source = select(&g, "x", &[config(1, -1, -1)], &mut report);
        assert_eq!(source.group(LIGHT_SET).unwrap().fields.len(), 1);
        assert_eq!(report.fields_without_set, 1);
    }

    #[test]
    fn a_file_whose_groups_store_nothing_is_empty_not_unchosen() {
        let shells = Gparam {
            version: 5,
            groups: vec![group(LIGHT_SET, vec![]), group("Tone Map[Yebis]", vec![]), group(FOG, vec![])],
            unread: Unread::default(),
        };
        let mut report = Report {
            asked: true,
            ..Report::default()
        };
        let source = select(&shells, "m20_00_0010", &[config(7, 0, 9)], &mut report);
        assert!(source.groups.is_empty());
        assert_eq!((report.groups, report.fields, report.empty_groups), (0, 0, 3));
        // the groups are left, but no field of them is: nothing is unchosen
        assert_eq!((report.unchosen_groups.len(), report.unchosen_fields), (3, 0));
        assert!(report.unchosen_sets.is_empty());
        let notices = report.notices();
        assert!(notices.iter().all(|n| !n.reason.starts_with("draw-param groups with no value set")));
        let n = notices.iter().find(|n| n.reason.contains("stores no value at all")).unwrap();
        assert_eq!(n.count, 3);
        // a group with fields it leaves is told apart, with the IDs it stores
        let mut mixed = Report::default();
        select(&file(), "x", &[config(7, 0, 9)], &mut mixed);
        assert_eq!(mixed.unchosen_sets["Bloom"], (vec![2, 5], 1));
        assert_eq!(mixed.empty_groups, 0);
    }

    #[test]
    fn the_notices_say_what_was_asked_chosen_and_left() {
        // not asked: nothing
        assert!(Report::default().notices().is_empty());
        let find = |notices: &[Notice], start: &str| notices.iter().position(|n| n.reason.starts_with(start));
        // asked for a file the archive lacks
        let missing = Report {
            asked: true,
            missing: Some("m11_01_0999".into()),
            ..Report::default()
        };
        let notices = missing.notices();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, NoticeKind::Unsupported);
        assert_eq!(notices[0].examples, ["m11_01_0999"]);
        // read
        let configs = [config(7, 0, 9), config(3, 0, 9)];
        let mut report = Report {
            asked: true,
            available: vec!["m11_01_0000".into(), "m11_01_0100".into()],
            layers_not_read: true,
            ..Report::default()
        };
        report.scenes.insert("[2, -1, -1, -1]".into(), 4);
        select(&file(), "m11_01_0000", &configs, &mut report);
        let notices = report.notices();
        let n = &notices[find(&notices, "draw-param files of the area's map other than").unwrap()];
        assert_eq!((n.kind, n.count), (NoticeKind::Selection, 1));
        assert_eq!(n.examples, ["m11_01_0100"]);
        let n = &notices[find(&notices, "collision parts storing a scene config").unwrap()];
        assert_eq!((n.count, n.examples.clone()), (4, vec!["4 x [2, -1, -1, -1]".to_owned()]));
        let n = &notices[find(&notices, "parts naming another value set").unwrap()];
        assert_eq!(n.count, 1);
        let n = &notices[find(&notices, "value sets parts name that the draw-param file does not store").unwrap()];
        assert_eq!((n.kind, n.count), (NoticeKind::Unsupported, 2));
        let n = &notices[find(&notices, "draw-param groups no part names").unwrap()];
        assert_eq!((n.kind, n.examples.clone()), (NoticeKind::Approximation, vec!["Tone Map[Yebis]".to_owned()]));
        // the groups left unread count their fields
        let n = &notices[find(&notices, "draw-param groups with no value set").unwrap()];
        assert_eq!(n.count, 2);
        assert_eq!(n.examples, [ENV_MAP, "Bloom"]);
        assert!(find(&notices, "draw-param extras").is_some());
        assert!(find(&notices, "draw-param opaque block").is_some());
        assert!(find(&notices, "draw-param layers of the game's weather").is_some());
    }

    #[test]
    fn a_map_names_its_files_by_its_first_two_blocks_and_open_world_tiles_share_one_set() {
        assert_eq!(map_stem(Game::Sekiro, "m11_01_00_00").as_deref(), Some("m11_01"));
        assert_eq!(map_stem(Game::EldenRing, "m10_00_00_00").as_deref(), Some("m10_00"));
        assert_eq!(map_stem(Game::EldenRing, "m12_02_00_00").as_deref(), Some("m12_02"));
        assert_eq!(map_stem(Game::EldenRing, "m60_44_36_00").as_deref(), Some("m60_00"));
        assert_eq!(map_stem(Game::EldenRing, "m61_50_50_00").as_deref(), Some("m61_00"));
        assert_eq!(map_stem(Game::Sekiro, "m6").as_deref(), None);
        assert_eq!(path("m11_01_0720"), "/param/drawparam/m11_01_0720.gparam.dcx");
    }
}
