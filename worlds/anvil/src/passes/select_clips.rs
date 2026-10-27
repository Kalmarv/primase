//! `select-anvil-clips{min-coverage=F, max-clips=N}`: which of the library's Animations a rig gets to play, **chosen by
//! track coverage, not by the engine's Kino graph**. The reader (`Placer::with_clip_candidates`) gives each rig every
//! Animation with at least one joint-driving track on one of its joints ([`AnvilClipCandidates`]); this pass keeps the
//! ones that fit it and binds them to every placed occurrence of the rig.
//!
//! The rider clips of Riders Republic are not played from an Entity's AnimComponent: the game's Kino animation graph
//! (`KinoGraphData`, `KinoExternalStateData`, `KinoExternalResourceHolder`) holds which clip plays when, and nothing
//! here reads it. What this pass states is which clips **can** play on a rig, because their tracks fit its joints, not
//! which the game plays.
//!
//! - **Coverage** of a clip on a rig: of the clip's joint-driving tracks (a translation or rotation track of a bone,
//!   not the root's motion of hash 0: [`StoredAnimation::joint_tracks`](anvil_formats::animation::StoredAnimation)), the
//!   fraction whose bone hash names one of the rig's joints. The tracks of other kinds count for nothing either way.
//! - **`min-coverage`** `F`, a fraction in (0, 1]: a clip stays when its coverage on the rig is at least `F`. The
//!   program's choice, none read from the game.
//! - **`max-clips`** `N`, at least 1: at most `N` clips per rig stay, the highest coverage first (then the most tracks
//!   covered, then the name). The program's choice too.
//! - A clip an Entity's own AnimComponent already plays on that rig is not chosen again.
//!
//! Chosen clips are made as [`AnvilClip`] on the rig, each bound ([`AnvilClipBinding`]) to every occurrence of the rig
//! (`lower-anvil-clips` lowers both); this runs before it. Counted: rigs, candidates, those below `F`, those over `N`,
//! clips chosen, bindings made, and per rig how many were chosen; the bindings are the approximation's count.

use std::collections::{HashMap, HashSet};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::rig::RigInstance;
use primase_core::{Component, Entity, Name, World};

use crate::components::{
    AnvilClip, AnvilClipBinding, AnvilClipCandidates, AnvilRigOccurrence, ClipCandidate,
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectAnvilClips {
    pub min_coverage: f64,
    pub max_clips: usize,
}

impl SelectAnvilClips {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<SelectAnvilClips, String> {
        let coverage = g.required("min-coverage");
        let min_coverage = coverage
            .parse::<f64>()
            .map_err(|_| format!("min-coverage={coverage} is not a number"))?;
        if !(min_coverage.is_finite() && min_coverage > 0.0 && min_coverage <= 1.0) {
            return Err(format!("min-coverage={coverage}: a fraction, 0 < F <= 1"));
        }
        let max = g.required("max-clips");
        let max_clips = max
            .parse::<usize>()
            .map_err(|_| format!("max-clips={max} is not a whole number"))?;
        if max_clips == 0 {
            return Err(format!("max-clips={max}: at least 1"));
        }
        Ok(SelectAnvilClips {
            min_coverage,
            max_clips,
        })
    }
}

/// `select-anvil-clips`' declaration ([`primase_core::pass::PassRegistry::add`]): the minimum coverage and the cap
/// are the program's.
pub const SELECT_ANVIL_CLIPS: PassSpec = PassSpec {
    name: "select-anvil-clips",
    options: &[
        OptionSpec::required(
            "min-coverage",
            "0.9",
            "0 < F <= 1: the least fraction of a clip's tracks that fit a rig's joints",
        ),
        OptionSpec::required("max-clips", "16", "at least 1: the most clips a rig keeps"),
    ],
    make: |g| Ok(Box::new(SelectAnvilClips::from_given(g)?)),
};

/// The fraction of `c`'s clip's joint-driving tracks that `c` covers.
fn coverage(c: &ClipCandidate) -> f64 {
    let tracks = u32::try_from(c.clip.joint_tracks.len()).unwrap_or(u32::MAX);
    f64::from(c.covered) / f64::from(tracks.max(1))
}

/// Highest coverage first, then the most tracks covered, then the clip's name and id. Coverage is compared as
/// fractions of whole numbers (cross-multiplied), not as floats.
fn ranked(a: &ClipCandidate, b: &ClipCandidate) -> std::cmp::Ordering {
    let tracks = |c: &ClipCandidate| {
        u64::try_from(c.clip.joint_tracks.len())
            .unwrap_or(u64::MAX)
            .max(1)
    };
    let (ta, tb) = (tracks(a), tracks(b));
    (u64::from(b.covered).saturating_mul(ta))
        .cmp(&u64::from(a.covered).saturating_mul(tb))
        .then(b.covered.cmp(&a.covered))
        .then_with(|| a.clip.name.cmp(&b.clip.name))
        .then(a.clip.id.cmp(&b.clip.id))
}

impl Pass for SelectAnvilClips {
    fn name(&self) -> &'static str {
        "select-anvil-clips"
    }
    fn options(&self) -> Options {
        Options::from([
            ("min-coverage".to_owned(), self.min_coverage.to_string()),
            ("max-clips".to_owned(), self.max_clips.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilClipCandidates::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[AnvilClip::NAME, AnvilClipBinding::NAME, Name::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilClipCandidates::NAME]
    }
    fn approximates(&self) -> &'static str {
        "clips chosen by how many of their tracks fit a rig's joints (the program's minimum coverage and per-rig cap), \
         not by the engine's Kino graph: which clips can play on a rig, not which the game plays"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the occurrences of each rig, as the reader placed them or as lower-anvil-skin made them
        let mut occurrences: HashMap<Entity, Vec<Entity>> = HashMap::new();
        for (e, o) in w.iter::<AnvilRigOccurrence>() {
            occurrences.entry(o.rig).or_default().push(e);
        }
        for (e, i) in w.iter::<RigInstance>() {
            occurrences.entry(i.skeleton).or_default().push(e);
        }
        let rigs: Vec<Entity> = w.iter::<AnvilClipCandidates>().map(|(e, _)| e).collect();
        for rig in rigs {
            let Some(found) = w.remove::<AnvilClipCandidates>(rig) else {
                continue;
            };
            let rig_name = w.get::<Name>(rig).map_or_else(String::new, |n| n.0.clone());
            log.count("rigs given candidate clips", 1);
            log.count(
                "candidate clips (one joint-driving track or more on the rig)",
                u64::try_from(found.candidates.len()).unwrap_or(u64::MAX),
            );
            let places = occurrences.get(&rig).cloned().unwrap_or_default();
            if places.is_empty() {
                log.count("rigs with no placed occurrence (no clip chosen)", 1);
                continue;
            }
            let played: HashSet<u64> = w
                .iter::<AnvilClip>()
                .filter(|(_, c)| c.rig == rig)
                .map(|(_, c)| c.id)
                .collect();
            let mut fits: Vec<&ClipCandidate> = Vec::new();
            for c in &found.candidates {
                if coverage(c) < self.min_coverage {
                    log.count("candidate clips below min-coverage", 1);
                } else if played.contains(&c.clip.id) {
                    log.count("candidate clips its AnimComponent already plays", 1);
                } else {
                    fits.push(c);
                }
            }
            fits.sort_by(|a, b| ranked(a, b));
            let over = fits.len().saturating_sub(self.max_clips);
            log.count(
                "candidate clips over max-clips",
                u64::try_from(over).unwrap_or(u64::MAX),
            );
            fits.truncate(self.max_clips);
            let chosen = u64::try_from(fits.len()).unwrap_or(u64::MAX);
            log.count("clips chosen", chosen);
            log.count(&format!("clips chosen for rig {rig_name}"), chosen);
            for c in fits {
                let clip = w.spawn();
                w.insert(clip, Name(c.clip.name.clone()));
                w.insert(
                    clip,
                    AnvilClip {
                        frame: found.frame,
                        id: c.clip.id,
                        animation: c.clip.animation.clone(),
                        rig,
                    },
                );
                for &occurrence in &places {
                    let name = w.get::<Name>(occurrence).cloned();
                    let b = w.spawn();
                    if let Some(name) = name {
                        w.insert(b, name);
                    }
                    w.insert(b, AnvilClipBinding { clip, occurrence });
                    log.approximate(
                        "clip bindings chosen by track coverage (min-coverage and max-clips the program's), not by the engine's Kino graph",
                        1,
                    );
                }
            }
        }
        Ok(())
    }
}
