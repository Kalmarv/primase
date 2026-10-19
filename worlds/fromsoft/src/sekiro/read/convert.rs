//! A Sekiro area through the convert loop ([`primase_core::convert`]): [`AreaReader`] reads an area of an install
//! with [`super::area::read_area`] and gives each part the closure of its roots in that world.
//!
//! [`super::area::read_area`] decodes every model, texture and collision file it keeps while it reads (the archive's files
//! are compressed whole), so the read world already holds the payloads: a part's load is its roots' closure, nothing
//! read again. A prototype's box before any pass is its stored model's posed box in the world frame, the extent the
//! read's region rule uses.
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::convert::{Reader, Report, Result};
use primase_core::{Entity, World};

use super::area::{Options, read_area};
use crate::components::models::SourceModel;
use crate::dcx::Decoder;

/// See the module documentation.
#[derive(Debug)]
pub struct AreaReader<'a> {
    install: PathBuf,
    area: String,
    decoder: Decoder<'a>,
    options: Options,
}

impl<'a> AreaReader<'a> {
    /// The area `area` (`m11_01` or `m11_01_00_00`) of the install at `install`, read as `options` says, archives
    /// decoded with `decoder`.
    pub fn new(
        install: impl Into<PathBuf>,
        area: &str,
        decoder: Decoder<'a>,
        options: Options,
    ) -> AreaReader<'a> {
        AreaReader {
            install: install.into(),
            area: area.to_owned(),
            decoder,
            options,
        }
    }
}

impl Reader for AreaReader<'_> {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut world = World::new();
        let r = read_area(
            &self.install,
            &self.area,
            &self.decoder,
            &self.options,
            &mut world,
        )?;
        let mut report = Report::new();
        report.push((
            "sekiro area".into(),
            format!(
                "{}: {} placements of {} prototypes ({} distant-backdrop prototypes with {} placements), {} materials, \
                 {} textures, {} layered-shader programs, models missing {:?}",
                r.area,
                r.placements,
                r.prototypes,
                r.far_prototypes,
                r.far_placements,
                r.materials,
                r.textures,
                r.shader_programs,
                r.missing_models
            ),
        ));
        report.push(("sekiro collision".into(), format!("{:?}", r.collision)));
        report.push((
            "sekiro object collision".into(),
            format!("{:?}", r.object_collision),
        ));
        report.push(("sekiro extras".into(), format!("{:?}", r.extras)));
        if let Some(states) = &r.states {
            report.push(("sekiro states".into(), format!("{states:?}")));
        }
        Ok((world, report))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "fromsoft"]
    }
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let model = &world.get::<SourceModel>(prototype)?.0;
        super::area::world_bounds(model)
            .ok()
            .flatten()
            .map(|b| b.map(|c| c.map(f64::from)))
    }
}
