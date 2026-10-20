//! The reader's side of GTA's collision: the ybn bounds of the region's map archives ([`attach_map_bounds`]), each a
//! [`GtaBounds`] entity with its detail (the prefix of a file named `hi@...` or `ma@...`: the finer copies the game
//! keeps for weapons and vehicles; none for the plain bounds a body walks or rolls on), and the install's material
//! names (`materials.dat`) as the [`GtaMaterials`] resource. Every live ybn file is read for its box, as the game
//! streams them by those boxes; those meeting the region are kept. Given GTA Online's archive set too, both states'
//! live files are read: a file live in one state only carries a `core.condition` on the `gta.map` axis.
use std::collections::BTreeMap;

use primase_core::World;
use primase_core::state::{Clause, Condition, Literal, StateAxis};

use crate::components::collision::{GtaBounds, GtaMaterials};
use crate::rage::bounds::{Bound, read_ybn};
use crate::read::archive::archive_set::{ArchiveSet, FileRef};
use crate::read::archive::keys::Keys;
use crate::{Result, invalid};

/// A ybn file name's detail: what precedes an `@` in it, lowercase.
pub fn ybn_detail(file_name: &str) -> Option<String> {
    file_name.split_once('@').map(|(d, _)| d.to_ascii_lowercase())
}

/// The material names of a `materials.dat`: after its version line, the first word of every line that is not a
/// comment (`#`) or blank, in order.
pub fn material_names(text: &str) -> Result<Vec<String>> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let version = lines.next().ok_or_else(|| invalid("materials.dat", 0, "empty"))?;
    if version.parse::<f32>().is_err() {
        return Err(invalid("materials.dat", 0, format!("version line {version:?}")));
    }
    Ok(lines
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

/// What [`attach_map_bounds`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundsReport {
    /// Live ybn files (of either state), those whose box meets the region (read), and the `hi@`/`ma@` copies among
    /// those.
    pub ybn_live: usize,
    pub ybn_read: usize,
    pub ybn_finer_read: usize,
    /// Of those read, the ones live in one state only (each with its `gta.map` condition).
    pub ybn_one_state: usize,
    pub material_names: usize,
}

/// The static map bounds whose box meets `region` (`[x0, y0, x1, y1]`, GTA ground coordinates), each as an entity
/// with [`GtaBounds`], and the material names as the [`GtaMaterials`] resource (see the module documentation). Every
/// live ybn of Story's archive set and, when given, GTA Online's (over the same install) is read for its box (in
/// parallel); the game streams them by those boxes. With Online's set, `w` holds the `gta.map` axis
/// ([`crate::read::region::read_region`]), and both states' `materials.dat` must name the same materials.
pub fn attach_map_bounds(
    w: &mut World,
    story: &ArchiveSet,
    online: Option<&ArchiveSet>,
    keys: &Keys,
    region: [f32; 4],
) -> Result<BoundsReport> {
    let mut report = BoundsReport::default();
    // every live file, with the states it is live in (Story, Online)
    let mut live: BTreeMap<FileRef, (bool, bool)> = BTreeMap::new();
    for (set, is_story) in std::iter::once((story, true)).chain(online.map(|o| (o, false))) {
        for &f in set.tables.get("ybn").into_iter().flat_map(|t| t.values()) {
            let states = live.entry(f).or_default();
            if is_story {
                states.0 = true;
            } else {
                states.1 = true;
            }
        }
    }
    // the gta.map axis a file live in one state is conditioned on (story 0, online 1)
    let axis = match online {
        Some(_) => Some(
            w.iter::<StateAxis>()
                .find(|(_, a)| a.key == "gta.map")
                .map(|(e, _)| e)
                .ok_or_else(|| invalid("ybn", 0, "GTA Online's bounds without the gta.map axis"))?,
        ),
        None => None,
    };
    let files: Vec<FileRef> = live.keys().copied().collect();
    report.ybn_live = files.len();
    let [x0, y0, x1, y1] = region;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let read = |f: FileRef| -> Result<Option<Bound>> {
        let r = story.install.resource(f, keys)?;
        let bound = read_ybn(&r)?;
        let (lo, hi) = (bound.header.box_min, bound.header.box_max);
        let meets = hi[0] >= x0 && lo[0] <= x1 && hi[1] >= y0 && lo[1] <= y1;
        Ok(meets.then_some(bound))
    };
    let results: Vec<Result<Vec<(usize, Bound)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> Result<Vec<(usize, Bound)>> {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&f) = files.get(k) else { break };
                        if let Some(b) = read(f)? {
                            out.push((k, b));
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(invalid("ybn", 0, "a reading thread panicked")))
            })
            .collect()
    });
    let mut found = Vec::new();
    for r in results {
        found.extend(r?);
    }
    found.sort_by_key(|(k, _)| *k);
    for (k, bound) in found {
        report.ybn_read += 1;
        let f = files[k];
        let path = story.path(f).to_string();
        let detail = ybn_detail(path.rsplit('\\').next().unwrap_or(&path));
        if detail.is_some() {
            report.ybn_finer_read += 1;
        }
        let e = w.spawn();
        w.insert(e, GtaBounds { path, detail, bound });
        let value = match live[&f] {
            (true, false) => 0,
            (false, true) => 1,
            _ => continue,
        };
        if let Some(axis) = axis {
            report.ybn_one_state += 1;
            w.insert(
                e,
                Condition(vec![Clause(vec![Literal {
                    axis,
                    values: 1 << value,
                }])]),
            );
        }
    }
    let materials = |set: &ArchiveSet| -> Result<Option<Vec<String>>> {
        let Some(f) = set.live("dat", "materials") else {
            return Ok(None);
        };
        let bytes = set.install.binary(f, keys)?;
        material_names(&String::from_utf8_lossy(&bytes)).map(Some)
    };
    let names = materials(story)?;
    if let Some(o) = online
        && materials(o)? != names
    {
        // one material list per world: bounds index it
        return Err(invalid(
            "materials.dat",
            0,
            "Story and GTA Online name different materials",
        ));
    }
    if let Some(names) = names {
        report.material_names = names.len();
        w.insert_resource(GtaMaterials(names));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ybn's detail is its name's prefix before `@`, lowercase.
    #[test]
    fn a_ybn_s_detail_is_its_name_s_prefix() {
        assert_eq!(ybn_detail("hi@cs1_08_3.ybn"), Some("hi".into()));
        assert_eq!(ybn_detail("MA@x.ybn"), Some("ma".into()));
        assert_eq!(ybn_detail("cs1_08_3.ybn"), None);
    }
}
