//! The GTA reader of [`primase_core::convert`]: a region of an install, read once to plan from and loaded part by
//! part ([`crate::read::source::GtaSource`]). Which selection and passes a conversion runs is its caller's choice: the
//! reader reads every entity of both maps within reach, with their state as conditions, and chooses none of it.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::{Entity, World};

use crate::read::source::{GtaSource, prototype_box};

/// Every component and resource a region's read and load put in a world, by name (the names in the namespaces
/// [`Reader::emits`] states): what a coverage report lists of this reader. The reader's honesty test (real data:
/// `installs.gta`, `keys.gta`) reads a corner of Legion Square and checks that its world holds no other name.
pub const EMITTED_NAMES: &[&str] = &[
    "core.condition",
    "core.instance",
    "core.lods",
    "core.mesh",
    "core.name",
    "core.provenance",
    "core.source",
    "core.state_axis",
    "core.texture",
    "core.transform",
    "gta.archetype",
    "gta.bones",
    "gta.bounds",
    "gta.drawable",
    "gta.embedded_bounds",
    "gta.entity",
    "gta.lights",
    "gta.materials",
    "gta.resolved_in",
    "gta.shader",
    "gta.skeleton",
    "gta.sources",
    "gta.water",
    "gta.water_surface",
];
/// What to read: the install and its key folder, the region, and how far beyond it entities are read. Both maps'
/// entities are read, with `gta.map` conditions, each prototype's drawables and textures from the state it is resolved
/// in.
#[derive(Clone, Debug)]
pub struct GtaOptions {
    pub install: PathBuf,
    pub keys: PathBuf,
    /// `[x0, y0, x1, y1]`, GTA ground coordinates.
    pub region: [f32; 4],
    /// Metres outside the region entities are read, so large pieces of the world that reach in are seen.
    pub reach: f32,
}

/// The GTA reader (see the module documentation).
pub struct GtaReader {
    source: GtaSource,
    region: [f32; 4],
    reach: f32,
    /// What the last read or load couldn't find, until [`Reader::notices`] takes it.
    pending: Vec<Notice>,
}

/// The stage the reader's notices name.
const STAGE: &str = "gta reader";

/// What reading couldn't find: placements whose archetype has no definition in the archives (left out).
fn read_notices(missing_archetypes: usize) -> Vec<Notice> {
    let mut out = Vec::new();
    if missing_archetypes > 0 {
        out.push(Notice::new(
            STAGE,
            NoticeKind::Unsupported,
            "placements whose archetype has no definition in the archives, left out",
            missing_archetypes as u64,
        ));
    }
    out
}

/// What loading couldn't find or couldn't give a core meaning: prototypes without a live drawable file (left without
/// geometry), textures the archives don't hold (each named, as the example, with how many bindings asked for it),
/// streams storing values that are not finite, and primitives whose vertex blend streams stay `gta.blend_streams`.
fn load_notices(
    missing_assets: usize,
    textures_not_found: &BTreeMap<String, usize>,
    non_finite_streams: &BTreeMap<String, usize>,
    blend_streams: usize,
) -> Vec<Notice> {
    let mut out = Vec::new();
    if missing_assets > 0 {
        out.push(Notice::new(
            STAGE,
            NoticeKind::Unsupported,
            "prototypes without a live drawable file, left without geometry",
            missing_assets as u64,
        ));
    }
    if !textures_not_found.is_empty() {
        let mut n = Notice::new(
            STAGE,
            NoticeKind::Unsupported,
            "textures not found in the archives, their bindings left unbound",
            textures_not_found.len() as u64,
        );
        for (name, uses) in textures_not_found {
            n = n.example(format!("{name} ({uses} bindings)"));
        }
        out.push(n);
    }
    if !non_finite_streams.is_empty() {
        let mut n = Notice::new(
            STAGE,
            NoticeKind::Unsupported,
            "vertex streams storing values that are not finite (NaN, infinity), left out (one per primitive)",
            non_finite_streams.values().sum::<usize>() as u64,
        );
        for (semantic, primitives) in non_finite_streams {
            n = n.example(format!("{semantic} ({primitives} primitives)"));
        }
        out.push(n);
    }
    if blend_streams > 0 {
        out.push(Notice::new(
            STAGE,
            NoticeKind::Unsupported,
            "primitives whose vertex blend weights, indices and bone palette are kept as gta.blend_streams, not \
             core joints and weights (a core skin needs a skeleton and its binding, which a pass makes)",
            blend_streams as u64,
        ));
    }
    out
}

impl std::fmt::Debug for GtaReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaReader")
            .field("region", &self.region)
            .field("reach", &self.reach)
            .finish_non_exhaustive()
    }
}

impl GtaReader {
    /// Opens the install.
    pub fn open(o: &GtaOptions) -> Result<GtaReader> {
        Ok(GtaReader {
            source: GtaSource::open(&o.install, &o.keys)?,
            region: o.region,
            reach: o.reach,
            pending: Vec::new(),
        })
    }
}

impl Reader for GtaReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let (w, r) = self.source.read(self.region, self.reach)?;
        self.pending.extend(read_notices(r.region.missing_archetypes));
        Ok((
            w,
            vec![
                ("gta region".into(), format!("{:?}", r.region)),
                ("gta lights".into(), format!("{:?}", r.lights)),
                ("gta bounds".into(), format!("{:?}", r.bounds)),
                (
                    "gta water (each file: water, calming, wave quads)".into(),
                    format!("{:?}", r.water),
                ),
            ],
        ))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        let (w, r) = self.source.load(world, roots)?;
        self.pending.extend(load_notices(
            r.asset_missing,
            &r.textures_not_found,
            &r.non_finite_streams,
            r.blend_streams,
        ));
        let mut report = Report::new();
        if !r.textures_not_found.is_empty() || r.asset_missing > 0 || !r.non_finite_streams.is_empty() {
            report.push((
                "gta assets".into(),
                format!(
                    "{} prototypes without a live file, textures not found {:?}, non-finite vertex streams left out {:?}",
                    r.asset_missing, r.textures_not_found, r.non_finite_streams
                ),
            ));
        }
        Ok((w, report))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "gta"]
    }
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        prototype_box(world, prototype)
    }
    /// What the last read or load couldn't find (each taken once).
    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Missing source inputs, vertex streams left out for values that are not finite, and blend streams kept in the
    /// dialect become notices with their counts and the names the source has; nothing missing or left out, none.
    #[test]
    fn missing_inputs_are_notices() {
        assert!(read_notices(0).is_empty());
        assert!(load_notices(0, &BTreeMap::new(), &BTreeMap::new(), 0).is_empty());
        let r = read_notices(15);
        assert_eq!((r.len(), r[0].count, r[0].kind), (1, 15, NoticeKind::Unsupported));
        let textures = BTreeMap::from([("road_01".to_owned(), 3), ("sign_02".to_owned(), 1)]);
        let l = load_notices(2, &textures, &BTreeMap::new(), 0);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].count, 2);
        assert!(l[0].reason.contains("drawable"));
        assert_eq!(l[1].count, 2);
        assert_eq!(l[1].examples, ["road_01 (3 bindings)", "sign_02 (1 bindings)"]);
        // a NaN normal stream on one primitive, a tangent stream on two: one notice, counted per primitive
        let streams = BTreeMap::from([("Normal".to_owned(), 1), ("Tangent".to_owned(), 2)]);
        let n = load_notices(0, &BTreeMap::new(), &streams, 0);
        assert_eq!((n.len(), n[0].count, n[0].kind), (1, 3, NoticeKind::Unsupported));
        assert!(n[0].reason.contains("not finite"));
        assert_eq!(n[0].examples, ["Normal (1 primitives)", "Tangent (2 primitives)"]);
        // blend streams kept in the dialect, their palette unread: counted per primitive
        let b = load_notices(0, &BTreeMap::new(), &BTreeMap::new(), 4);
        assert_eq!((b.len(), b[0].count, b[0].kind), (1, 4, NoticeKind::Unsupported));
        assert!(b[0].reason.contains("gta.blend_streams"));
    }
}
