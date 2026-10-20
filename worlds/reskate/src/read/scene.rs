//! A skate-scene interchange folder as a source of the convert loop ([`SceneReader`]).
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::{Entity, MeshFile, World};

/// Every component and resource a skate scene's read and load put in a world, by name (the names in the namespaces
/// [`Reader::emits`] states): what a coverage report lists of this reader. The reader's honesty test reads a scene that
/// holds each kind and checks that its world holds no other name.
pub const EMITTED_NAMES: &[&str] = &[
    "core.collision",
    "core.curve",
    "core.instance",
    "core.layered",
    "core.lods",
    "core.marker",
    "core.material_variants",
    "core.mesh",
    "core.mesh_file",
    "core.name",
    "core.priority",
    "core.source",
    "core.surfaces",
    "core.texture",
    "core.texture_file",
    "core.tile",
    "core.transform",
    "skate.collision",
    "skate.form",
    "skate.grind_curve",
    "skate.layered_syntax",
    "skate.light",
    "skate.loaded_file",
    "skate.marker",
    "skate.material",
    "skate.parts",
    "skate.pause_image",
    "skate.placement",
    "skate.prototype",
    "skate.rock",
    "skate.scene",
    "skate.texture",
    "skate.water",
];

/// A skate-scene folder read for a conversion: [`super::interchange::read`] gives the world to plan from (its GLBs and
/// textures as file references), and each part's closure is loaded with [`super::interchange::load`] (the meshes, the
/// textures and the pause-map picture its entities reference). Its components are `core` and the `skate` dialect; a
/// prototype's box before loading is its GLB's, from the file's header ([`primase_glb::mesh_file_box`]).
#[derive(Debug)]

pub struct SceneReader {
    dir: PathBuf,
    /// What it noticed since it was last asked: a prototype whose GLB header could not be read for its box.
    noticed: RefCell<Vec<Notice>>,
}

impl SceneReader {
    /// The scene folder `dir` (holding `scene.json`).
    pub fn new(dir: impl Into<PathBuf>) -> SceneReader {
        SceneReader {
            dir: dir.into(),
            noticed: RefCell::new(Vec::new()),
        }
    }
}

impl Reader for SceneReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut world = World::new();
        let r = super::interchange::read(&self.dir, &mut world)?;
        let mut report = vec![(
            "skate scene".to_owned(),
            format!("{}: {} warnings", self.dir.display(), r.warnings.len()),
        )];
        report.extend(
            r.warnings
                .into_iter()
                .map(|w| ("skate scene warning".to_owned(), w)),
        );
        report.extend(
            r.tolerated
                .into_iter()
                .map(|(what, n)| ("skate scene tolerated".to_owned(), format!("{what} x {n}"))),
        );
        Ok((world, report))
    }

    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let r = super::interchange::load(&self.dir, &mut part)?;
        let report = vec![(
            "skate scene load".to_owned(),
            format!(
                "{} meshes from {} files, {} textures from {} files{}",
                r.meshes.meshes,
                r.meshes.files,
                r.textures.textures,
                r.textures.files,
                if r.pause_image {
                    ", the pause-map picture"
                } else {
                    ""
                }
            ),
        )];
        Ok((part, report))
    }

    fn emits(&self) -> &'static [&'static str] {
        &["core", "skate"]
    }

    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let file = world.get::<MeshFile>(prototype)?;
        match primase_glb::mesh_file_box(&self.dir, file) {
            Ok(b) => b,
            Err(e) => {
                self.noticed.borrow_mut().push(Notice::new(
                    "skate-scene",
                    NoticeKind::Unsupported,
                    format!("prototype GLB headers not read for their box, planned without one: {e}"),
                    1,
                ));
                None
            }
        }
    }

    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.noticed.borrow_mut())
    }
}
