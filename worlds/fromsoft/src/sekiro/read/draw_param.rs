//! The area's draw-param file: the graphics environment (lighting, fog, tone, sky) the game applies there, a GPARAM
//! ([`fromsoft_formats::gparam`]) in `/param/drawparam/`, kept as stored for `lower-fromsoft-environment` to interpret.
//!
//! Where it is. Both games keep the files of a map in `/param/drawparam/<mNN_NN>_<id>.gparam.dcx` (the map's first two
//! blocks, then a four-digit ID: Sekiro's `m11_01_0720`; Elden Ring's `m10_00_0100`, and for its open-world tiles the
//! shared `m60_00_0010`, `m61_00_0050`: no file carries a tile's coordinates). The archive's name dictionaries list
//! those names and no others of the set (Elden Ring adds `_weatherbase`, `_weatheroutdoor`, `_commonevent` and
//! `_capturemap` layers, which are not read).
//!
//! Which file. The MSB does not name one: a part's graphics config (`msb::PartGparam`) holds *value IDs* inside the
//! groups of the file in force. A file's four-digit id is a slot (the hundreds digit) and a sub id (the last two digits)
//! ([`fromsoft_formats::area::DrawParamId`]). A collision part's scene config names a file by them: its first scene byte
//! is the slot and, in Elden Ring, the byte at +32 the sub id (it names a file the install holds for all 1,304 Sekiro and
//! 3,209 of the 3,266 Elden Ring collisions); an area's events set the sub id too (Sekiro's
//! `2003:88`, Elden Ring's `2012:11`, [`fromsoft_formats::gparam_events`]). Two of the game's own tables settle more
//! (Elden Ring, [`fromsoft_formats::gparam_params`]): `GparamRefSettingsParam` names the map whose files a map uses (the caves
//! and tunnels, `m31` and `m32`, use the catacombs' `m30_00`), and `WeatherParam.GparamId` the global `m00_00_<id>_weatheroutdoor`
//! file of each weather. How the engine combines the collision under
//! the player, the events and the weather into one file is not established, so the file is the caller's choice
//! ([`DrawParams`]): a stem from the ones the archive holds ([`fromsoft_formats::area::draw_params`]), or the file one
//! collision part's scene config names. The events' switches and the ids the collisions name are in the read's report and
//! notices; none is applied.
//!
//! Which values. A file holds, for each field, one value per value ID (and time of day). The area's parts name the ID
//! of four groups (`LightSet ParamEditor`, `FogParamEditor`, `LightScattering : ParamEditor`, `Env Map:Editor`): the ID
//! most of them name is read. Every other group has no part-level ID: its one value set, else the set of ID 0, is read,
//! and a group with several sets and no ID 0 is not selected. FogVolume sets are all retained as unresolved
//! local-volume source data; they are never selected through the part's FogParamEditor slot. A field stored at several
//! times of day is kept as every one of its samples, in stored order; a program that wants one hour runs
//! `sample-fromsoft-draw-params`.
use super::super::source::{DrawField, DrawGroup, DrawParam, SetChosen};
use fromsoft_formats::area::{DrawParamId, draw_param_path, draw_params};
use fromsoft_formats::emevd::Script;
use fromsoft_formats::gparam_events::{self, Switch};
use fromsoft_formats::{
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
    /// The file with this stem (`m11_01_0720`), one of [`fromsoft_formats::area::draw_params`].
    File(String),
    /// The file the scene config of the named collision part (`h003000`) names: its first scene byte as the slot and, in
    /// Elden Ring, its byte at +32 as the sub id ([`fromsoft_formats::msb::PartGparam::draw_param_id`]). That the game reads
    /// this file while the player stands on that collision is the community's reading and what the census of the stock
    /// files supports, not an observation of the running game.
    Collision(String),
}

/// What a [`DrawParams::Collision`] choice found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionChoice {
    /// The collision part asked for.
    pub collision: String,
    /// The four-digit file id its scene config names.
    pub id: u16,
    /// The stem of the file of that id the archive holds for the area's map, if it holds one.
    pub file: Option<String>,
}

/// What the read of the draw-param file found and left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Whether a file was asked for.
    pub asked: bool,
    /// The stems the archive holds for the area's map: what a caller may ask for.
    pub available: Vec<String>,
    /// The file read, its GPARAM version, the groups kept and the field values in them: one for each stored time sample,
    /// so a field stored at two hours counts twice (see [`Report::time_variants`]).
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
    /// Further values stored for a chosen ID, at other times of day: kept beside the first, in stored order (the count
    /// of samples after each field's first), for `sample-fromsoft-draw-params` to choose among.
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
    /// With [`DrawParams::Collision`]: what the collision named.
    pub collision: Option<CollisionChoice>,
    /// The file ids the area's collision parts name through their scene configs, with how many name each: what a
    /// [`DrawParams::Collision`] can choose among.
    pub collision_ids: BTreeMap<u16, usize>,
    /// The calls of the draw-param switching instructions in the area's own event script, in script order: not applied.
    pub switches: Vec<gparam_events::Call>,
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
                // every stored sample is kept, in stored order: choosing an hour is `sample-fromsoft-draw-params`'s
                report.time_variants += at.len().saturating_sub(1);
                for index in at {
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

/// The stem `choice` names among the `available` ones, with what it found out noted in `report` (`collision`, and
/// `missing` when no file is named or none is held).
fn stem_of(
    choice: &DrawParams,
    available: &[String],
    collisions: &BTreeMap<String, DrawParamId>,
    report: &mut Report,
) -> Option<String> {
    match choice {
        DrawParams::None => None,
        DrawParams::File(stem) => {
            if available.contains(stem) {
                Some(stem.clone())
            } else {
                report.missing = Some(stem.clone());
                None
            }
        }
        DrawParams::Collision(name) => {
            let Some(id) = collisions.get(name) else {
                report.missing = Some(format!(
                    "collision {name} (no such collision part with a scene config)"
                ));
                return None;
            };
            let file = available
                .iter()
                .find(|stem| DrawParamId::from_stem(stem).is_some_and(|(_, found)| found == *id))
                .cloned();
            report.collision = Some(CollisionChoice {
                collision: name.clone(),
                id: id.number(),
                file: file.clone(),
            });
            if file.is_none() {
                report.missing = Some(format!("{:04} (the file id collision {name} names)", id.number()));
            }
            file
        }
    }
}

/// Read what `choice` asks for. `configs` are the graphics configs of the parts the read keeps and of the area's
/// collision parts; `collisions` are the file ids the area's collision parts name, by part name.
pub(super) fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    area: &str,
    choice: &DrawParams,
    configs: &[PartGparam],
    collisions: &BTreeMap<String, DrawParamId>,
) -> Result<(Option<DrawParam>, Report)> {
    if *choice == DrawParams::None {
        return Ok((None, Report::default()));
    }
    let mut report = Report {
        asked: true,
        available: draw_params(archive, game, area),
        layers_not_read: game == Game::EldenRing,
        ..Report::default()
    };
    for id in collisions.values() {
        *report.collision_ids.entry(id.number()).or_default() += 1;
    }
    // the area's own events that switch a file (not every map has a script of its own)
    match archive.read(&format!("/event/{area}.emevd.dcx"), decoder) {
        Ok(bytes) => report.switches = gparam_events::calls(game, &Script::read(&bytes)?),
        Err(fromsoft_formats::Error::Missing(_)) => {}
        Err(e) => return Err(e),
    }
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
    let available = report.available.clone();
    let Some(stem) = stem_of(choice, &available, collisions, &mut report) else {
        return Ok((None, report));
    };
    let bytes = archive.read(&draw_param_path(&stem), decoder)?;
    report.bytes = bytes.len();
    let gparam = match Gparam::read(&bytes) {
        Ok(gparam) => gparam,
        Err(e) => {
            report.unreadable = Some(e.to_string());
            return Ok((None, report));
        }
    };
    let source = select(&gparam, &stem, configs, &mut report);
    Ok((Some(source), report))
}

/// A switch call in a few words, for a notice.
fn describe(call: &gparam_events::Call) -> String {
    fn show<T: std::fmt::Display>(v: Option<T>) -> String {
        v.map_or_else(|| "?".to_owned(), |v| v.to_string())
    }
    let what = match call.switch {
        Switch::AreaSubId {
            area,
            block,
            sub,
            seconds,
        } => format!(
            "sub id {} of area {} block {} over {} s",
            show(sub),
            show(area),
            show(block),
            show(seconds)
        ),
        Switch::Override { id, seconds } => format!("override {} over {} s", show(id), show(seconds)),
        Switch::ClearOverride { seconds } => format!("override cleared over {} s", show(seconds)),
    };
    format!("event {}: {what}", call.event)
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
        let others: Vec<String> = self.available.iter().filter(|s| *s != file).cloned().collect();
        if !others.is_empty() {
            out.push(listed(
                NoticeKind::Selection,
                "draw-param files of the area's map other than the one read (a collision's scene config and the area's events name them; how the game combines those and the weather is not established)",
                others,
            ));
        }
        if !self.scenes.is_empty() {
            out.push(self.scenes.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "collision parts storing a scene config (four bytes, the first the slot of a draw-param file), not used to choose unless a collision was asked for",
                    self.scenes.values().sum::<usize>() as u64,
                ),
                |n, (bytes, parts)| n.example(format!("{parts} x {bytes}")),
            ));
        }
        if !self.switches.is_empty() {
            out.push(listed(
                NoticeKind::Selection,
                "draw-param switches the area's own events make (Sekiro's Set Area Gparam Sub ID, Elden Ring's Activate and Deactivate Gparam Override), not applied",
                self.switches.iter().map(describe).collect(),
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
        let unread_fields = self
            .unchosen_fields
            .saturating_sub(fog_sets.map_or(0, |(_, fields)| *fields));
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
    use fromsoft_formats::gparam::{Field, Group, Value, Values};

    #[test]
    fn every_stored_time_sample_of_a_chosen_set_is_kept_in_stored_order() {
        let gparam = Gparam {
            version: 5,
            unread: Unread::default(),
            groups: vec![group(
                LIGHT_SET,
                vec![Field {
                    times: vec![6.0, 12.0, 18.0],
                    ..field("Brightness", &[7, 9, 7], Values::F32(vec![2.0, 99.0, 8.0]))
                }],
            )],
        };
        let mut report = Report::default();
        let source = select(&gparam, "fixture", &[config(7, -1, -1)], &mut report);
        let light = source.group(LIGHT_SET).unwrap();
        assert_eq!(
            light
                .fields
                .iter()
                .map(|f| (&f.value, f.time))
                .collect::<Vec<_>>(),
            [(&Value::Float(2.0), Some(6.0)), (&Value::Float(8.0), Some(18.0))]
        );
        assert_eq!(report.time_variants, 1);
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
            gparam_override: None,
            scene_sub_id: None,
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
                        field(
                            "Colour",
                            &[3, 7],
                            Values::Vec4(vec![[0.; 4], [1., 0.5, 0.25, 1.]]),
                        ),
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
                group(
                    "Bloom",
                    vec![field("Threshold", &[2, 5], Values::F32(vec![1.0, 2.0]))],
                ),
            ],
            unread: Unread {
                extras: 2,
                data_bytes: 8,
            },
        }
    }

    #[test]
    fn the_value_set_most_parts_name_is_read_and_every_other_choice_is_counted() {
        let configs = [
            config(7, 0, 9),
            config(7, 0, 9),
            config(3, 0, 9),
            config(-1, -1, -1),
        ];
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
        assert_eq!(
            (fog.id, fog.chosen),
            (0, SetChosen::ByParts),
            "parts name ID 0 of the fog"
        );
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
        // Sharpness stores two samples for set 7 (kept, both), Colour one, Dist one, Exposure one
        assert_eq!((report.groups, report.fields), (3, 5));
        assert_eq!(
            report.unread,
            Unread {
                extras: 2,
                data_bytes: 8
            }
        );
        assert_eq!(report.file.as_deref(), Some("m11_01_0000"));
    }

    #[test]
    fn a_group_with_one_set_is_read_whatever_the_parts_name_and_a_tie_takes_the_lowest() {
        let only = Gparam {
            version: 5,
            groups: vec![group(
                "Fog volume",
                vec![field("A", &[4], Values::F32(vec![1.0]))],
            )],
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
                vec![Field {
                    times: vec![0.0, 12.0, 0.0],
                    ..field("Density", &[100, 100, 101], Values::F32(vec![1.0, 2.0, 3.0]))
                }],
            )],
            unread: Unread::default(),
        };
        for fog in [-1, 90, 101] {
            let mut report = Report {
                asked: true,
                ..Default::default()
            };
            let source = select(&fog_volume, "m10_00_0000", &[config(-1, fog, -1)], &mut report);
            assert!(source.group(FOG_VOLUME).is_none());
            assert_eq!(
                source.groups.iter().map(|g| (g.id, g.chosen)).collect::<Vec<_>>(),
                [(100, SetChosen::Unresolved), (101, SetChosen::Unresolved)]
            );
            assert_eq!(
                source.groups[0]
                    .fields
                    .iter()
                    .map(|f| (&f.value, f.time))
                    .collect::<Vec<_>>(),
                [(&Value::Float(1.0), Some(0.0)), (&Value::Float(2.0), Some(12.0))]
            );
            assert_eq!(source.groups[1].value("Density"), Some(&Value::Float(3.0)));
            assert!(report.other_ids.is_empty() && report.absent_ids.is_empty());
            assert_eq!(report.unchosen_sets[FOG_VOLUME], (vec![100, 101], 1));
            assert!(
                report
                    .notices()
                    .iter()
                    .any(|n| n.reason.contains("selection Unresolved"))
            );
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
            groups: vec![
                group(LIGHT_SET, vec![]),
                group("Tone Map[Yebis]", vec![]),
                group(FOG, vec![]),
            ],
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
        assert!(
            notices
                .iter()
                .all(|n| !n.reason.starts_with("draw-param groups with no value set"))
        );
        let n = notices
            .iter()
            .find(|n| n.reason.contains("stores no value at all"))
            .unwrap();
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
        assert_eq!(
            (n.count, n.examples.clone()),
            (4, vec!["4 x [2, -1, -1, -1]".to_owned()])
        );
        let n = &notices[find(&notices, "parts naming another value set").unwrap()];
        assert_eq!(n.count, 1);
        let n = &notices[find(
            &notices,
            "value sets parts name that the draw-param file does not store",
        )
        .unwrap()];
        assert_eq!((n.kind, n.count), (NoticeKind::Unsupported, 2));
        let n = &notices[find(&notices, "draw-param groups no part names").unwrap()];
        assert_eq!(
            (n.kind, n.examples.clone()),
            (NoticeKind::Approximation, vec!["Tone Map[Yebis]".to_owned()])
        );
        // the groups left unread count their fields
        let n = &notices[find(&notices, "draw-param groups with no value set").unwrap()];
        assert_eq!(n.count, 2);
        assert_eq!(n.examples, [ENV_MAP, "Bloom"]);
        assert!(find(&notices, "draw-param extras").is_some());
        assert!(find(&notices, "draw-param opaque block").is_some());
        assert!(find(&notices, "draw-param layers of the game's weather").is_some());
    }

    fn ids(pairs: &[(&str, u8, u8)]) -> BTreeMap<String, DrawParamId> {
        pairs
            .iter()
            .map(|&(name, slot, sub)| (name.to_owned(), DrawParamId::new(slot, sub).unwrap()))
            .collect()
    }

    /// A collision names a file by its slot and sub id; a stem is named only if the map holds it.
    #[test]
    fn a_choice_names_a_stem_among_the_ones_the_map_holds() {
        let available: Vec<String> = ["m10_00_0000", "m10_00_0100", "m10_00_0500"]
            .map(String::from)
            .into();
        let collisions = ids(&[("h000800", 0, 0), ("h014000", 5, 0), ("h014500", 8, 10)]);
        let mut report = Report::default();
        assert_eq!(
            stem_of(
                &DrawParams::File("m10_00_0100".into()),
                &available,
                &collisions,
                &mut report
            ),
            Some("m10_00_0100".to_owned())
        );
        assert!(
            stem_of(
                &DrawParams::File("m10_00_0999".into()),
                &available,
                &collisions,
                &mut report
            )
            .is_none()
        );
        assert_eq!(report.missing.as_deref(), Some("m10_00_0999"));
        let mut report = Report::default();
        assert_eq!(
            stem_of(
                &DrawParams::Collision("h014000".into()),
                &available,
                &collisions,
                &mut report
            ),
            Some("m10_00_0500".to_owned())
        );
        let choice = report.collision.unwrap();
        assert_eq!((choice.id, choice.file.as_deref()), (500, Some("m10_00_0500")));
        // a collision whose file the map lacks is named in `missing`, with the id
        let mut report = Report::default();
        assert!(
            stem_of(
                &DrawParams::Collision("h014500".into()),
                &available,
                &collisions,
                &mut report
            )
            .is_none()
        );
        assert_eq!(report.collision.unwrap().file, None);
        assert!(report.missing.unwrap().starts_with("0810 "));
        // and one the map does not have at all
        let mut report = Report::default();
        assert!(
            stem_of(
                &DrawParams::Collision("h999999".into()),
                &available,
                &collisions,
                &mut report
            )
            .is_none()
        );
        assert!(report.collision.is_none());
        assert!(report.missing.unwrap().starts_with("collision h999999"));
    }

    /// The area's own switch calls are listed in a notice, with a parameter-filled field shown as a question mark.
    #[test]
    fn the_events_switches_are_told_and_not_applied() {
        use fromsoft_formats::gparam_events::{Call, Switch};
        let report = Report {
            asked: true,
            file: Some("m11_01_0000".into()),
            available: vec!["m11_01_0000".into()],
            switches: vec![
                Call {
                    event: 11_105_000,
                    instruction: 3,
                    switch: Switch::AreaSubId {
                        area: Some(11),
                        block: Some(1),
                        sub: Some(20),
                        seconds: Some(5.0),
                    },
                },
                Call {
                    event: 11_105_001,
                    instruction: 0,
                    switch: Switch::Override {
                        id: None,
                        seconds: Some(1.5),
                    },
                },
            ],
            ..Report::default()
        };
        let notices = report.notices();
        let n = notices
            .iter()
            .find(|n| n.reason.starts_with("draw-param switches the area's own events"))
            .unwrap();
        assert_eq!((n.kind, n.count), (NoticeKind::Selection, 2));
        assert_eq!(
            n.examples,
            [
                "event 11105000: sub id 20 of area 11 block 1 over 5 s",
                "event 11105001: override ? over 1.5 s"
            ]
        );
    }
}
