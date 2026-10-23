//! A clip dictionary (`.ycd`) read into a world ([`GtaSource::read_clips`]): one entity per animation, holding the
//! animation's bone tracks decoded ([`crate::rage::ycd_channels`]) and sampled at every frame ([`GtaAnimation`]),
//! bound to the skeleton the tracks' bone ids tag; `lower-gta-clips` makes the `core.clip`.
//!
//! An animation is stored as sequences, each a span of at most `sequence_frame_limit` frames plus the first frame of
//! the next (every sequence but the last holds `limit + 1` frames): frame `g` of the animation is frame
//! `g mod limit` of sequence `g / limit`, the last frame the last sequence's. A track a sequence has no channels for in
//! every sequence is left out (counted); one it has in some sequences only is an error. Which animations are read is
//! the caller's choice (all of the dictionary's by default); the clips that play each are kept on it ([`GtaClipUse`]).
use primase_core::{Entity, Name, Source, World};

use crate::components::{GtaAnimation, GtaAnimationTrack, GtaClipUse, GtaSkeleton};
use crate::rage::ycd::{Animation, ClipDictionary};
use crate::rage::ycd_channels::Track;
use crate::read::archive::archive_set::FileRef;
use crate::read::archive::rpf::EntryKind;
use crate::read::source::GtaSource;
use crate::{Result, invalid};

/// What reading a clip dictionary did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipsReport {
    /// The dictionary's path.
    pub path: String,
    /// The animations the dictionary holds, and the ones read.
    pub animations_in_dictionary: usize,
    pub animations: usize,
    pub clips_in_dictionary: usize,
    /// The bone tracks read, and the ones left out for holding no channels.
    pub tracks: usize,
    pub tracks_without_channels: usize,
}

/// An animation's bone tracks sampled at every frame (see the module documentation), and how many tracks had no
/// channels.
fn sampled_tracks(a: &Animation) -> Result<(Vec<GtaAnimationTrack>, usize)> {
    let fail = |detail: String| invalid("ycd", 0, detail);
    let blocks = a.decode()?;
    let frames = usize::from(a.frames);
    let limit = usize::from(a.sequence_frame_limit);
    if frames == 0 || limit == 0 || blocks.is_empty() {
        return Err(fail(format!(
            "an animation of {frames} frames in sequences of {limit} ({} sequences)",
            blocks.len()
        )));
    }
    let last = blocks.len().saturating_sub(1);
    let mut tracks = Vec::new();
    let mut without = 0usize;
    for (t, id) in a.bone_ids.iter().enumerate() {
        let per_block: Vec<Option<&Track>> = blocks
            .iter()
            .map(|b| b.tracks.get(t).and_then(Option::as_ref))
            .collect();
        let Some(width) = per_block.iter().flatten().map(|t| t.width()).next() else {
            without = without.saturating_add(1);
            continue;
        };
        if width == 0 {
            without = without.saturating_add(1);
            continue;
        }
        let mut samples = Vec::with_capacity(frames.saturating_mul(width));
        for g in 0..frames {
            let b = (g / limit).min(last);
            let local = g.saturating_sub(b.saturating_mul(limit));
            let track = per_block.get(b).copied().flatten().ok_or_else(|| {
                fail(format!(
                    "bone {:#06x} track {} has no channels in sequence {b}, which it has in another",
                    id.bone, id.track
                ))
            })?;
            if track.width() != width {
                return Err(fail(format!(
                    "bone {:#06x} track {} has {} values in one sequence and {width} in another",
                    id.bone,
                    id.track,
                    track.width()
                )));
            }
            samples.extend(track.sample(local)?.into_iter().take(width));
        }
        tracks.push(GtaAnimationTrack {
            bone: id.bone,
            track: id.track,
            unknown: id.unknown,
            width: u8::try_from(width).map_err(|_| fail("a track of more than 255 values".into()))?,
            samples,
        });
    }
    Ok((tracks, without))
}

impl GtaSource {
    /// The animations of the clip dictionary `ycd` (all of them, or only those whose hashes `only` lists) read into
    /// `w`, bound to `skeleton`, an entity holding the [`GtaSkeleton`] whose bone tags the animations' bone ids are.
    /// Each animation is an entity named after the first clip that plays it (else its hash) with a [`GtaAnimation`]
    /// and the dictionary's path as its source.
    pub fn read_clips(
        &self,
        w: &mut World,
        ycd: FileRef,
        skeleton: Entity,
        only: Option<&[u32]>,
    ) -> Result<ClipsReport> {
        let (install, keys) = (self.install(), self.keys());
        let path = install
            .archives
            .get(ycd.archive)
            .and_then(|a| a.entries.get(ycd.entry))
            .filter(|e| !matches!(e.kind, EntryKind::Directory { .. }))
            .map(|e| e.path.clone())
            .ok_or_else(|| {
                invalid(
                    "ycd",
                    0,
                    format!("no file at archive {} entry {}", ycd.archive, ycd.entry),
                )
            })?;
        if !w.has::<GtaSkeleton>(skeleton) {
            return Err(invalid(
                "ycd",
                0,
                format!("{skeleton:?} holds no gta.skeleton for {path}'s bone ids to tag"),
            ));
        }
        let dictionary = ClipDictionary::read_resource(&install.resource(ycd, keys)?)?;
        let mut report = ClipsReport {
            path: path.clone(),
            animations_in_dictionary: dictionary.animations.len(),
            clips_in_dictionary: dictionary.clips.len(),
            ..ClipsReport::default()
        };
        for (hash, a) in &dictionary.animations {
            if only.is_some_and(|only| !only.contains(hash)) {
                continue;
            }
            let (tracks, without) = sampled_tracks(a)
                .map_err(|e| invalid("ycd", 0, format!("{path}, animation {hash:#010x}: {e}")))?;
            let clips: Vec<GtaClipUse> = dictionary
                .clips
                .iter()
                .flat_map(|c| {
                    c.plays
                        .iter()
                        .filter(|p| p.animation == *hash)
                        .map(|p| GtaClipUse {
                            hash: c.hash,
                            name: c.name.clone(),
                            start: p.start,
                            end: p.end,
                            rate: p.rate,
                        })
                })
                .collect();
            report.tracks = report.tracks.saturating_add(tracks.len());
            report.tracks_without_channels = report.tracks_without_channels.saturating_add(without);
            report.animations = report.animations.saturating_add(1);
            let e = w.spawn();
            w.insert(
                e,
                Name(
                    clips
                        .first()
                        .map_or_else(|| format!("{hash:#010x}"), |c| c.name.clone()),
                ),
            );
            w.insert(e, Source(path.clone()));
            w.insert(
                e,
                GtaAnimation {
                    skeleton,
                    hash: *hash,
                    frames: u32::from(a.frames),
                    duration: a.duration,
                    flags: a.flags,
                    tracks,
                    clips,
                },
            );
        }
        Ok(report)
    }
}
