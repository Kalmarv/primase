//! How often two worn parts give one dummy polygon number different places: what `assemble-character` reports when it
//! carries the parts' sockets onto one skeleton (a name two parts place otherwise keeps the first part's). Every model of
//! every part binder of the dictionaries that the archives hold, weapons (`wp_`) left out (a weapon is held, not worn:
//! `leave-out=`), by kind (the binder's first two letters: `hd`, `bd`, `am`, `lg`, `fc`...; one part of each kind is worn
//! at once): for each dummy polygon reference id, the kinds whose models carry it, and of the ids two kinds carry, those
//! where a model of one kind and a model of another disagree (another attach or parent bone, by name, or a position or
//! direction more than 1e-3 apart). Counted per install; the counts are the test's expectations.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` and `oracles.uxm.{sekiro,elden}/dictionary.txt`;
//! a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::flver::Model;
use fromsoft_formats::{Binder, Game, Keys, NameDictionary, dcx::Decoder};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A dummy polygon as the comparison sees it: its bones by name and its stored vectors.
#[derive(Clone, Debug)]
struct Placed {
    kind: String,
    model: String,
    attach: String,
    parent: String,
    vectors: [[f32; 3]; 3],
}

impl Placed {
    fn alike(&self, other: &Placed) -> bool {
        self.attach == other.attach
            && self.parent == other.parent
            && self
                .vectors
                .iter()
                .zip(&other.vectors)
                .all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-3))
    }
}

/// What the census counts for one install.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census {
    binders: usize,
    models: usize,
    models_with_dummies: usize,
    /// Reference ids carried by some model, by models of two kinds or more, and of those, the ids where models of two kinds
    /// disagree.
    ids: usize,
    shared: usize,
    disagreeing: usize,
}

#[test]
fn the_dummy_polygons_worn_parts_share_are_counted_by_agreement() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(folder), Some(names)) = (
            install(data),
            keys(data),
            oracle(OracleSet::Uxm, data, "dictionary.txt"),
        ) else {
            continue;
        };
        let archive = common::open_game(root, game, &Keys::from_folder(folder, game, None)?)?;
        let dictionary = NameDictionary::read_for(names, game)?;
        let paths: BTreeSet<&str> = dictionary
            .names
            .values()
            .flatten()
            .map(String::as_str)
            .filter(|p| {
                p.starts_with("/parts/")
                    && p.contains(".partsbnd")
                    && !p.starts_with("/parts/wp_")
                    && archive.contains(p)
            })
            .collect();
        let mut census = Census::default();
        let mut by_id: BTreeMap<i16, Vec<Placed>> = BTreeMap::new();
        for path in paths {
            census.binders += 1;
            let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
            for member in &binder.files {
                let Some(name) = member.name.as_deref() else {
                    continue;
                };
                if !name.to_ascii_lowercase().ends_with(".flver") {
                    continue;
                }
                let model = Model::read(&decoder.decode_if_dcx(&member.bytes)?)?;
                census.models += 1;
                if model.dummies.is_empty() {
                    continue;
                }
                census.models_with_dummies += 1;
                let bone = |i: i16| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| model.bones.get(i))
                        .map_or_else(String::new, |b| b.name.clone())
                };
                let mut seen = BTreeSet::new();
                for d in &model.dummies {
                    // a model that repeats a number counts it once (its first), as the socket carry does
                    if !seen.insert(d.reference_id) {
                        continue;
                    }
                    by_id.entry(d.reference_id).or_default().push(Placed {
                        kind: path["/parts/".len()..].get(..2).unwrap_or_default().to_owned(),
                        model: format!("{path}!{name}"),
                        attach: bone(d.attach_bone),
                        parent: bone(d.parent_bone),
                        vectors: [d.position, d.forward, d.upward],
                    });
                }
            }
        }
        census.ids = by_id.len();
        let across = |placed: &[Placed]| {
            placed
                .iter()
                .any(|a| placed.iter().any(|b| a.kind != b.kind && !a.alike(b)))
        };
        for placed in by_id.values() {
            let kinds: BTreeSet<&str> = placed.iter().map(|p| p.kind.as_str()).collect();
            if kinds.len() > 1 {
                census.shared += 1;
                if across(placed) {
                    census.disagreeing += 1;
                }
            }
        }
        println!("FS_PART_SOCKETS={game:?} {census:?}");
        let sample: Vec<String> = by_id
            .iter()
            .filter(|(_, p)| across(p))
            .take(8)
            .map(|(id, p)| {
                let kinds: BTreeSet<&str> = p.iter().map(|q| q.kind.as_str()).collect();
                format!(
                    "{id}: kinds {kinds:?}, {} models, first {} {} / {}",
                    p.len(),
                    p[0].model,
                    p[0].attach,
                    p[0].parent
                )
            })
            .collect();
        println!("FS_PART_SOCKETS_SAMPLE={game:?} {sample:?}");
        // no dummy polygon number is carried by models of two kinds: Sekiro's are all on face models (each carries the
        // player's 307), Elden Ring's 45 each on one kind
        let expected = if game == Game::Sekiro {
            Census {
                binders: 86,
                models: 79,
                models_with_dummies: 12,
                ids: 307,
                shared: 0,
                disagreeing: 0,
            }
        } else {
            Census {
                binders: 2_026,
                models: 1_950,
                models_with_dummies: 620,
                ids: 45,
                shared: 0,
                disagreeing: 0,
            }
        };
        assert_eq!(census, expected, "{game:?}");
    }
    Ok(())
}
