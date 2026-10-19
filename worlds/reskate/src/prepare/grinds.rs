//! `prepare-reskate-grinds`: each grind curve's invisible collision prism, built as Studio builds it, for the writer to
//! write.
//!
//! Each `skate.grind_curve` gets a `skate.grind_prism`: the polyline simplified, its runs split at sharp corners and a
//! prism hung under each run with one edge on the line ([`PrismRules`]; Studio's by default), of the curve's radius
//! (5-250 mm; 30 mm when it states none). A curve with no usable points gets an empty prism (the writer warns of it);
//! a curve whose surface is not one of Studio's five smooth-grind surfaces is refused. The writer writes the prisms and
//! makes none: a curve without one is reported unsupported. Run it after the passes that move the curves (the floor).

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};
use primase_meshops::vector::Vec3;

use crate::dialect::{SkateGrindCurve, SkateGrindPrism};
use crate::grind::{self, PrismRules, SURFACES};
use crate::scene;

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateGrinds {
    rules: PrismRules,
}

impl PrepareReskateGrinds {
    /// With Studio's rules ([`PrismRules::STUDIO`]).
    pub fn new() -> PrepareReskateGrinds {
        PrepareReskateGrinds {
            rules: PrismRules::STUDIO,
        }
    }

    /// Other rules: 3 sides or more, a finite tolerance and spacing of 0 m or more, a corner of 0-180 degrees.
    pub fn with_rules(mut self, rules: PrismRules) -> Result<PrepareReskateGrinds, String> {
        let PrismRules {
            sides,
            tolerance,
            min_spacing,
            corner_degrees,
        } = rules;
        if sides < 3 {
            return Err(format!("sides={sides}: a prism has 3 sides or more"));
        }
        if !(tolerance.is_finite() && tolerance >= 0.0) || !(min_spacing.is_finite() && min_spacing >= 0.0) {
            return Err(format!(
                "tolerance={tolerance}, min_spacing={min_spacing}: finite metres of 0 or more"
            ));
        }
        if !(0.0..=180.0).contains(&corner_degrees) {
            return Err(format!(
                "corner_degrees={corner_degrees}: an angle of 0-180 degrees"
            ));
        }
        self.rules = rules;
        Ok(self)
    }

    pub fn rules(&self) -> PrismRules {
        self.rules
    }

    /// From its options (each optional, Studio's otherwise), through [`PrepareReskateGrinds::with_rules`].
    pub fn from_options(o: &Options) -> Result<PrepareReskateGrinds, String> {
        let mut r = PrismRules::STUDIO;
        for (k, v) in o {
            let number = || v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number"));
            match k.as_str() {
                "sides" => r.sides = v.parse().map_err(|_| format!("sides={v} is not a count"))?,
                "tolerance" => r.tolerance = number()?,
                "min_spacing" => r.min_spacing = number()?,
                "corner_degrees" => r.corner_degrees = number()?,
                _ => return Err(format!("unknown option {k}")),
            }
        }
        PrepareReskateGrinds::new().with_rules(r)
    }
}

impl Default for PrepareReskateGrinds {
    /// Studio's rules.
    fn default() -> Self {
        PrepareReskateGrinds::new()
    }
}

/// [`PrepareReskateGrinds::from_options`] as a pass factory.
pub fn prepare_reskate_grinds(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateGrinds::from_options(o)?))
}

impl Pass for PrepareReskateGrinds {
    fn name(&self) -> &'static str {
        "prepare-reskate-grinds"
    }
    fn options(&self) -> Options {
        let r = &self.rules;
        Options::from([
            ("sides".to_owned(), r.sides.to_string()),
            ("tolerance".to_owned(), r.tolerance.to_string()),
            ("min_spacing".to_owned(), r.min_spacing.to_string()),
            ("corner_degrees".to_owned(), r.corner_degrees.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.grind_curve"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.grind_prism"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let curves: Vec<_> = w.iter::<SkateGrindCurve>().map(|(e, c)| (e, c.clone())).collect();
        for (e, c) in curves {
            if w.has::<SkateGrindPrism>(e) {
                return Err(
                    "prepare-reskate-grinds: a grind curve already has its prism (skate.grind_prism)".into(),
                );
            }
            if c.surface as usize >= SURFACES.len() {
                return Err(format!(
                    "prepare-reskate-grinds: a grind curve's surface {} is none of Studio's {} smooth-grind surfaces",
                    c.surface,
                    SURFACES.len()
                ));
            }
            // built in the writer's frame (z up), stored in the scene's: both are the same coordinates swapped and
            // negated, exactly
            let points: Vec<Vec3> = c.points.iter().map(|p| scene::w(p[0], p[1], p[2])).collect();
            let (pos, idx, pieces, length) =
                grind::build(&points, c.closed, grind::radius(c.radius), &self.rules);
            if idx.is_empty() {
                log.count("grind curves with no usable points: empty prisms", 1);
            } else {
                log.count("grind curves made prisms", 1);
                log.count("grind prism runs", u64::from(pieces));
                log.count("grind prism triangles", (idx.len() / 3) as u64);
            }
            w.insert(
                e,
                SkateGrindPrism {
                    positions: pos.iter().map(|q| [q.x, q.z, -q.y]).collect(),
                    triangles: idx.as_chunks::<3>().0.to_vec(),
                    pieces,
                    length,
                },
            );
        }
        Ok(())
    }
}
