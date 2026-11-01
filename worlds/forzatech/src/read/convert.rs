//! The `convert::Reader` over a track: `read` makes the placements and prototypes of the caller's box and season
//! ([`super::placements`]), each prototype's levels kept as `forzatech.levels`; `load` gives the forms a part reaches
//! their meshes ([`super::models`]), makes `core.lods` of the levels once they have meshes ([`models::make_lods`]),
//! and gives the materials their chains and textures ([`super::materials`]). So a level of detail is selected after
//! loading. Both scope choices are the caller's, with no default.
use super::materials::{Library, MaterialReport};
use super::models::{self, Materials, ModelReport};
use super::open::Track;
use super::placements::{self, PlacementReport, Scope};
use crate::FRAME;
use crate::components::ModelFiles;
use forzatech_formats::world::{Area, Season};
use primase_core::convert::{self, Reader, Report};
use primase_core::pass::{OptionSpec, Options};
use primase_core::read_scope::{ReadScope, ReadScopeSpec};
use primase_core::{Entity, World};
use std::collections::BTreeSet;

/// The read scope: the box (the game's own x and z, metres: `x0:z0:x1:z1`, origins in `[x0, x1) × [z0, z1)`) and the
/// season.
pub const READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "forzatech-area",
    options: &[
        OptionSpec::required(
            "area",
            "-1550:-5000:-1100:-4566.67",
            "x0:z0:x1:z1 in the game's metres (x east, z north): placements whose origin is in [x0, x1) x [z0, z1); \
             a hand-placed instance by its first model bound's centre",
        ),
        OptionSpec::required(
            "season",
            "summer",
            "spring, summer, autumn or winter: the placements, templates and materials it shows",
        ),
    ],
};

/// Reads one box of one track in one season.
pub struct ForzaReader {
    track: Track,
    scope: Scope,
    pub placements: PlacementReport,
    pub models: ModelReport,
    pub materials: MaterialReport,
    /// The track's materials and shaders, opened on the first load.
    library: Option<Library>,
}

impl std::fmt::Debug for ForzaReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForzaReader")
            .field("track", &self.track)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl ForzaReader {
    pub fn new(track: Track, scope: Scope) -> Self {
        Self {
            track,
            scope,
            placements: PlacementReport::default(),
            models: ModelReport::default(),
            materials: MaterialReport::default(),
            library: None,
        }
    }

    /// The scope from the caller's options, checked against [`READ_SCOPE`].
    pub fn scope(options: &Options) -> Result<Scope, String> {
        READ_SCOPE.check(options)?;
        let area = options.get("area").ok_or("area is required")?;
        let v: Vec<f64> = area
            .split(':')
            .map(|n| {
                n.parse::<f64>()
                    .map_err(|_| format!("area {area:?}: {n:?} is not a number"))
            })
            .collect::<Result<_, _>>()?;
        let [x0, z0, x1, z1] = v[..] else {
            return Err(format!("area {area:?}: needs x0:z0:x1:z1"));
        };
        if !(v.iter().all(|x| x.is_finite()) && x0 < x1 && z0 < z1) {
            return Err(format!("area {area:?}: needs finite x0 < x1 and z0 < z1"));
        }
        let season = match options.get("season").map(String::as_str) {
            Some("spring") => Season::Spring,
            Some("summer") => Season::Summer,
            Some("autumn") => Season::Autumn,
            Some("winter") => Season::Winter,
            other => {
                return Err(format!(
                    "season {other:?}: needs spring, summer, autumn or winter"
                ));
            }
        };
        Ok(Scope {
            area: Area::new(x0, z0, x1, z1),
            season,
        })
    }

    fn options(&self) -> Options {
        let a = &self.scope.area;
        Options::from([
            (
                "area".into(),
                format!("{}:{}:{}:{}", a.min_x, a.min_z, a.max_x, a.max_z),
            ),
            ("season".into(), self.scope.season.folder().into()),
        ])
    }

    fn load_meshes(&mut self, world: &mut World, entities: Vec<Entity>) -> convert::Result<Report> {
        let library = match &mut self.library {
            Some(l) => l,
            slot @ None => slot.insert(Library::open(&self.track)?),
        };
        let mut materials = Materials::new(library);
        let mut report = ModelReport::default();
        for e in entities {
            models::load_mesh(
                &self.track,
                self.scope.season,
                world,
                e,
                &mut materials,
                &mut report,
            )?;
        }
        models::make_lods(world, &mut report);
        let mut lines = report.lines();
        lines.extend(materials.report.lines());
        self.materials.add(&materials.report);
        self.models.files += report.files;
        self.models.meshes += report.meshes;
        self.models.triangles += report.triangles;
        self.models.other_level += report.other_level;
        self.models.empty_forms += report.empty_forms;
        self.models.promoted += report.promoted;
        self.models.default_lost += report.default_lost;
        Ok(lines)
    }
}

impl Reader for ForzaReader {
    fn scope_spec(&self) -> Option<&'static ReadScopeSpec> {
        Some(&READ_SCOPE)
    }
    fn read_scope(&self) -> Option<ReadScope> {
        READ_SCOPE.resolve(self.options()).ok()
    }
    fn read(&mut self) -> convert::Result<(World, Report)> {
        let mut world = World::new();
        self.placements = placements::read_placements(&self.track, &self.scope, &mut world)?;
        Ok((world, self.placements.lines()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> convert::Result<(World, Report)> {
        let mut part = world.subset(&world.closure(roots.iter().copied()));
        let entities: Vec<Entity> = part.iter::<ModelFiles>().map(|(e, _)| e).collect();
        let lines = self.load_meshes(&mut part, entities)?;
        Ok((part, lines))
    }
    fn load_all(&mut self, mut world: World) -> convert::Result<(World, Report)> {
        let entities: Vec<Entity> = world.iter::<ModelFiles>().map(|(e, _)| e).collect();
        let lines = self.load_meshes(&mut world, entities)?;
        Ok((world, lines))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "forzatech"]
    }
    /// The prototype's own files' `Modl` boxes, joined and converted to the world's frame.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let files = world.get::<ModelFiles>(prototype)?;
        let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for f in &files.files {
            for corner in [FRAME.point(f.min), FRAME.point(f.max)] {
                for k in 0..3 {
                    b[0][k] = b[0][k].min(f64::from(corner[k]));
                    b[1][k] = b[1][k].max(f64::from(corner[k]));
                }
            }
        }
        b[0][0].is_finite().then_some(b)
    }
}
