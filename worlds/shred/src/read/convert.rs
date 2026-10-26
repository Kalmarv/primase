//! Conversion adapter; the caller owns archive selection and the source-frame assertion.
use super::{
    level::{self, LevelReport},
    pak::Pak,
};
use crate::components::ModelDependencies;
use primase_core::{
    Entity, Mesh, World,
    convert::{self, Reader, Report},
    frame::SourceFrame,
};
use std::collections::BTreeSet;

pub struct ShredReader<'a> {
    paks: Vec<(&'a str, &'a Pak)>,
    frame: SourceFrame,
    loaded: Option<World>,
    pub report: LevelReport,
}

impl std::fmt::Debug for ShredReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShredReader")
            .field("frame", &self.frame)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl<'a> ShredReader<'a> {
    pub fn new(paks: Vec<(&'a str, &'a Pak)>, frame: SourceFrame) -> Self {
        Self {
            paks,
            frame,
            loaded: None,
            report: LevelReport::default(),
        }
    }
}

impl Reader for ShredReader<'_> {
    fn read(&mut self) -> convert::Result<(World, Report)> {
        let (world, report) = level::read_level(&self.paks, self.frame)?;
        self.report = report;
        // Keep material references in the planning world: closure must reach them before geometry loads.
        let mut planning = world.clone();
        let prototypes = planning.entities_named("core.mesh");
        for e in prototypes {
            let mesh = planning.remove::<Mesh>(e).expect("mesh entity");
            let materials: BTreeSet<_> = mesh.primitives().iter().filter_map(|p| p.material).collect();
            planning.insert(e, ModelDependencies(materials.into_iter().collect()));
        }
        self.loaded = Some(world);
        Ok((planning, self.report.lines()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> convert::Result<(World, Report)> {
        let loaded = self.loaded.as_ref().ok_or("SHRED reader must read before load")?;
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let entities = part.entities_named("shred.model_dependencies");
        for e in entities {
            if let Some(mesh) = loaded.get::<Mesh>(e) {
                part.insert(e, mesh.clone());
            }
        }
        Ok((part, vec![]))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "shred"]
    }
    fn prototype_box(&self, _: &World, e: Entity) -> Option<[[f64; 3]; 2]> {
        let mesh = self.loaded.as_ref()?.get::<Mesh>(e)?;
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        let mut any = false;
        for p in mesh.primitives() {
            for v in p.positions()? {
                any = true;
                for k in 0..3 {
                    bounds[0][k] = bounds[0][k].min(v[k] as f64);
                    bounds[1][k] = bounds[1][k].max(v[k] as f64);
                }
            }
        }
        any.then_some(bounds)
    }
}
