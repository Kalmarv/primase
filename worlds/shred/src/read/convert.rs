//! Conversion adapter; the caller owns archive selection, the source-frame assertion and every optional input.
use super::{
    collision::{self, CollisionReport},
    level::{self, LevelReport},
    material_join::{MaterialInputs, MaterialLibrary, MaterialReport},
};
use crate::components::{SessionGraph, Stored};
use primase_core::{
    Entity, Mesh, World,
    convert::{self, Reader, Report},
    frame::SourceFrame,
};
use rme_formats::{
    level_params::{self, ParamsReport, ScriptInputs},
    pak::Pak,
    surfaces::SurfaceTable,
};
use std::collections::BTreeSet;

/// Reads the whole of the level that exactly the caller's PAKs hold: placements, prototypes with their meshes and
/// material entities, spot chunks and, when the caller gives them, the MAIN.DB surface table (collision) and the
/// material banks (source materials and the texture copies they bind) and the level's parameter script. Geometry is read with the level, so a
/// lowering that runs on the read world sees it and `load` keeps what the selected world holds; only texture pixels
/// wait for `load`.
pub struct ShredReader<'a> {
    paks: Vec<(&'a str, &'a Pak)>,
    frame: SourceFrame,
    surfaces: Option<SurfaceTable>,
    material_inputs: Option<MaterialInputs<'a>>,
    scripts: Option<ScriptInputs<'a>>,
    material_library: Option<MaterialLibrary<'a>>,
    sessions: Option<SessionGraph>,
    pub report: LevelReport,
    pub collision: CollisionReport,
    pub material_report: MaterialReport,
    pub params: ParamsReport,
}

impl std::fmt::Debug for ShredReader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShredReader")
            .field("frame", &self.frame)
            .field("report", &self.report)
            .field("collision", &self.collision)
            .finish_non_exhaustive()
    }
}

impl<'a> ShredReader<'a> {
    pub fn new(paks: Vec<(&'a str, &'a Pak)>, frame: SourceFrame) -> Self {
        Self {
            paks,
            frame,
            surfaces: None,
            material_inputs: None,
            scripts: None,
            material_library: None,
            sessions: None,
            report: LevelReport::default(),
            collision: CollisionReport::default(),
            material_report: MaterialReport::default(),
            params: ParamsReport::default(),
        }
    }

    /// Also read the `.hkw` collision of the PAKs (`read::collision::read_collision`) with MAIN.DB's surface table.
    /// Without a table the world has no collision: surface ids have no names to be indices of.
    pub fn with_collision(mut self, surfaces: SurfaceTable) -> Self {
        self.surfaces = Some(surfaces);
        self
    }

    pub fn with_materials(mut self, inputs: MaterialInputs<'a>) -> Self {
        self.material_inputs = Some(inputs);
        self
    }

    /// Also read the level's `vfx_level_load_<level>` script from the (decrypted) scripts archive as the
    /// `shred.level_params` resource.
    pub fn with_scripts(mut self, inputs: ScriptInputs<'a>) -> Self {
        self.scripts = Some(inputs);
        self
    }

    /// Record the caller's MAIN.DB graph. Activating a session is a separate pass.
    pub fn with_sessions(mut self, graph: SessionGraph) -> Self {
        self.sessions = Some(graph);
        self
    }
}

impl Reader for ShredReader<'_> {
    fn read(&mut self) -> convert::Result<(World, Report)> {
        let (mut world, report) = level::read_level(&self.paks, self.frame)?;
        self.report = report;
        let mut lines = self.report.lines();
        if let Some(graph) = &self.sessions {
            world.insert_resource(graph.clone());
            lines.push((
                "shred sessions".into(),
                format!(
                    "level {}: {} session / {} goal / {} task rows; graph recorded, no session selected",
                    graph.level_id,
                    graph.sessions.len(),
                    graph.goals.len(),
                    graph.tasks.len()
                ),
            ));
        } else {
            lines.push((
                "shred sessions".into(),
                "MAIN.DB session graph not supplied; all placements retained".into(),
            ));
        }
        if let Some(surfaces) = &self.surfaces {
            self.collision = collision::read_collision(&mut world, &self.paks, surfaces, self.frame)?;
            lines.extend(self.collision.lines());
        }
        if let Some(inputs) = &self.scripts {
            let (params, report) = level_params::read_level_params(inputs).map_err(crate::Error::from)?;
            self.params = report;
            lines.extend(params_lines(&self.params));
            if let Some(params) = params {
                world.insert_resource(Stored(params));
            }
        }
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
        Ok((world, lines))
    }
    /// The part of `world` the roots reach, with the pixels of the texture copies it holds decoded: the meshes,
    /// materials and collision are in it as the selected world has them, so a selection or lowering that changed
    /// them is what comes back.
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> convert::Result<(World, Report)> {
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let lines = self.decode(&mut part)?;
        Ok((part, lines))
    }
    fn load_all(&mut self, mut world: World) -> convert::Result<(World, Report)> {
        let lines = self.decode(&mut world)?;
        Ok((world, lines))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "shred"]
    }
    fn prototype_box(&self, world: &World, e: Entity) -> Option<[[f64; 3]; 2]> {
        let mesh = world.get::<Mesh>(e)?;
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

impl ShredReader<'_> {
    /// Decode the texture copies a world holds faces of (none without material inputs).
    fn decode(&self, world: &mut World) -> convert::Result<Report> {
        match &self.material_library {
            Some(library) => Ok(library.load(world)?),
            None => Ok(vec![]),
        }
    }
}

/// The log lines of a level-script read.
fn params_lines(report: &ParamsReport) -> Report {
    let text = match (&report.script, report.branching) {
        (None, _) => format!("no vfx_level_load script for level {}", report.level),
        (Some(s), true) => {
            format!("{s} has control flow or nested functions; none of its calls are read")
        }
        (Some(s), false) => {
            let mut t = format!(
                "{s}: {} calls read as data, {} not read",
                report.read,
                report.unread.len()
            );
            for (pc, why) in &report.unread {
                t.push_str(&format!("; instruction {pc}: {why}"));
            }
            t
        }
    };
    vec![("shred level params".into(), text)]
}
