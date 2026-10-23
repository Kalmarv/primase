//! A Unity scene as a conversion's reader ([`UnityReader`]): the level read ([`crate::level::read_level`]) as the world
//! to plan from, and a part as the subset of that world its roots reach. Everything is read at once, geometry and
//! textures included (the level read is one pass over the scene), so loading a part decodes nothing more: it selects.
//! What the read could not put into the world as stored is reported as notices, and the level report as a labelled
//! line ([`LevelReport::to_json`] for a consumer that wants it as data).

use std::collections::BTreeSet;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::{Entity, Mesh, World};

use crate::level::{LevelOptions, LevelReport, read_level_with};
use crate::player::Player;

/// The stage notices name.
const STAGE: &str = "unity reader";

/// Scene `level` (`level1`) of `player` (see the module documentation).
pub struct UnityReader<'a> {
    pub player: &'a Player,
    pub level: String,
    options: LevelOptions,
    pending: Vec<Notice>,
}

impl std::fmt::Debug for UnityReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnityReader")
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

impl<'a> UnityReader<'a> {
    /// The reader of scene `level` of `player`.
    pub fn new(player: &'a Player, level: impl Into<String>) -> UnityReader<'a> {
        UnityReader {
            player,
            level: level.into(),
            options: LevelOptions::default(),
            pending: Vec::new(),
        }
    }

    /// The reader told the quality level whose skin weights limit the skinned meshes ([`LevelOptions::quality`]).
    #[must_use]
    pub fn with_quality(mut self, quality: impl Into<String>) -> UnityReader<'a> {
        self.options.quality = Some(quality.into());
        self
    }

    /// Read the scene into a new world: the world, the level report (as [`read_level`] gives it) and the report lines
    /// [`Reader::read`] returns. The notices are kept for [`Reader::notices`].
    pub fn read_world(&mut self) -> Result<(World, LevelReport, Report)> {
        let mut w = World::new();
        let level = read_level_with(self.player, &self.level, &self.options, &mut w)?;
        self.notices_of(&level);
        let report = vec![("unity level".into(), format!("{}: {level:?}", self.level))];
        Ok((w, level, report))
    }

    fn notices_of(&mut self, level: &LevelReport) {
        let mut add = |kind: NoticeKind, what: &str, reasons: Vec<(String, u64)>| {
            for (reason, n) in reasons {
                if n > 0 {
                    self.pending
                        .push(Notice::new(STAGE, kind, format!("{what}: {reason}"), n));
                }
            }
        };
        let own =
            |m: &std::collections::BTreeMap<String, u64>| m.iter().map(|(k, n)| (k.clone(), *n)).collect();
        add(
            NoticeKind::Unsupported,
            "textures not decoded",
            own(&level.textures_not_decoded),
        );
        add(
            NoticeKind::Unsupported,
            "not reproduced",
            own(&level.not_reproduced),
        );
        add(NoticeKind::Approximation, "read otherwise", own(&level.notices));
        add(
            NoticeKind::Unsupported,
            "colliders not colliding",
            level
                .colliders_not_colliding
                .iter()
                .map(|(k, n)| ((*k).to_owned(), *n))
                .collect(),
        );
    }

    /// Add a notice of the reader's own (a stage built on this one, as SkateBIRD's, adds its own).
    pub fn notice(&mut self, kind: NoticeKind, reason: &str, count: u64) {
        if count > 0 {
            self.pending.push(Notice::new(STAGE, kind, reason, count));
        }
    }
}

impl Reader for UnityReader<'_> {
    fn read(&mut self) -> Result<(World, Report)> {
        let (w, _, report) = self.read_world()?;
        Ok((w, report))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "unity"]
    }
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        mesh_box(world, prototype)
    }
    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
}

/// A prototype's mesh box in its own frame: geometry is read with the level, so the box is the mesh's.
pub fn mesh_box(world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
    let mesh = world.get::<Mesh>(prototype)?;
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in mesh.primitives() {
        for v in p.positions().into_iter().flatten() {
            for i in 0..3 {
                b[0][i] = b[0][i].min(f64::from(v[i]));
                b[1][i] = b[1][i].max(f64::from(v[i]));
            }
        }
    }
    (b[0][0] <= b[1][0]).then_some(b)
}
