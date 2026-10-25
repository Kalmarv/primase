//! Ambient light as fill lights (pass `fill-ambient-lights`): a renderer with no ambient of its own, only punctual
//! lights, gets a grid of shadowless point lights in its place, each bright enough that a surface between them sees
//! about the ambient's level.
//!
//! The ambient is `core.environment`'s ([`primase_core::Ambient`]), reduced to one colour, the mean of its colours
//! times its intensity (a gradient's reduction is reported); its level is that colour's largest channel, and each light
//! takes the colour over the level. Lights stand at centres `anchor + (k + 0.5) * spacing` (integer `k` on each axis)
//! of the lattice's cells, so where the grid puts them whatever the bounds: one anchor serves the whole map. Which
//! cells is the caller's ([`FillCells`]): every cell that meets the bounds (covering them, a centre at most half a
//! spacing past them) or only those whose centre is inside them (an interior's, where a light past a wall would shine
//! through it: the lights cast no shadows). A light's intensity is the level times `(spacing / 2)^2`: under the
//! inverse square, a surface half a spacing from it gets the ambient's level. Its range is the caller's `reach`. A
//! light nearer a placed surface than `clearance` is left out (counted): a surface 1 m from a light of a 15 m grid
//! would be lit about 56 times the ambient's level, a bright spot.
//!
//! What determines the output is the caller's: the anchor, the spacing, the bounds (a whole map's play space, or an
//! interior's own box), which cells, the clearance and the reach. The lights' intensity is in `core.light`'s units,
//! the ambient's scale, so a destination's calibration of lights applies to them as to the scene's own.
//!
//! The ambient is consumed: the pass takes it out of `core.environment` (the sun stays), since the lights now stand
//! for it, so a writer does not report it as light it leaves out. A black ambient, which lights nothing, is consumed
//! too.
//!
//! The clearance search reads every placed surface near a light: in a part of a map divided for conversion
//! ([`primase_core::convert::PartScope`]) that is not the whole, the pass refuses (see the crate's README).

use primase_core::convert::PartScope;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{AmbientLight, Environment, Light, LightKind, Name, Transform, World};

use crate::surfaces::SurfaceGrid;

/// Which lattice cells [`FillGrid`] puts a light in. No default: the program chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillCells {
    /// Every cell `[anchor + k * spacing, anchor + (k + 1) * spacing)` that meets the bounds (closed), at least one on
    /// each axis: the bounds covered, a light at most half a spacing past them (`cells=meeting`).
    Meeting,
    /// Only the cells whose centre is inside the bounds (closed): every light inside them, none when no centre is
    /// (`cells=inside`).
    Inside,
}

impl Choice for FillCells {
    const VALUES: &'static [(&'static str, Self)] =
        &[("meeting", FillCells::Meeting), ("inside", FillCells::Inside)];
}

/// Where [`FillAmbientLights`] puts its lights and how far they reach: see the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct FillGrid {
    anchor: [f64; 3],
    spacing: f64,
    bounds: [[f64; 3]; 2],
    cells: FillCells,
    clearance: f64,
    reach: f64,
}

/// One lattice centre: its integer cell index on each axis and its position.
pub type FillPoint = ([i64; 3], [f64; 3]);

/// The most cells one grid may span: a bound on a grid whose bounds and spacing are out of proportion.
const MOST_POINTS: i128 = 10_000_000;

impl FillGrid {
    /// A grid from `anchor`, `spacing` metres apart (above 0), over `bounds` (min and max corners in the world frame, min
    /// not above max) by `cells`, leaving out lights nearer a surface than `clearance` (not below 0; 0 leaves none out),
    /// each reaching `reach` metres (above 0). Every value finite, every value the caller's.
    pub fn new(
        anchor: [f64; 3],
        spacing: f64,
        bounds: [[f64; 3]; 2],
        cells: FillCells,
        clearance: f64,
        reach: f64,
    ) -> Result<FillGrid, String> {
        let finite = |what: &str, xs: &[f64]| match xs.iter().find(|x| !x.is_finite()) {
            Some(x) => Err(format!("fill grid: {what} {x} is not a finite number")),
            None => Ok(()),
        };
        finite("anchor", &anchor)?;
        finite("spacing", &[spacing])?;
        finite("bounds", bounds.as_flattened())?;
        finite("clearance", &[clearance])?;
        finite("reach", &[reach])?;
        if spacing <= 0.0 {
            return Err(format!("fill grid: spacing {spacing} is not above 0 metres"));
        }
        if (0..3).any(|a| bounds[0][a] > bounds[1][a]) {
            return Err(format!(
                "fill grid: bounds {bounds:?}: a minimum above its maximum"
            ));
        }
        if clearance < 0.0 {
            return Err(format!("fill grid: clearance {clearance} is below 0 metres"));
        }
        if reach <= 0.0 {
            return Err(format!("fill grid: reach {reach} is not above 0 metres"));
        }
        let grid = FillGrid {
            anchor,
            spacing,
            bounds,
            cells,
            clearance,
            reach,
        };
        // the cells meeting the bounds, the most there can be, counted before any is listed
        let count: i128 = grid
            .meeting()
            .iter()
            .map(|&(a, b)| i128::from(b) - i128::from(a) + 1)
            .product();
        if count > MOST_POINTS {
            return Err(format!(
                "fill grid: {count} cells, over {MOST_POINTS}: spacing {spacing} is too small for bounds {bounds:?}"
            ));
        }
        Ok(grid)
    }

    /// From the options [`FILL_AMBIENT_LIGHTS`] declares, each required.
    fn from_given(g: &Given) -> Result<FillGrid, String> {
        let numbers = |k: &str, n: usize| -> Result<Vec<f64>, String> {
            let v = g.required(k);
            let xs: Vec<f64> = v
                .split(';')
                .map(|x| {
                    x.trim()
                        .parse::<f64>()
                        .map_err(|_| format!("{k}={v}: not numbers"))
                })
                .collect::<Result<_, _>>()?;
            if xs.len() == n {
                Ok(xs)
            } else {
                Err(format!("{k}={v}: {n} numbers expected"))
            }
        };
        let cells = g.choice::<FillCells>("cells");
        let a = numbers("anchor", 3)?;
        let b = numbers("bounds", 6)?;
        FillGrid::new(
            [a[0], a[1], a[2]],
            numbers("spacing", 1)?[0],
            [[b[0], b[1], b[2]], [b[3], b[4], b[5]]],
            cells,
            numbers("clearance", 1)?[0],
            numbers("reach", 1)?[0],
        )
    }

    fn options(&self) -> Options {
        let list = |xs: &[f64]| xs.iter().map(f64::to_string).collect::<Vec<_>>().join(";");
        Options::from([
            ("anchor".to_owned(), list(&self.anchor)),
            ("spacing".to_owned(), self.spacing.to_string()),
            ("bounds".to_owned(), list(self.bounds.as_flattened())),
            ("cells".to_owned(), self.cells.text().to_owned()),
            ("clearance".to_owned(), self.clearance.to_string()),
            ("reach".to_owned(), self.reach.to_string()),
        ])
    }

    /// On each axis, the first and last index of a cell `[anchor + k * spacing, anchor + (k + 1) * spacing)` that meets
    /// the bounds (at least one cell).
    fn meeting(&self) -> [(i64, i64); 3] {
        std::array::from_fn(|a| {
            let first = ((self.bounds[0][a] - self.anchor[a]) / self.spacing).floor() as i64;
            let last = ((self.bounds[1][a] - self.anchor[a]) / self.spacing).ceil() as i64 - 1;
            (first, last.max(first))
        })
    }

    /// On each axis, the cells chosen ([`FillCells`]): each index with its centre, in order. A centre inside the bounds
    /// is in a cell that meets them, so the inside cells are the meeting ones whose centre is inside.
    fn axes(&self) -> [Vec<(i64, f64)>; 3] {
        let meeting = self.meeting();
        std::array::from_fn(|a| {
            let (first, last) = meeting[a];
            (first..=last)
                .map(|k| (k, self.anchor[a] + (k as f64 + 0.5) * self.spacing))
                .filter(|&(_, c)| {
                    self.cells == FillCells::Meeting || (self.bounds[0][a] <= c && c <= self.bounds[1][a])
                })
                .collect()
        })
    }

    /// Every lattice centre the grid puts a light at (before the clearance search), in index order.
    pub fn points(&self) -> Vec<FillPoint> {
        let [x, y, z] = self.axes();
        let mut out = Vec::new();
        for &(i, px) in &x {
            for &(j, py) in &y {
                for &(k, pz) in &z {
                    out.push(([i, j, k], [px, py, pz]));
                }
            }
        }
        out
    }

    /// The box the clearance search reads: the lattice centres' box, `clearance` wider on every side; `None` when the
    /// grid has no centre.
    fn search_bounds(&self) -> Option<[[f64; 3]; 2]> {
        let mut out = [[0.0; 3]; 2];
        for (a, axis) in self.axes().iter().enumerate() {
            out[0][a] = axis.first()?.1 - self.clearance;
            out[1][a] = axis.last()?.1 + self.clearance;
        }
        Some(out)
    }
}

/// `fill-ambient-lights{anchor=..,spacing=..,bounds=..,cells=..,clearance=..,reach=..}`: see the module
/// documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct FillAmbientLights {
    pub grid: FillGrid,
}

impl FillAmbientLights {
    pub fn new(grid: FillGrid) -> FillAmbientLights {
        FillAmbientLights { grid }
    }
}

/// `fill-ambient-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): the grid is the program's choice,
/// every option required.
pub const FILL_AMBIENT_LIGHTS: PassSpec = PassSpec {
    name: "fill-ambient-lights",
    options: &[
        OptionSpec::required(
            "anchor",
            "0;0;0",
            "X;Y;Z, metres in the world frame: a corner of the lattice's cells",
        ),
        OptionSpec::required("spacing", "15", "metres between lights, above 0"),
        OptionSpec::required(
            "bounds",
            "0;0;0;15;15;15",
            "X0;Y0;Z0;X1;Y1;Z1, metres in the world frame: the box the lights cover, min not above max",
        ),
        OptionSpec::choice::<FillCells>(
            "cells",
            "which cells get a light: every one that meets the bounds, or only those whose centre is inside them",
        ),
        OptionSpec::required(
            "clearance",
            "3.75",
            "metres, not below 0: a light nearer a placed surface is left out (0 leaves none out)",
        ),
        OptionSpec::required("reach", "22.5", "metres, above 0: each light's range"),
    ],
    make: |g| Ok(Box::new(FillAmbientLights::new(FillGrid::from_given(g)?))),
};

impl Pass for FillAmbientLights {
    fn name(&self) -> &'static str {
        "fill-ambient-lights"
    }
    fn options(&self) -> Options {
        self.grid.options()
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name", "core.transform", "core.light"]
    }
    /// `core.environment`: its ambient taken out, consumed.
    fn modifies(&self) -> &[&'static str] {
        &["core.environment"]
    }
    fn approximates(&self) -> &'static str {
        "ambient light as shadowless point lights on a grid, of the ambient's mean colour"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<PartScope>().is_some_and(|s| !s.whole) {
            return Err("fill-ambient-lights: this part is not the whole map. Placing a map-anchored grid in parts \
                        needs each part's owned region (to place each lattice light once) and context reaching the \
                        clearance past it (for the clearance search); neither is implemented"
                .into());
        }
        let Some(ambient) = w.resource::<Environment>().and_then(|e| e.ambient.clone()) else {
            log.count("scenes without an ambient: no fill lights", 1);
            return Ok(());
        };
        let colours = ambient.light.colours();
        if matches!(ambient.light, AmbientLight::Gradient { .. }) {
            log.approximate(
                "the ambient's gradient reduced to its mean colour for the fill lights",
                1,
            );
        }
        let mut mean = [0.0f32; 3];
        for c in &colours {
            for (m, v) in mean.iter_mut().zip(c) {
                *m += *v * ambient.intensity / colours.len() as f32;
            }
        }
        let level = mean.iter().copied().fold(0.0f32, f32::max);
        if level <= 0.0 {
            log.count("scenes with a black ambient: no fill lights", 1);
            consume(w, log);
            return Ok(());
        }
        let g = &self.grid;
        let colour = mean.map(|c| c / level);
        let half = g.spacing / 2.0;
        let intensity = level * (half * half) as f32;
        // indexed in cells of the clearance itself, so a query reads the 27 cells around its own
        let surfaces = match g.search_bounds() {
            Some(bounds) if g.clearance > 0.0 => Some(SurfaceGrid::of_world(w, g.clearance, bounds)?),
            _ => None,
        };
        for (k, p) in g.points() {
            if surfaces.as_ref().is_some_and(|s| s.within(p, g.clearance)) {
                log.count(
                    "ambient fill lights left out (within the clearance of a surface)",
                    1,
                );
                continue;
            }
            let e = w.spawn();
            w.insert(e, Name(format!("ambient fill {}.{}.{}", k[0], k[1], k[2])));
            w.insert(
                e,
                Transform {
                    cols: [
                        [1.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [0.0, 0.0, 1.0],
                        p.map(|x| x as f32),
                    ],
                },
            );
            w.insert(
                e,
                Light {
                    kind: LightKind::Point,
                    color: colour,
                    intensity,
                    range: Some(g.reach as f32),
                    cast_shadows: Some(false),
                    radius: None,
                },
            );
            log.place(e);
            log.count("ambient fill lights", 1);
        }
        log.approximate("ambient light as shadowless point lights on a grid", 1);
        consume(w, log);
        Ok(())
    }
}

/// Takes the ambient out of the environment, the rest of it kept: the fill lights stand for it now.
fn consume(w: &mut World, log: &mut PassLog) {
    if let Some(mut env) = w.resource::<Environment>().cloned() {
        env.ambient = None;
        w.insert_resource(env);
        log.count("ambients consumed (the fill lights stand for them)", 1);
    }
}
