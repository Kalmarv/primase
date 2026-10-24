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
//! Nothing is lowered here: a program chooses what the clips are (`lower-havok-clips`: what stands in for a component a
//! track does not state, the keys for each frame, which animations, the root motion) and how they reach the model's
//! skeleton (`correspond-skeletons`, `map-clip`). The Havok skeleton is in the FLVER's frame (the reference poses of the
//! bones the two share are equal as stored: measured on `c1010` and `c0000`), so both go to the world's by the same
//! frame.
//!
//! What the reader does not read is named: animations that are not spline compressed (one interleaved one in the
//! install), files that are not tagfiles of a supported SDK (one 2015.1 file), animations binding a skeleton the binder
//! does not hold, an animation named as one already read, and the `.tae` event file.

use std::collections::BTreeMap;

use primase_core::{Entity, Name, Origin, Provenance, World};
use primase_havok::animation::{Container, Skeleton, container};
use primase_havok::world::{HavokAnimation, insert_animation, insert_skeleton};
use primase_havok::{Compendium, Tagfile};

use crate::{Archive, Binder, Result, dcx::Decoder};

/// What reading a character's animation binders found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationReport {
    /// The Havok skeleton's bones (0 when the binders hold none).
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
    /// Members of the binders this reader does not read (`c1010.tae`, ...), by name.
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
fn file_name(name: &str) -> String {
    crate::names::fold(name.rsplit(['/', '\\']).next().unwrap_or(name))
}

/// The file's name after its last separator, lowercased, without its extension.
fn stem(name: &str) -> String {
    let file = file_name(name);
    file.rsplit_once('.').map_or(file.clone(), |(s, _)| s.to_owned())
}

fn refuse(report: &mut AnimationReport, why: &str, file: &str) {
    let list = report.refused.entry(why.to_owned()).or_default();
    list.push(file.to_owned());
}

fn binder_path(stem: &str) -> String {
    format!("/chr/{stem}.anibnd.dcx")
}

/// The binders a binder's marker members name as this character's (`<id>_<set>.txt` or `.blf` with an archive binder of
/// that name), in the binder's order, each once.
fn companions(archive: &Archive, id: &str, binder: &Binder) -> Vec<String> {
    let prefix = format!("{id}_");
    let mut out: Vec<String> = Vec::new();
    for f in &binder.files {
        let Some(name) = f.name.as_deref() else {
            continue;
        };
        let file = file_name(name);
        let Some(set) = file.strip_suffix(".txt").or_else(|| file.strip_suffix(".blf")) else {
            continue;
        };
        if set.starts_with(&prefix) && archive.contains(&binder_path(set)) && !out.iter().any(|o| o == set) {
            out.push(set.to_owned());
        }
    }
    out
}

/// What one binder holds: its skeletons, and its animation containers by file stem, each with where it was read from.
struct Held {
    skeletons: Vec<Skeleton>,
    animations: Vec<(String, Origin, Container)>,
}

/// Read the tagfiles of `binder` (one that names a marker for a binder in `markers` is not unread).
fn hold(
    binder: &Binder,
    binder_name: &str,
    decoder: &Decoder<'_>,
    markers: &[String],
    report: &mut AnimationReport,
) -> Result<Held> {
    let compendium = match binder.files.iter().find(|f| {
        f.name
            .as_deref()
            .is_some_and(|n| crate::names::fold(n).ends_with(".compendium"))
    }) {
        Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
        None => None,
    };
    let mut held = Held {
        skeletons: Vec::new(),
        animations: Vec::new(),
    };
    for f in &binder.files {
        let name = f.name.as_deref().unwrap_or("");
        let lower = crate::names::fold(name);
        if lower.ends_with(".compendium") {
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
        let tag = match Tagfile::read(&bytes, compendium.as_ref()) {
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

/// The names of the animations character `id` has (`a000_003000`), in binder order, as [`read`] would read them,
/// without reading any: the `.hkx` members of `chr/<id>.anibnd` and of the binders its markers name, except the
/// skeleton (`skeleton.hkx`). `None` when the archive has no such binder. Whether a file is a tagfile that reads is
/// `read`'s to say (its report refuses the others by reason): this names what the binders hold.
pub fn clip_names(archive: &Archive, decoder: &Decoder<'_>, id: &str) -> Result<Option<Vec<String>>> {
    let path = binder_path(id);
    if !archive.contains(&path) {
        return Ok(None);
    }
    let main = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let mut binders = vec![];
    for set in companions(archive, id, &main) {
        binders.push(Binder::read(
            &archive.read(&binder_path(&set), decoder)?,
            decoder,
        )?);
    }
    let mut names: Vec<String> = Vec::new();
    for binder in std::iter::once(&main).chain(&binders) {
        for f in &binder.files {
            let Some(name) = f.name.as_deref() else {
                continue;
            };
            let file = stem(name);
            if file_name(name).ends_with(".hkx") && file != "skeleton" && !names.contains(&file) {
                names.push(file);
            }
        }
    }
    Ok(Some(names))
}

/// Read `chr/<id>.anibnd` of the archive (and the binders its markers name) into `w`, which holds the character's
/// model: `None` when the archive has no such binder. The skeleton and animations are in the world's frame by
/// [`crate::pose`]'s source frame.
pub(super) fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    id: &str,
    w: &mut World,
) -> Result<Option<Animations>> {
    let path = binder_path(id);
    if !archive.contains(&path) {
        return Ok(None);
    }
    let main = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let named = companions(archive, id, &main);
    let mut report = AnimationReport::default();
    let mut held = vec![hold(
        &main,
        &format!("{id}.anibnd"),
        decoder,
        &named,
        &mut report,
    )?];
    for set in &named {
        let binder = Binder::read(&archive.read(&binder_path(set), decoder)?, decoder)?;
        held.push(hold(
            &binder,
            &format!("{set}.anibnd"),
            decoder,
            &[],
            &mut report,
        )?);
        report.companions.push(set.clone());
    }
    insert(held, id, report, w).map(Some)
}

/// Read an animation binder another binder holds (a part form's own: `wp_a_0761_1.anibnd` in
/// `parts/wp_a_0761_l.partsbnd`) into `w` as [`read`] reads a character's, its skeleton named after `name` (the form):
/// it names no companion binders (a marker member is unread).
pub(super) fn read_member(
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
    insert(held, name, report, w)
}

/// The animation binder of a placed object's model (Sekiro's `obj/o001010`, Elden Ring's `aeg007_310`), decoded, if it
/// has one: a `.anibnd` member of the model's own binder (`model_binder`, `binder`), else a binder beside it
/// (`/obj/o001010.anibnd.dcx` for `/obj/o001010.objbnd.dcx`). Which of the two the install uses, if either, is what
/// `tests/object_animation_census.rs` prints; a model with neither is static.
pub(super) fn object_binder(
    archive: &Archive,
    decoder: &Decoder<'_>,
    model_binder: &str,
    binder: &Binder,
) -> Result<Option<Vec<u8>>> {
    let member = binder.files.iter().find(|f| {
        f.name.as_deref().is_some_and(|n| {
            let file = file_name(n);
            file.ends_with(".anibnd") || file.ends_with(".anibnd.dcx")
        })
    });
    if let Some(f) = member {
        return decoder.decode_if_dcx(&f.bytes).map(Some);
    }
    let beside = [".objbnd.dcx", ".geombnd.dcx"]
        .iter()
        .find_map(|ext| model_binder.strip_suffix(ext))
        .map(|stem| format!("{stem}.anibnd.dcx"));
    match beside {
        Some(path) if archive.contains(&path) => Ok(Some(archive.read(&path, decoder)?)),
        _ => Ok(None),
    }
}

/// Put what the binders `held` (in read order) hold into `w`: the skeletons, each once by name (`<id> havok
/// skeleton`), and the animations binding them, what is refused named in `report`.
fn insert(held: Vec<Held>, id: &str, report: AnimationReport, w: &mut World) -> Result<Animations> {
    let mut out = Animations {
        report,
        ..Animations::default()
    };
    let mut skeletons: BTreeMap<String, (Entity, Skeleton)> = BTreeMap::new();
    for s in held.iter().flat_map(|h| &h.skeletons) {
        if skeletons.contains_key(&s.name) {
            continue;
        }
        let entity = insert_skeleton(w, s, &crate::pose::FRAME)?;
        w.insert(entity, Name(format!("{id} havok skeleton")));
        out.report.bones += s.bones.len();
        out.skeleton.get_or_insert(entity);
        skeletons.insert(s.name.clone(), (entity, s.clone()));
    }
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
    Ok(out)
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
        assert_eq!(file_name(r"x\C1010.TAE"), "c1010.tae");
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
