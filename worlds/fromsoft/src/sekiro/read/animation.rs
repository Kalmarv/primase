//! A character's animation binder (`chr/<id>.anibnd`) read into the world beside its model: the Havok skeleton
//! (`skeleton.hkx`) as a `core.skeleton` of its own and each animation as a `havok.animation` (the spline compressed
//! tracks as stored, the binding that says which bone each drives), one entity each, named by the file (`a000_003000`)
//! and stating where it was read from (`core.provenance`: the binder, as the archive, and the member, as the file).
//! A part binder holds such a binder for a form of its own ([`read_member`]: Sekiro's `parts/wp_a_0761_l.partsbnd`
//! holds `wp_a_0761.anibnd` and `wp_a_0761_1.anibnd`, the closed and the open umbrella's), which is read the same way.
//!
//! A binder can name more binders its animations are in: Sekiro's player (`c0000.anibnd`) holds a skeleton, event files
//! and a load-order marker (`c0000_a000_hi.txt`) for each of the binders that hold the animations (`c0000_a000_hi.anibnd`,
//! ...); Elden Ring's characters keep theirs in divisions (`c2010.anibnd` holds the skeleton and `c2010_div00.blf`,
//! `c2010_div01.blf`, the load lists of `c2010_div00.anibnd` and `c2010_div01.anibnd`; the player's markers are `.txt`
//! as Sekiro's). Those are listed by the data: a marker member (`.txt` or `.blf`) of the binder named `<id>_<set>`
//! whose binder the archive holds is read too, in the order the binder lists them, its animations binding the skeleton
//! by name as any other. Markers of other characters (`delayload_c0000_c1020.txt`) name no binder of this one and stay
//! unread.
//!
//! A binder's event files (`c1010.tae`, an object's `o252050.tae`, the player's `a50.tae` of group 50) are read too
//! ([`fromsoft_formats::tae`], each numbered by its binder slot): the record of each animation read is put on its entity
//! as `fromsoft.clip_events` (the events with their times and parameter bytes as stored), found by the animation's
//! number (`a050_003000` is the record numbered 50,003,000; [`fromsoft_formats::tae::clip_animation_id`]), its imports
//! followed among the event files of its own binder.
//! The report counts the records no animation matches and the animations no record matches, so a numbering that does not
//! hold shows as counts, not as silence.
//!
//! Nothing is lowered here: a program chooses what the clips are (`lower-havok-clips`: what stands in for a component a
//! track does not state, the keys for each frame, which animations, the root motion) and how they reach the model's
//! skeleton (`correspond-skeletons`, `map-clip`). The Havok skeleton is in the FLVER's frame (the reference poses of the
//! bones the two share are equal as stored: measured on `c1010` and `c0000`), so both go to the world's by the same
//! frame.
//!
//! What the reader does not read is named: animations that are not spline compressed (one interleaved one in the
//! install), files that are not tagfiles of a supported SDK (one 2015.1 file), animations binding a skeleton the binder
//! does not hold, an animation named as one already read, and an event file of a version other than Sekiro's and Elden
//! Ring's.

use std::collections::BTreeMap;
use std::sync::Arc;

use havok_formats::animation::{Container, Skeleton, container};
use havok_formats::{Compendium, Tagfile};
use primase_core::{Entity, Name, Origin, Provenance, World};
use primase_havok::world::{HavokAnimation, insert_animation, insert_skeleton};

use crate::sekiro::character_data::ClipEvents;
use fromsoft_formats::names::stem;
use fromsoft_formats::tae::{self, Tae};
use fromsoft_formats::{Archive, Binder, Result, dcx::Decoder};

pub use crate::components::clip_source::ClipSource;

/// Reader-private binder index. Viewers use the published asset members, never
/// a parallel World resource or a second public listing/decode interface.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ClipListing {
    character: String,
    clips: Vec<ClipSource>,
    companions: Vec<String>,
}

fn metadata_binder(binder: &Binder) -> Binder {
    Binder {
        version: binder.version,
        format: binder.format,
        files: binder
            .files
            .iter()
            .filter(|file| {
                file.name.as_deref().is_none_or(|name| {
                    let leaf = file_name(name);
                    !leaf.ends_with(".hkx") || stem(name) == "skeleton"
                })
            })
            .cloned()
            .collect(),
    }
}

fn list_members(out: &mut ClipListing, set: &str, binder: &Binder) {
    for (member_index, file) in binder.files.iter().enumerate() {
        let Some(member) = &file.name else {
            continue;
        };
        let name = stem(member);
        if file_name(member).ends_with(".hkx") && name != "skeleton" {
            let binder = binder_path(set);
            let key=serde_json::json!({"character":out.character,"binder":binder,"member":member,"index":member_index}).to_string();
            out.clips.push(ClipSource {
                character: out.character.clone(),
                key,
                name,
                binder,
                member: member.clone(),
                member_index,
                duration: None,
            });
        }
    }
}

fn publish_listing(listing: &ClipListing, world: &mut World) -> Result<Entity> {
    // Validate all known lengths before spawning any entity.
    for clip in &listing.clips {
        if clip
            .duration
            .is_some_and(|d| !d.is_finite() || d < 0. || !(d as f32).is_finite())
        {
            return Err(crate::invalid(
                "FromSoftware clip listing",
                0,
                format!("{}: unrepresentable length", clip.key),
            ));
        }
    }
    let mut members = Vec::new();
    for clip in &listing.clips {
        let entity = world.spawn();
        world.insert(entity, primase_core::ClipKey(clip.key.clone()));
        world.insert(entity, Name(clip.name.clone()));
        world.insert(entity, clip.clone());
        world.insert(
            entity,
            Provenance::new(Origin::new(Some(&clip.binder), &clip.member, None)),
        );
        if let Some(duration) = clip.duration {
            world.insert(entity, primase_core::ClipLength(duration as f32));
        }
        members.push((entity, primase_core::Role::Clip));
    }
    let root = world.spawn();
    world.insert(root, Name(format!("{} animations", listing.character)));
    // Include binders with no clips: a census must still see their event records.
    let mut provenance = Provenance::new(Origin::new(None, &binder_path(&listing.character), None));
    for set in &listing.companions {
        provenance.merge(&Provenance::new(Origin::new(None, &binder_path(set), None)));
    }
    world.insert(root, provenance);
    world.insert(root, primase_core::Asset { members });
    Ok(root)
}

/// Reader-private archive/binder state. Neither bytes nor executable decoder text enter
/// the World. A listed binder is opened once for this reader session; load parses only
/// the chosen clip and the skeleton, compendium and event members it needs.
#[derive(Debug, Default)]
pub(super) struct ClipReader {
    archive: Option<Arc<Archive>>,
    binders: BTreeMap<String, Binder>,
    listings: BTreeMap<String, ClipListing>,
    loaded_binders: std::collections::BTreeSet<String>,
    event_records: BTreeMap<(String, String), Vec<i64>>,
    metadata: BTreeMap<String, std::result::Result<CachedMetadata, String>>,
}
#[derive(Debug)]
struct CachedMetadata {
    compendium: Option<Compendium>,
    skeletons: Vec<Skeleton>,
    events: Vec<(String, Arc<Tae>)>,
    report: AnimationReport,
}
impl ClipReader {
    pub(super) fn with_archive(archive: Arc<Archive>) -> Self {
        Self {
            archive: Some(archive),
            ..Default::default()
        }
    }
    #[cfg(test)]
    pub(super) fn test_binder(id: &str, binder: Binder) -> Self {
        let mut listing = ClipListing {
            character: id.into(),
            ..Default::default()
        };
        list_members(&mut listing, id, &binder);
        Self {
            archive: Some(Arc::new(Archive::empty(fromsoft_formats::Game::Sekiro))),
            binders: BTreeMap::from([(binder_path(id), binder)]),
            listings: BTreeMap::from([(id.into(), listing)]),
            ..Default::default()
        }
    }
    fn binder(
        &mut self,
        install: &fromsoft_formats::Install,
        decoder: &Decoder<'_>,
        path: &str,
    ) -> Result<()> {
        if self.binders.contains_key(path) {
            return Ok(());
        }
        if self.archive.is_none() {
            self.archive = Some(Arc::new(install.open()?));
        }
        let archive = self.archive.as_ref().expect("archive opened above");
        let binder = Binder::read(&archive.read(path, decoder)?, decoder)?;
        self.binders.insert(path.into(), binder);
        Ok(())
    }
    pub(super) fn listing(
        &mut self,
        install: &fromsoft_formats::Install,
        decoder: &Decoder<'_>,
        id: &str,
    ) -> Result<Option<ClipListing>> {
        if let Some(listing) = self.listings.get(id) {
            return Ok(Some(listing.clone()));
        }
        if self.archive.is_none() {
            self.archive = Some(Arc::new(install.open()?));
        }
        let path = binder_path(id);
        if !self
            .archive
            .as_ref()
            .expect("archive opened above")
            .contains(&path)
        {
            return Ok(None);
        }
        self.binder(install, decoder, &path)?;
        let main = &self.binders[&path];
        let named = companions(self.archive.as_ref().expect("archive opened above"), id, main);
        let mut listing = ClipListing {
            character: id.into(),
            clips: vec![],
            companions: named.clone(),
        };
        list_members(&mut listing, id, main);
        for set in named {
            let path = binder_path(&set);
            self.binder(install, decoder, &path)?;
            list_members(&mut listing, &set, &self.binders[&path]);
        }
        self.listings.insert(id.into(), listing.clone());
        Ok(Some(listing))
    }
    pub(super) fn plan(
        &mut self,
        install: &fromsoft_formats::Install,
        decoder: &Decoder<'_>,
        id: &str,
        chosen: Option<&[String]>,
        world: &mut World,
    ) -> Result<Option<Entity>> {
        let Some(mut listing) = self.listing(install, decoder, id)? else {
            return Ok(None);
        };
        if let Some(chosen) = chosen {
            for name in chosen {
                select_source(&listing, name)?;
            }
            listing
                .clips
                .retain(|clip| chosen.iter().any(|name| clip.name == *name || clip.key == *name));
        }
        publish_listing(&listing, world).map(Some)
    }

    /// Parsed skeleton/TAE/compendium metadata is private to this reader. Roots
    /// share the parsed event files; only their chosen HKX member is parsed anew.
    fn held(
        &mut self,
        path: &str,
        binder_name: &str,
        member: Option<usize>,
        markers: &[String],
        decoder: &Decoder<'_>,
        report: &mut AnimationReport,
    ) -> Result<Held> {
        if !self.metadata.contains_key(path) {
            let parsed = (|| -> Result<CachedMetadata> {
                let binder = &self.binders[path];
                let compendium = read_compendium(binder, decoder)?;
                let mut metadata_report = AnimationReport::default();
                let held = hold_with_compendium(
                    &metadata_binder(binder),
                    binder_name,
                    decoder,
                    markers,
                    &mut metadata_report,
                    compendium.as_ref(),
                )?;
                for unread in &mut metadata_report.unread {
                    *unread = format!("{path}!{unread}");
                }
                if !held.animations.is_empty() {
                    refuse(
                        &mut metadata_report,
                        "skeleton.hkx holds animation data rather than a skeleton",
                        "skeleton",
                    );
                }
                Ok(CachedMetadata {
                    compendium,
                    skeletons: held.skeletons,
                    events: held.events,
                    report: metadata_report,
                })
            })();
            self.metadata
                .insert(path.into(), parsed.map_err(|error| error.to_string()));
        }
        let cached = self.metadata[path].as_ref().map_err(|error| {
            crate::invalid("FromSoftware animation metadata", 0, format!("{path}: {error}"))
        })?;
        report.tae_files += cached.report.tae_files;
        report.unread.extend(cached.report.unread.iter().cloned());
        for (reason, files) in &cached.report.refused {
            report
                .refused
                .entry(reason.clone())
                .or_default()
                .extend(files.iter().cloned());
        }
        let mut held = Held {
            skeletons: cached.skeletons.clone(),
            events: cached.events.clone(),
            animations: vec![],
        };
        if let Some(index) = member {
            let file = self.binders[path].files.get(index).ok_or_else(|| {
                crate::invalid(
                    "FromSoftware clip member",
                    0,
                    format!("{path}: member {index} is absent"),
                )
            })?;
            let selected = Binder {
                version: self.binders[path].version,
                format: self.binders[path].format,
                files: vec![file.clone()],
            };
            let mut clip = hold_with_compendium(
                &selected,
                binder_name,
                decoder,
                &[],
                report,
                cached.compendium.as_ref(),
            )?;
            held.skeletons.append(&mut clip.skeletons);
            held.animations.append(&mut clip.animations);
        }
        Ok(held)
    }
    pub(super) fn load(
        &mut self,
        install: &fromsoft_formats::Install,
        decoder: &Decoder<'_>,
        source: &ClipSource,
        target: Entity,
        world: &mut World,
    ) -> Result<AnimationReport> {
        let listing = self
            .listing(install, decoder, &source.character)?
            .ok_or_else(|| {
                crate::invalid("FromSoftware animation", 0, "listed animation binder is absent")
            })?;
        if !listing.clips.contains(source) {
            return Err(crate::invalid(
                "FromSoftware animation",
                0,
                format!("{}: descriptor differs from the authored listing", source.key),
            ));
        }
        let main_path = binder_path(&source.character);
        let mut report = AnimationReport::default();
        self.loaded_binders.insert(main_path.clone());
        let mut held = vec![self.held(
            &main_path,
            &format!("{}.anibnd", source.character),
            (source.binder == main_path).then_some(source.member_index),
            &listing.companions,
            decoder,
            &mut report,
        )?];
        if source.binder != main_path {
            self.loaded_binders.insert(source.binder.clone());
            held.push(self.held(
                &source.binder,
                &source.binder,
                Some(source.member_index),
                &[],
                decoder,
                &mut report,
            )?);
            report.companions.push(source.binder.clone());
        }
        for (index, held) in held.iter().enumerate() {
            let binder = if index == 0 { &main_path } else { &source.binder };
            for (file, tae) in &held.events {
                self.event_records
                    .entry((binder.clone(), file.clone()))
                    .or_insert_with(|| tae.animations.iter().map(|a| tae.number(a)).collect());
            }
        }
        let result = insert(held, &source.character, report, install.game, world)?;
        let [decoded] = result.animations.as_slice() else {
            return Err(crate::invalid(
                "FromSoftware animation",
                0,
                format!(
                    "{}: expected one readable clip, got {}; {:?}",
                    source.key,
                    result.animations.len(),
                    result.report.refused
                ),
            ));
        };
        // Clip dependencies refer to the skeleton, never to their own animation entity.
        // Moving the raw components preserves the plan's key and source descriptor.
        if !world
            .move_components(*decoded, target)
            .map_err(|why| crate::invalid("FromSoftware animation", 0, why))?
        {
            return Err(crate::invalid(
                "FromSoftware animation",
                0,
                "clip plan entity disappeared during load",
            ));
        }
        // Keep the same fully qualified binder/member provenance used by the listing,
        // including companions; grouping never needs to parse an opaque key.
        world.insert(
            target,
            Provenance::new(Origin::new(Some(&source.binder), &source.member, None)),
        );
        Ok(result.report)
    }

    /// Unique source-file/record counts across strict per-root loads, rather than
    /// summing the same main TAE once for every animation that uses it.
    pub(super) fn summarize(&self, world: &World, entities: &[Entity], report: &mut AnimationReport) {
        let mut skeletons = std::collections::BTreeSet::new();
        let mut matched = std::collections::BTreeSet::new();
        report.animations = entities.len();
        report.events = 0;
        report.clips_without_events = 0;
        for &entity in entities {
            if let Some(animation) = world.get::<HavokAnimation>(entity) {
                skeletons.insert(animation.skeleton);
            }
            match world.get::<ClipEvents>(entity) {
                None => report.clips_without_events += 1,
                Some(events) => {
                    report.events += events.events.len();
                    if let Some(source) = world.get::<ClipSource>(entity) {
                        for binder in [binder_path(&source.character), source.binder.clone()] {
                            if self
                                .event_records
                                .get(&(binder.clone(), events.file.clone()))
                                .is_some_and(|ids| ids.contains(&events.animation))
                            {
                                matched.insert((binder, events.file.clone(), events.animation));
                                break;
                            }
                        }
                    }
                }
            }
        }
        report.bones = skeletons
            .into_iter()
            .filter_map(|entity| world.get::<primase_core::Skeleton>(entity))
            .map(|s| s.joints.len())
            .sum();
        report.tae_files = self.event_records.len();
        report.tae_animations = self.event_records.values().map(|ids| ids.len()).sum();
        report.tae_unmatched = report.tae_animations - matched.len();
        report.companions = self
            .listings
            .values()
            .flat_map(|listing| listing.companions.iter())
            .filter(|set| self.loaded_binders.contains(&binder_path(set)))
            .cloned()
            .collect();
        // Include metadata notices even if every clip in that binder was refused.
        for cached in self.metadata.values().filter_map(|cached| cached.as_ref().ok()) {
            for unread in &cached.report.unread {
                if !report.unread.contains(unread) {
                    report.unread.push(unread.clone());
                }
            }
        }
        let mut unique = std::collections::BTreeSet::new();
        report.unread.retain(|name| unique.insert(name.clone()));
    }
}

/// Light skeleton read for probes that choose no clip. Reads the main binder's
/// skeleton and compendium only. No model, texture, TAE or
/// animation tracks are parsed. A present binder without a readable skeleton is a named error, not an empty source.
pub fn read_clip_skeleton(
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
    w: &mut World,
) -> Result<Option<Animations>> {
    let path = binder_path(id);
    if !archive.contains(&path) {
        return Ok(None);
    }
    let mut binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    binder.files.retain(|f| {
        f.name.as_deref().is_some_and(|n| {
            file_name(n).ends_with(".compendium") || (file_name(n).ends_with(".hkx") && stem(n) == "skeleton")
        })
    });
    let mut report = AnimationReport::default();
    let held = hold(&binder, &format!("{id}.anibnd"), decoder, &[], &mut report)?;
    if held.skeletons.is_empty() {
        return Err(crate::invalid(
            "FromSoftware animation skeleton",
            0,
            format!("{id} main binder has no readable skeleton: {:?}", report.refused),
        ));
    }
    insert(vec![held], id, report, archive.game(), w).map(Some)
}

fn select_source<'a>(listing: &'a ClipListing, key_or_name: &str) -> Result<&'a ClipSource> {
    if let Some(source) = listing.clips.iter().find(|clip| clip.key == key_or_name) {
        return Ok(source);
    }
    let matches: Vec<_> = listing
        .clips
        .iter()
        .filter(|clip| clip.name == key_or_name)
        .collect();
    match matches.as_slice() {
        [source] => Ok(*source),
        [] => Err(crate::invalid(
            "FromSoftware animation",
            0,
            format!("unknown clip {key_or_name}"),
        )),
        _ => Err(crate::invalid(
            "FromSoftware animation",
            0,
            format!(
                "ambiguous clip name {key_or_name}; use one of the binder/member keys: {:?}",
                matches.iter().map(|source| &source.key).collect::<Vec<_>>()
            ),
        )),
    }
}

/// What reading a character's animation binders found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationReport {
    /// Bones in source skeletons retained by this read (0 when none is retained).
    pub bones: usize,
    /// `havok.animation` entities made.
    pub animations: usize,
    /// The binders read besides the character's own (`c0000_a000_hi`, ...), by name, in the order the marker members
    /// list them.
    pub companions: Vec<String>,
    /// Vector components of a track whose stored value or quantisation range is NaN or infinite (`scale x of R_Egg in
    /// a001_009000`): they cannot be decoded and are read as absent, the program's `absent` choice standing for them.
    pub absent_components: Vec<String>,
    /// Files not read, by reason, each with the stems of its first few.
    pub refused: BTreeMap<String, Vec<String>>,
    /// Members of the binders this reader does not read (markers of other characters, ...), by name.
    pub unread: Vec<String>,
    /// Event files (`c1010.tae`) read.
    pub tae_files: usize,
    /// Animation records those files hold.
    pub tae_animations: usize,
    /// Of those, the ones no animation read is numbered as (see [`fromsoft_formats::tae::clip_animation_id`]): events of an
    /// animation the binders do not hold, or a numbering the rule does not match.
    pub tae_unmatched: usize,
    /// Animations read that no record of the event files is numbered as.
    pub clips_without_events: usize,
    /// Events put on animations (`fromsoft.clip_events`).
    pub events: usize,
}

/// The entities a read made: the Havok skeleton and each animation.
#[derive(Debug, Default)]
pub struct Animations {
    pub skeleton: Option<Entity>,
    pub animations: Vec<Entity>,
    pub report: AnimationReport,
}

/// The file's name after its last separator, lowercased.
fn file_name(name: &str) -> String {
    fromsoft_formats::names::fold(name.rsplit(['/', '\\']).next().unwrap_or(name))
}

fn refuse(report: &mut AnimationReport, why: &str, file: &str) {
    let list = report.refused.entry(why.to_owned()).or_default();
    list.push(file.to_owned());
}

fn binder_path(stem: &str) -> String {
    format!("/chr/{stem}.anibnd.dcx")
}

/// The binders a binder's marker members name as this character's (`<id>_<set>.txt` or `.blf` with an archive binder of
/// that name), in the binder's order, each once: [`Binder::division_markers`] filtered by what the archive holds.
fn companions(archive: &Archive, id: &str, binder: &Binder) -> Vec<String> {
    binder
        .division_markers(&format!("{id}_"))
        .into_iter()
        .filter(|set| archive.contains(&binder_path(set)))
        .collect()
}

/// What one binder holds: its skeletons, its animation containers by file stem, each with where it was read from, and
/// its event files (`.tae`) by file name.
struct Held {
    skeletons: Vec<Skeleton>,
    animations: Vec<(String, Origin, Container)>,
    events: Vec<(String, Arc<Tae>)>,
}

fn read_compendium(binder: &Binder, decoder: &Decoder<'_>) -> Result<Option<Compendium>> {
    match binder.files.iter().find(|file| {
        file.name
            .as_deref()
            .is_some_and(|name| fromsoft_formats::names::fold(name).ends_with(".compendium"))
    }) {
        Some(file) => Ok(Some(Compendium::read(&decoder.decode_if_dcx(&file.bytes)?)?)),
        None => Ok(None),
    }
}

/// Read the tagfiles of `binder` (one that names a marker for a binder in `markers` is not unread).
fn hold(
    binder: &Binder,
    binder_name: &str,
    decoder: &Decoder<'_>,
    markers: &[String],
    report: &mut AnimationReport,
) -> Result<Held> {
    let compendium = read_compendium(binder, decoder)?;
    hold_with_compendium(binder, binder_name, decoder, markers, report, compendium.as_ref())
}

fn hold_with_compendium(
    binder: &Binder,
    binder_name: &str,
    decoder: &Decoder<'_>,
    markers: &[String],
    report: &mut AnimationReport,
    compendium: Option<&Compendium>,
) -> Result<Held> {
    let mut held = Held {
        skeletons: Vec::new(),
        animations: Vec::new(),
        events: Vec::new(),
    };
    for f in &binder.files {
        let name = f.name.as_deref().unwrap_or("");
        let lower = fromsoft_formats::names::fold(name);
        if lower.ends_with(".compendium") {
            continue;
        }
        if lower.ends_with(".tae") {
            let file = file_name(name);
            match Tae::read_member(binder, f) {
                Ok(tae) => {
                    report.tae_files += 1;
                    held.events.push((file, Arc::new(tae)));
                }
                Err(e) => refuse(report, &format!("event file not read: {e}"), &file),
            }
            continue;
        }
        if !lower.ends_with(".hkx") {
            if !markers.iter().any(|m| {
                [".txt", ".blf"]
                    .iter()
                    .any(|ext| file_name(name) == format!("{m}{ext}"))
            }) {
                report.unread.push(file_name(name));
            }
            continue;
        }
        let file = stem(name);
        let bytes = decoder.decode_if_dcx(&f.bytes)?;
        let tag = match Tagfile::read(&bytes, compendium) {
            Ok(t) => t,
            Err(e) => {
                refuse(report, &format!("Havok tagfile not read: {}", e.detail), &file);
                continue;
            }
        };
        let c = match container(&tag) {
            Ok(c) => c,
            Err(e) => {
                refuse(
                    report,
                    &format!("animation container not read: {}", e.detail),
                    &file,
                );
                continue;
            }
        };
        if let Some(s) = c.skeletons.first() {
            held.skeletons.push(s.clone());
        } else {
            let origin = Origin::new(Some(binder_name), name, None);
            held.animations.push((file, origin, c));
        }
    }
    Ok(held)
}

/// Read an animation binder another binder holds (a part form's own: `wp_a_0761_1.anibnd` in
/// `parts/wp_a_0761_l.partsbnd`) into `w` with the same raw parsing as the character reader's load, its skeleton named after `name` (the form):
/// it names no companion binders (a marker member is unread).
pub(super) fn read_member(
    game: fromsoft_formats::Game,
    bytes: &[u8],
    decoder: &Decoder<'_>,
    name: &str,
    w: &mut World,
) -> Result<Animations> {
    let binder = Binder::read(bytes, decoder)?;
    let mut report = AnimationReport::default();
    let held = vec![hold(
        &binder,
        &format!("{name}.anibnd"),
        decoder,
        &[],
        &mut report,
    )?];
    insert(held, name, report, game, w)
}

/// The animation binder of a placed object's model (Sekiro's `obj/o001010`, Elden Ring's `aeg007_310`), decoded, if it
/// has one: a `.anibnd` member of the model's own binder. In Sekiro's install that is where they are (233 of its 1395
/// object binders hold one, and no object has a binder of its own beside it: `https://github.com/Kalmarv/fromsoft-formats/blob/main/tests/object_animation_census.rs`); a
/// model without the member is static. The member holds `skeleton.hkx`, the animations (`a000_000500.hkx`), their
/// compendium and the object's event file (`o252050.tae`), which [`read_member`] reads as a character's binder.
pub(super) fn object_binder(decoder: &Decoder<'_>, binder: &Binder) -> Result<Option<Vec<u8>>> {
    let member = binder.files.iter().find(|f| {
        f.name.as_deref().is_some_and(|n| {
            let file = file_name(n);
            file.ends_with(".anibnd") || file.ends_with(".anibnd.dcx")
        })
    });
    member.map(|f| decoder.decode_if_dcx(&f.bytes)).transpose()
}

/// Put what the binders `held` (in read order) hold into `w`: the skeletons, each once by name (`<id> havok
/// skeleton`), and the animations binding them, what is refused named in `report`.
fn insert(
    mut held: Vec<Held>,
    id: &str,
    report: AnimationReport,
    game: fromsoft_formats::Game,
    w: &mut World,
) -> Result<Animations> {
    let mut out = Animations {
        report,
        ..Animations::default()
    };
    let mut skeletons: BTreeMap<String, (Entity, Skeleton)> = BTreeMap::new();
    for s in held.iter().flat_map(|h| &h.skeletons) {
        if skeletons.contains_key(&s.name) {
            continue;
        }
        let made = insert_skeleton(w, s, &crate::pose::FRAME)?;
        let label = format!("{id} havok skeleton");
        let existing = w
            .iter::<primase_core::Skeleton>()
            .find(|(e, _)| *e != made && w.get::<Name>(*e).is_some_and(|n| n.0 == label))
            .map(|(e, _)| e);
        let entity = if let Some(existing) = existing {
            if w.get::<primase_core::Skeleton>(existing) != w.get::<primase_core::Skeleton>(made) {
                return Err(crate::invalid(
                    "FromSoftware animation",
                    0,
                    "Havok skeleton changed between clip reads",
                ));
            }
            w.despawn(made);
            existing
        } else {
            made
        };
        w.insert(entity, Name(label));
        out.report.bones += s.bones.len();
        out.skeleton.get_or_insert(entity);
        skeletons.insert(s.name.clone(), (entity, s.clone()));
    }
    // the event files of every binder, in read order, the binder of each, and which of their animations an animation
    // read matched
    let mut records: Vec<(String, Arc<Tae>)> = Vec::new();
    let mut binder_of: Vec<usize> = Vec::new();
    for (set, h) in held.iter_mut().enumerate() {
        binder_of.extend(std::iter::repeat_n(set, h.events.len()));
        records.append(&mut h.events);
    }
    let mut matched: std::collections::BTreeSet<(usize, i64)> = std::collections::BTreeSet::new();
    let mut read_names: BTreeMap<String, usize> = BTreeMap::new();
    for (set, (file, origin, c)) in held
        .into_iter()
        .enumerate()
        .flat_map(|(set, h)| h.animations.into_iter().map(move |a| (set, a)))
    {
        let Some(binding) = c.bindings.first() else {
            refuse(&mut out.report, "an animation with no binding", &file);
            continue;
        };
        let Some((skeleton, havok)) = skeletons.get(&binding.original_skeleton_name) else {
            refuse(
                &mut out.report,
                "an animation binding a skeleton the binders do not hold",
                &file,
            );
            continue;
        };
        if let Some(first) = read_names.get(&file) {
            refuse(
                &mut out.report,
                &format!(
                    "an animation named as one already read (from binder {first}, again in binder {set})"
                ),
                &file,
            );
            continue;
        }
        match insert_animation(w, &file, &c, *skeleton, havok, &crate::pose::FRAME) {
            Ok(Some(e)) => {
                w.insert(e, Provenance::new(origin));
                if let Some(h) = w.get::<HavokAnimation>(e) {
                    let unreadable = h.spline.unreadable.clone();
                    let bones = h.track_to_bone.clone();
                    for u in unreadable {
                        let bone = bones
                            .get(u.track)
                            .and_then(|&b| usize::try_from(b).ok())
                            .and_then(|b| havok.bones.get(b))
                            .map_or_else(|| format!("track {}", u.track), |b| b.name.clone());
                        out.report
                            .absent_components
                            .push(format!("{} of {bone} in {file}", u.part));
                    }
                }
                // the events of the animation of this number, from the first event file that has a record of it
                let record = tae::clip_animation_id(&file).and_then(|n| {
                    records
                        .iter()
                        .enumerate()
                        .find_map(|(i, (name, t))| t.animation(n).map(|a| (i, name, a)))
                });
                match record {
                    Some((i, name, a)) => {
                        let number = records[i].1.number(a);
                        // an import names an animation of the event files of the record's own binder
                        let binder: Vec<&Tae> = records
                            .iter()
                            .zip(&binder_of)
                            .filter(|(_, set)| **set == binder_of[i])
                            .map(|((_, t), _)| &**t)
                            .collect();
                        matched.insert((i, number));
                        // an import naming an animation the binder's event files do not hold leaves the clip without
                        // events, named in the report
                        match tae::resolve(&binder, number) {
                            Ok(Some((_, inherited))) => {
                                out.report.events += inherited.events.len();
                                w.insert(e, super::visibility::decode(id, &inherited.events));
                                w.insert(
                                    e,
                                    ClipEvents {
                                        file: name.clone(),
                                        animation: number,
                                        header: a.header.clone(),
                                        file_name: a.file_name.clone(),
                                        authored_events: a.events.clone(),
                                        typed: super::clip_events::decode(game, &inherited.events),
                                        events: inherited.events.clone(),
                                    },
                                );
                            }
                            Ok(None) => {
                                return Err(crate::invalid("TAE", 0, "matched event record disappeared"));
                            }
                            Err(why) => refuse(&mut out.report, &format!("events not read: {why}"), &file),
                        }
                    }
                    None if !records.is_empty() => out.report.clips_without_events += 1,
                    None => {}
                }
                read_names.insert(file, set);
                out.report.animations += 1;
                out.animations.push(e);
            }
            Ok(None) => refuse(
                &mut out.report,
                "an animation that is not spline compressed",
                &file,
            ),
            Err(e) => refuse(
                &mut out.report,
                &format!("animation not read: {}", e.detail),
                &file,
            ),
        }
    }
    out.report.tae_animations = records.iter().map(|(_, t)| t.animations.len()).sum();
    out.report.tae_unmatched = out.report.tae_animations - matched.len();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_loads_reuse_parsed_metadata_and_keep_named_unread_members() {
        let file = |name: &str| fromsoft_formats::BinderFile {
            id: None,
            name: Some(name.into()),
            flags: 0,
            bytes: vec![0xff],
        };
        let mut reader = ClipReader::test_binder(
            "c1010",
            Binder {
                version: [0; 8],
                format: 0,
                files: vec![file("walk.hkx"), file("run.hkx"), file("unknown.bin")],
            },
        );
        let path = binder_path("c1010");
        let decoder = Decoder {
            oodle: None,
            max_output: 1024,
        };
        let mut first = AnimationReport::default();
        reader
            .held(&path, "c1010.anibnd", None, &[], &decoder, &mut first)
            .unwrap();
        assert_eq!(first.unread, vec![format!("{path}!unknown.bin")]);
        // A reparse would now fail on this invalid compendium. The second root
        // must use the metadata already parsed from the reader's original binder.
        reader
            .binders
            .get_mut(&path)
            .unwrap()
            .files
            .push(file("bad.compendium"));
        let mut second = AnimationReport::default();
        reader
            .held(&path, "c1010.anibnd", None, &[], &decoder, &mut second)
            .unwrap();
        assert_eq!(reader.metadata.len(), 1);
        assert_eq!(second.unread, first.unread);
    }

    #[test]
    fn listing_does_not_decode_hkx_and_selected_decode_keeps_only_dependencies() {
        let file = |name: &str| fromsoft_formats::BinderFile {
            id: None,
            name: Some(name.into()),
            flags: 0,
            bytes: vec![0xff, 0x01],
        }; // deliberately not a readable HKX
        let binder = Binder {
            version: [0; 8],
            format: 0,
            files: vec![
                file("skeleton.hkx"),
                file("A000_000001.hkx"),
                file("A000_000002.hkx"),
                file("c1010.compendium"),
                file("c1010.tae"),
                file("other.txt"),
            ],
        };
        let mut listing = ClipListing {
            character: "c1010".into(),
            ..Default::default()
        };
        list_members(&mut listing, "c1010", &binder);
        assert_eq!(
            listing.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["a000_000001", "a000_000002"]
        );
        assert!(listing.clips.iter().all(|c| c.duration.is_none()));
        assert_ne!(listing.clips[0].key, listing.clips[1].key);
        let metadata = metadata_binder(&binder);
        assert_eq!(
            metadata
                .files
                .iter()
                .filter_map(|f| f.name.as_deref())
                .collect::<Vec<_>>(),
            ["skeleton.hkx", "c1010.compendium", "c1010.tae", "other.txt"]
        );
        let companion = Binder {
            version: binder.version,
            format: binder.format,
            files: vec![binder.files[2].clone()],
        };
        list_members(&mut listing, "c1010_div00", &companion);
        assert_eq!(
            listing.clips.len(),
            3,
            "duplicate stems in another binder retain distinct identities"
        );
        assert!(
            select_source(&listing, "a000_000002")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert_ne!(listing.clips[1].key, listing.clips[2].key);
        let mut world = World::new();
        let root = publish_listing(&listing, &mut world).unwrap();
        assert_eq!(world.get::<primase_core::Asset>(root).unwrap().members.len(), 3);
        assert_eq!(world.iter::<primase_core::Clip>().count(), 0);
        assert!(world.verify_components().is_empty());
        for (_, asset) in world.iter::<primase_core::Asset>() {
            for &(entity, _) in &asset.members {
                let source = world.get::<ClipSource>(entity).unwrap();
                let provenance = world.get::<Provenance>(entity).unwrap();
                assert_eq!(
                    provenance.origins[0].archive.as_deref(),
                    Some(source.binder.as_str())
                );
                assert_eq!(provenance.origins[0].file.as_ref(), source.member);
            }
        }
    }

    #[test]
    fn a_file_is_named_by_its_stem() {
        assert_eq!(
            stem(r"N:\NTC\data\Target\INTERROOT_win64\chr\c1010\hkx_compendium\A000_003000.hkx"),
            "a000_003000"
        );
        assert_eq!(stem("skeleton.HKX"), "skeleton");
        assert_eq!(stem("noextension"), "noextension");
        assert_eq!(file_name(r"x\C1010.TAE"), "c1010.tae");
    }

    #[test]
    fn plan_lengths_are_optional_and_invalid_lengths_do_not_mutate_the_world() {
        let binder = Binder {
            version: [0; 8],
            format: 0,
            files: vec![fromsoft_formats::BinderFile {
                id: None,
                name: Some("walk.hkx".into()),
                flags: 0,
                bytes: vec![0xff],
            }],
        };
        let mut listing = ClipListing {
            character: "c1010".into(),
            ..Default::default()
        };
        list_members(&mut listing, "c1010", &binder);
        listing.clips[0].duration = Some(2.5);
        let mut world = World::new();
        let root = publish_listing(&listing, &mut world).unwrap();
        let entity = world.get::<primase_core::Asset>(root).unwrap().members[0].0;
        assert_eq!(
            world.get::<primase_core::ClipLength>(entity),
            Some(&primase_core::ClipLength(2.5))
        );
        let next = world.next_index();
        listing.clips[0].duration = Some(f64::INFINITY);
        assert!(publish_listing(&listing, &mut world).is_err());
        assert_eq!(world.next_index(), next);
        listing.clips[0].duration = None;
        let root = publish_listing(&listing, &mut world).unwrap();
        let entity = world.get::<primase_core::Asset>(root).unwrap().members[0].0;
        assert!(!world.has::<primase_core::ClipLength>(entity));
    }

    #[test]
    fn a_cached_listed_member_refuses_unsupported_hkx_by_name_without_core_lowering() {
        let binder = Binder {
            version: [0; 8],
            format: 0,
            files: vec![fromsoft_formats::BinderFile {
                id: None,
                name: Some("walk.hkx".into()),
                flags: 0,
                bytes: vec![0xff],
            }],
        };
        let mut listing = ClipListing {
            character: "c1010".into(),
            ..Default::default()
        };
        list_members(&mut listing, "c1010", &binder);
        let source = listing.clips[0].clone();
        let mut world = World::new();
        let root = publish_listing(&listing, &mut world).unwrap();
        let entity = world.get::<primase_core::Asset>(root).unwrap().members[0].0;
        let mut reader = ClipReader {
            archive: Some(Arc::new(Archive::empty(fromsoft_formats::Game::Sekiro))),
            binders: BTreeMap::from([(source.binder.clone(), binder)]),
            listings: BTreeMap::from([("c1010".into(), listing)]),
            ..Default::default()
        };
        let decoder = Decoder {
            oodle: None,
            max_output: 1024,
        };
        let why = reader
            .load(
                &fromsoft_formats::Install::new(
                    "unused",
                    fromsoft_formats::Game::Sekiro,
                    fromsoft_formats::Keys::none(),
                ),
                &decoder,
                &source,
                entity,
                &mut world,
            )
            .unwrap_err()
            .to_string();
        assert!(
            why.contains("walk") && why.contains("Havok tagfile not read"),
            "{why}"
        );
        assert_eq!(world.get::<ClipSource>(entity), Some(&source));
        assert_eq!(world.iter::<primase_core::Clip>().count(), 0);
        assert!(world.verify_components().is_empty());
    }

    #[test]
    fn refusals_are_listed_by_reason() {
        let mut r = AnimationReport::default();
        refuse(&mut r, "why", "a");
        refuse(&mut r, "why", "b");
        refuse(&mut r, "other", "c");
        assert_eq!(r.refused["why"], ["a", "b"]);
        assert_eq!(r.refused.len(), 2);
    }
}
