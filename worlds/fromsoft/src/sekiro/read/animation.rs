//! A character's animation binder (`chr/<id>.anibnd`) read into the world beside its model: the Havok skeleton
//! (`skeleton.hkx`) as a `core.skeleton` of its own and each animation as a `havok.animation` (the spline compressed
//! tracks as stored, the binding that says which bone each drives), one entity each, named by the file (`a000_003000`).
//!
//! Nothing is lowered here: a program chooses what the clips are (`lower-havok-clips`: what stands in for a component a
//! track does not state, the keys for each frame, which animations) and how they reach the model's skeleton
//! (`correspond-skeletons`, `map-clip`). The Havok skeleton is in the FLVER's frame (the reference poses of the bones
//! the two share are equal as stored: measured on `c1010` and `c0000`), so both go to the world's by the same frame.
//!
//! What the reader does not read is named: animations that are not spline compressed (one interleaved one in the
//! install), files that are not tagfiles of a supported SDK (one 2015.1 file), animations binding a skeleton the binder
//! does not hold, and the `.tae` event file.

use std::collections::BTreeMap;

use primase_core::{Entity, Name, World};
use primase_havok::animation::{Skeleton, container};
use primase_havok::world::{insert_animation, insert_skeleton};
use primase_havok::{Compendium, Tagfile};

use crate::{Archive, Binder, Result, dcx::Decoder};

/// What reading a character's animation binder found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationReport {
    /// The Havok skeleton's bones (0 when the binder holds none).
    pub bones: usize,
    /// `havok.animation` entities made.
    pub animations: usize,
    /// Files not read, by reason, each with the stems of its first few.
    pub refused: BTreeMap<String, Vec<String>>,
    /// Members of the binder this reader does not read (`c1010.tae`, ...), by name.
    pub unread: Vec<String>,
}

/// The entities a read made: the Havok skeleton and each animation.
#[derive(Debug, Default)]
pub struct Animations {
    pub skeleton: Option<Entity>,
    pub animations: Vec<Entity>,
    pub report: AnimationReport,
}

/// The file's name after its last separator, lowercased.
fn stem(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase();
    file.rsplit_once('.').map_or(file.clone(), |(s, _)| s.to_owned())
}

fn refuse(report: &mut AnimationReport, why: &str, file: &str) {
    let list = report.refused.entry(why.to_owned()).or_default();
    list.push(file.to_owned());
}

/// Read `chr/<id>.anibnd` of the archive into `w`, which holds the character's model: `None` when the archive has no
/// such binder. The skeleton and animations are in the world's frame by [`crate::pose`]'s source frame.
pub(super) fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
    w: &mut World,
) -> Result<Option<Animations>> {
    let path = format!("/chr/{id}.anibnd.dcx");
    if !archive.contains(&path) {
        return Ok(None);
    }
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let compendium = match binder.files.iter().find(|f| {
        f.name
            .as_deref()
            .is_some_and(|n| n.to_lowercase().ends_with(".compendium"))
    }) {
        Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
        None => None,
    };
    let mut out = Animations::default();
    let mut skeletons: BTreeMap<String, (Entity, Skeleton)> = BTreeMap::new();
    let mut animation_files = Vec::new();
    for f in &binder.files {
        let name = f.name.as_deref().unwrap_or("");
        let lower = name.to_lowercase();
        if lower.ends_with(".compendium") {
            continue;
        }
        if !lower.ends_with(".hkx") {
            out.report.unread.push(stem_with_extension(name));
            continue;
        }
        let file = stem(name);
        let bytes = decoder.decode_if_dcx(&f.bytes)?;
        let tag = match Tagfile::read(&bytes, compendium.as_ref()) {
            Ok(t) => t,
            Err(e) => {
                refuse(
                    &mut out.report,
                    &format!("Havok tagfile not read: {}", e.detail),
                    &file,
                );
                continue;
            }
        };
        let c = match container(&tag) {
            Ok(c) => c,
            Err(e) => {
                refuse(
                    &mut out.report,
                    &format!("animation container not read: {}", e.detail),
                    &file,
                );
                continue;
            }
        };
        if let Some(s) = c.skeletons.first() {
            let entity = insert_skeleton(w, s, &crate::pose::FRAME)?;
            w.insert(entity, Name(format!("{id} havok skeleton")));
            out.report.bones += s.bones.len();
            out.skeleton.get_or_insert(entity);
            skeletons.insert(s.name.clone(), (entity, s.clone()));
        } else {
            animation_files.push((file, c));
        }
    }
    for (file, c) in animation_files {
        let Some(binding) = c.bindings.first() else {
            refuse(&mut out.report, "an animation with no binding", &file);
            continue;
        };
        let Some((skeleton, havok)) = skeletons.get(&binding.original_skeleton_name) else {
            refuse(
                &mut out.report,
                "an animation binding a skeleton the binder does not hold",
                &file,
            );
            continue;
        };
        match insert_animation(w, &file, &c, *skeleton, havok, &crate::pose::FRAME) {
            Ok(Some(e)) => {
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
    Ok(Some(out))
}

fn stem_with_extension(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_by_its_stem() {
        assert_eq!(
            stem(r"N:\NTC\data\Target\INTERROOT_win64\chr\c1010\hkx_compendium\A000_003000.hkx"),
            "a000_003000"
        );
        assert_eq!(stem("skeleton.HKX"), "skeleton");
        assert_eq!(stem("noextension"), "noextension");
        assert_eq!(stem_with_extension(r"x\C1010.TAE"), "c1010.tae");
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
