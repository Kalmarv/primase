//! A SkateBIRD level as a conversion's reader ([`SkatebirdReader`]): the Unity level read
//! ([`primase_unity::level::read_level`]) and, into the same world, the game's own scripts' facts: its reset points and
//! mission givers ([`super::read_markers`]), its `Bumped Specular Voronoi` programs ([`super::read_voronoi_programs`])
//! and what its scripts say of the scene's objects and grind rails ([`super::read_scripted`]). Everything is read at
//! once, geometry and textures included; a part is the subset of that world its roots reach. What the reads could not
//! put into the world as stored is reported as notices.

use std::collections::BTreeSet;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::{Entity, Mesh, World};
use primase_unity::level::read_level;
use primase_unity::player::Player;

/// The stage notices name.
const STAGE: &str = "skatebird reader";

/// Scene `level` (`level1`) of `player` (see the module documentation).
pub struct SkatebirdReader<'a> {
    pub player: &'a Player,
    pub level: String,
    pending: Vec<Notice>,
}

impl std::fmt::Debug for SkatebirdReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkatebirdReader")
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

impl<'a> SkatebirdReader<'a> {
    /// The reader of scene `level` of `player`.
    pub fn new(player: &'a Player, level: impl Into<String>) -> SkatebirdReader<'a> {
        SkatebirdReader {
            player,
            level: level.into(),
            pending: Vec::new(),
        }
    }

    fn notice(&mut self, kind: NoticeKind, reason: &str, count: u64) {
        if count > 0 {
            self.pending.push(Notice::new(STAGE, kind, reason, count));
        }
    }
}

impl Reader for SkatebirdReader<'_> {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut w = World::new();
        let level = read_level(self.player, &self.level, &mut w)?;
        let markers = super::read_markers(self.player, &self.level, &mut w)?;
        let programs = super::read_voronoi_programs(self.player, &mut w)?;
        let scripted = super::read_scripted(self.player, &self.level, &mut w)?;
        for (what, n) in &level.textures_not_decoded {
            self.notice(
                NoticeKind::Unsupported,
                &format!("textures not decoded ({what})"),
                *n,
            );
        }
        for (what, n) in &level.not_reproduced {
            self.notice(NoticeKind::Unsupported, &format!("not reproduced: {what}"), *n);
        }
        for (what, n) in &level.notices {
            self.notice(NoticeKind::Approximation, &format!("read otherwise: {what}"), *n);
        }
        for (what, n) in &level.colliders_not_colliding {
            self.notice(
                NoticeKind::Unsupported,
                &format!("colliders not colliding: {what}"),
                *n,
            );
        }
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
        let report = vec![
            ("skatebird level".into(), format!("{}: {level:?}", self.level)),
            ("skatebird markers".into(), format!("{markers:?}")),
            ("skatebird Voronoi programs".into(), format!("{programs:?}")),
            ("skatebird scripted".into(), format!("{scripted:?}")),
        ];
        Ok((w, report))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "unity", "skatebird"]
    }
    /// Its mesh's box: geometry is read with the level.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
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
    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
}
