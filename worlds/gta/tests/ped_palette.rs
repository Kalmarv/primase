#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The bone palettes of skinned drawables read from the install: every ped fragment of `streamedpeds` and every
//! entry of the `componentpeds` drawable dictionaries (the files that carry skinned geometry in volume).
//!
//! What the install's data says, measured (no oracle lists palettes; CodeWalker reads them as
//! `DrawableGeometry.BoneIds`, the ushorts at the pointer in the geometry's 0x68 with their count at 0x72):
//! - every skinned geometry (blend weights and indices) has a palette, of at most 256 ids;
//! - the palette of a ped component's geometry is the identity (id `i` at entry `i`, as many as the ped skeleton has
//!   bones the geometry may use, 98 to 133): its blend indices are bone indices of the ped's skeleton; a streamed ped
//!   fragment's geometry has the one-entry palette `[0]` with every index 0, and its ids are indices of its
//!   fragment's skeleton;
//! - a vertex's blend weights sum to 255 in all but the geometries of `UNEXPLAINED`, which store weights that do not
//!   (and indices of 255 in lanes with weight): a layout of those three the reader does not know. They are counted,
//!   not skipped: a fourth fails.
//!
//! The counts are printed. Needs the GTA install and its key material (`primase_testdata::install` and `keys`).
use std::collections::BTreeSet;

use primase_gta::rage::drawable::{Drawable, DrawableDictionary, Fragment, Geometry};
use primase_gta::read::archive::archive_set::Install;
use primase_gta::read::archive::keys::Keys;
use primase_testdata::{Game, install, keys};

/// The drawable dictionaries holding a geometry whose blend weights do not sum to 255 (file names).
const UNEXPLAINED: [&str; 3] = ["a_f_y_beach_01.ydd", "csb_bride.ydd", "csb_groom.ydd"];

#[derive(Default, Debug)]
struct Tally {
    files: usize,
    drawables: usize,
    geometries: usize,
    skinned: usize,
    identity_palettes: usize,
    max_palette: usize,
    /// Palette ids that are no bone index of the fragment's skeleton.
    foreign_ids: Vec<String>,
    unpalettized: Vec<String>,
    /// Files with a geometry whose weights do not sum to 255 on some vertex.
    odd_weights: BTreeSet<String>,
}

/// An element's bytes as four-byte vertices.
fn element(g: &Geometry, name: &str) -> Option<Vec<[u8; 4]>> {
    let e = g.elements.iter().find(|e| e.semantic() == name)?;
    Some(g.element_bytes(*e).as_chunks::<4>().0.to_vec())
}

fn check(d: &Drawable, file: &str, name: &str, tally: &mut Tally) {
    tally.drawables += 1;
    let bones = d.skeleton.as_ref().map(|s| s.local.len());
    for model in d.lods.iter().flatten() {
        for g in &model.geometries {
            tally.geometries += 1;
            tally.max_palette = tally.max_palette.max(g.bone_ids.len());
            let (Some(weights), Some(indices)) = (element(g, "BLENDWEIGHT0"), element(g, "BLENDINDICES0"))
            else {
                continue;
            };
            tally.skinned += 1;
            if g.bone_ids.is_empty() {
                tally.unpalettized.push(file.to_owned());
                continue;
            }
            if g.bone_ids.iter().enumerate().all(|(i, &b)| usize::from(b) == i) {
                tally.identity_palettes += 1;
            }
            if let Some(bones) = bones {
                for &id in &g.bone_ids {
                    if usize::from(id) >= bones {
                        tally.foreign_ids.push(format!("{file}: {id} of {bones} bones"));
                    }
                }
            }
            if weights
                .iter()
                .any(|w| w.iter().map(|&x| u32::from(x)).sum::<u32>() != 255)
            {
                tally.odd_weights.insert(name.to_owned());
            } else {
                // the weighted indices of a normal geometry all name entries of the palette
                for (w, i) in weights.iter().zip(&indices) {
                    for (&wt, &ix) in w.iter().zip(i) {
                        assert!(
                            wt == 0 || usize::from(ix) < g.bone_ids.len(),
                            "{file}: index {ix} of a {}-entry palette",
                            g.bone_ids.len()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn skinned_geometries_have_bone_palettes_of_their_skeletons() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&keys).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let mut yft = Tally::default();
    let mut ydd = Tally::default();
    for f in install.listing() {
        let ped = |what: &str| f.archive.contains(what);
        if f.extension == "yft" && ped("streamedpeds") {
            let r = install.resource(f.file, &keys).expect("resource");
            let frag = Fragment::read_resource(&r).unwrap_or_else(|e| panic!("{}: {e}", f.path));
            yft.files += 1;
            if let Some(d) = &frag.drawable {
                check(d, f.path, f.name, &mut yft);
            }
        } else if f.extension == "ydd" && ped("componentpeds") {
            let r = install.resource(f.file, &keys).expect("resource");
            let dict = DrawableDictionary::read_resource(&r).unwrap_or_else(|e| panic!("{}: {e}", f.path));
            ydd.files += 1;
            for d in &dict.drawables {
                check(d, f.path, f.name, &mut ydd);
            }
        }
    }
    println!("streamed ped fragments: {yft:?}");
    println!("component ped dictionaries: {ydd:?}");
    assert!(yft.files > 0 && ydd.files > 0);
    for (what, t) in [("fragments", &yft), ("dictionaries", &ydd)] {
        assert!(t.skinned > 0, "{what}: no skinned geometry");
        assert!(
            t.unpalettized.is_empty(),
            "{what}: skinned geometries without a palette, e.g. {:?}",
            t.unpalettized.first()
        );
        assert_eq!(
            t.identity_palettes, t.skinned,
            "{what}: a palette that is not the identity (a bone index at every entry)"
        );
        assert!(
            t.foreign_ids.is_empty(),
            "{what}: palette ids past the skeleton's bones, e.g. {:?}",
            t.foreign_ids.first()
        );
        assert!(t.max_palette <= 256);
    }
    let odd: BTreeSet<&str> = ydd.odd_weights.iter().map(String::as_str).collect();
    assert_eq!(
        odd,
        UNEXPLAINED.into_iter().collect::<BTreeSet<_>>(),
        "geometries with weights not summing to 255"
    );
    assert!(yft.odd_weights.is_empty(), "{:?}", yft.odd_weights);
}
