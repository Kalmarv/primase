//! Conversion adapter; the caller owns archive selection and the source-frame assertion.
use super::{
    level::{self, LevelReport},
    material_join::{MaterialInputs, MaterialLibrary, MaterialReport},
    pak::Pak,
};
use crate::components::{ModelDependencies, VertexInputs};
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
    material_inputs: Option<MaterialInputs<'a>>,
    material_library: Option<MaterialLibrary<'a>>,
    pub report: LevelReport,
    pub material_report: MaterialReport,
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
            material_inputs: None,
            material_library: None,
            report: LevelReport::default(),
            material_report: MaterialReport::default(),
        }
    }

    pub fn with_materials(mut self, inputs: MaterialInputs<'a>) -> Self {
        self.material_inputs = Some(inputs);
        self
    }
}

impl Reader for ShredReader<'_> {
    fn read(&mut self) -> convert::Result<(World, Report)> {
        let (mut world, report) = level::read_level(&self.paks, self.frame)?;
        self.report = report;
        let mut lines = self.report.lines();
        if let Some(inputs) = &self.material_inputs {
            let mut assets = self.paks.clone();
            assets.extend(inputs.assets.iter().copied());
            let mut library = MaterialLibrary::new(inputs.banks, &assets)?;
            self.material_report = library.attach(&mut world, inputs.textures)?;
            lines.extend(self.material_report.lines());
            self.material_library = Some(library);
        } else {
            lines.push((
                "shred materials".into(),
                "material inputs not supplied; geometry identities only".into(),
            ));
        }
        // Keep material references in the planning world: closure must reach them before geometry loads.
        let mut planning = world.clone();
        let prototypes = planning.entities_named("core.mesh");
        for e in prototypes {
            let mesh = planning.remove::<Mesh>(e).expect("mesh entity");
            planning.remove::<VertexInputs>(e);
            let materials: BTreeSet<_> = mesh.primitives().iter().filter_map(|p| p.material).collect();
            planning.insert(e, ModelDependencies(materials.into_iter().collect()));
        }
        self.loaded = Some(world);
        Ok((planning, lines))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> convert::Result<(World, Report)> {
        let loaded = self.loaded.as_ref().ok_or("SHRED reader must read before load")?;
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let entities = part.entities_named("shred.model_dependencies");
        for e in entities {
            if let Some(mesh) = loaded.get::<Mesh>(e) {
                part.insert(e, mesh.clone());
                if let Some(inputs) = loaded.get::<VertexInputs>(e) {
                    part.insert(e, inputs.clone());
                }
            }
        }
        let lines = if let Some(library) = &self.material_library {
            library.load(&mut part)?
        } else {
            vec![]
        };
        Ok((part, lines))
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
