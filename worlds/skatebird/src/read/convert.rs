//! A SkateBIRD level as a conversion's reader ([`SkatebirdReader`]): the Unity reader's read of the scene
//! ([`primase_unity::convert::UnityReader`]) and, into the same world, the game's own scripts' facts: its reset points and
//! mission givers ([`super::read_markers`]), its `Bumped Specular Voronoi` programs ([`super::read_voronoi_programs`])
//! and what its scripts say of the scene's objects and grind rails ([`super::read_scripted`]). Everything is read at
//! once, geometry and textures included; a part is the subset of that world its roots reach. What the reads could not
//! put into the world as stored is reported as notices.

use std::collections::BTreeSet;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::{Entity, World};
use primase_unity::convert::{UnityReader, mesh_box};
use primase_unity::player::Player;

/// Scene `level` (`level1`) of `player` (see the module documentation).
pub struct SkatebirdReader<'a> {
    unity: UnityReader<'a>,
    pending: Vec<Notice>,
}

impl std::fmt::Debug for SkatebirdReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkatebirdReader")
            .field("level", &self.unity.level)
            .finish_non_exhaustive()
    }
}

impl<'a> SkatebirdReader<'a> {
    /// The reader of scene `level` of `player`.
    pub fn new(player: &'a Player, level: impl Into<String>) -> SkatebirdReader<'a> {
        SkatebirdReader {
            unity: UnityReader::new(player, level),
            pending: Vec::new(),
        }
    }

    fn notice(&mut self, kind: NoticeKind, reason: &str, count: u64) {
        if count > 0 {
            self.pending
                .push(Notice::new("skatebird reader", kind, reason, count));
        }
    }
}

impl Reader for SkatebirdReader<'_> {
    fn read(&mut self) -> Result<(World, Report)> {
        // the Unity scene as the Unity reader reads it (its notices are its own), then the game's scripts on it
        let (mut w, _, mut report) = self.unity.read_world()?;
        let (player, level) = (self.unity.player, self.unity.level.clone());
        let markers = super::read_markers(player, &level, &mut w)?;
        let programs = super::read_voronoi_programs(player, &mut w)?;
        let scripted = super::read_scripted(player, &level, &mut w)?;
        for (why, n) in &programs.left_out {
            self.notice(
                NoticeKind::Unsupported,
                &format!("Voronoi materials without their program: {why}"),
                *n as u64,
            );
        }
        self.notice(
            NoticeKind::Unsupported,
            "reset points and mission givers on a GameObject without a Transform, left out",
            markers.without_transform as u64,
        );
        self.notice(
            NoticeKind::Unsupported,
            "grind rail nodes alone in their chain, left out",
            scripted.lone_rail_nodes as u64,
        );
        self.notice(
            NoticeKind::Unsupported,
            "grind rail nodes on a GameObject without a Transform, left out",
            scripted.rail_nodes_without_transform as u64,
        );
        report.extend([
            ("skatebird markers".into(), format!("{markers:?}")),
            ("skatebird Voronoi programs".into(), format!("{programs:?}")),
            ("skatebird scripted".into(), format!("{scripted:?}")),
        ]);
        Ok((w, report))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        self.unity.load(world, roots)
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "unity", "skatebird"]
    }
    /// Its mesh's box: geometry is read with the level.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        mesh_box(world, prototype)
    }
    fn notices(&mut self) -> Vec<Notice> {
        let mut all = self.unity.notices();
        all.append(&mut self.pending);
        all
    }
}
