//! `lower-reskate-grind-curves{kinds=K+K...,radius=M,surface=S,leave-out=nothing|on-slopes[,within=M,flat=F,share=S]}`:
//! each `core.curve` of the stated kinds (`rail`,
//! `ledge`, `coping`) becomes a `skate.grind_curve` on the same entity, its `core.curve` taken off: the same points and
//! closure, the skate kind of the curve's kind, `radius` metres (Studio's 5-250 mm) and Studio's smooth-grind `surface`
//! (`metal_rail`, `metal`, `metal_thin`, `concrete`, `wood`). A source that knows a curve is grindable (SkateBIRD's
//! `GrindableRail` chains, a game's own rail splines) states it as a curve; which kinds grind, how thick and on what
//! surface are the program's, every option stated (no default). A curve with Bezier tangents is refused: run
//! `flatten-curves` first. Curves of other kinds stay. Run it before `raise-skate`, so the curves are raised with the
//! rest of the scene; `prepare-reskate-grinds` then builds their prisms.
//!
//! Studio's grind prisms are solid, hung under the line, so a curve along a ramp's lip (a quarter-pipe's coping)
//! pokes its prism out of the ramp face, where riders going up hit it. `leave-out=on-slopes` leaves out the curves that
//! rest on sloped collision: each curve is sampled at its points and the midpoint of each segment (in the core frame,
//! through its entity's transform when it has one); a sample rests on a slope when the first collision surface straight
//! below it (any placed `core.collision`, counting one up to `within` metres above the sample) is within `within`
//! metres of it and its normal's vertical part is under `flat` (1 is level); a curve whose samples with a surface below
//! rest on slopes in at least the share `share` of them is left out: its `core.curve` removed, counted as an
//! omission by kind. A curve with no surface below any sample (a rope over open space) is kept. `leave-out=nothing`
//! makes every curve of the stated kinds a grind. Every value is the program's, stated.

use std::collections::BTreeSet;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Curve, Transform, World};
use primase_meshops::collision_query::{Ground, at};

use crate::components::SkateGrindCurve;
use crate::grind::SURFACES;

/// The core curve kinds a grind curve can be, as the skate kind each becomes.
const KINDS: [(&str, u8); 3] = [("rail", 0), ("ledge", 1), ("coping", 2)];

/// The skate kind of each stated curve kind, and the surface's index.
type Checked = (Vec<(&'static str, u8)>, u32);

/// Which curves of the stated kinds [`LowerReskateGrindCurves`] leaves out (module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GrindLeaveOut {
    /// None: every curve of the stated kinds grinds.
    Nothing,
    /// The curves resting on sloped collision: a sample within `within` metres of the surface below it, that surface's
    /// normal's vertical part under `flat`, in at least the share `share` of the samples with a surface below.
    OnSlopes { within: f64, flat: f64, share: f64 },
}

/// See the module documentation. `kinds`: at least one of `rail`, `ledge`, `coping`; `radius`: 0.005-0.25 m;
/// `surface`: one of Studio's smooth-grind surfaces ([`SURFACES`]); `leave_out`: the curves it leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerReskateGrindCurves {
    pub kinds: BTreeSet<String>,
    pub radius: f32,
    pub surface: String,
    pub leave_out: GrindLeaveOut,
}

impl LowerReskateGrindCurves {
    /// From its options: `kinds` (joined by `+`), `radius` (metres), `surface`, `leave-out` (`nothing`, or
    /// `on-slopes` with `within` metres, `flat` and `share`).
    fn from_given(g: &Given) -> Result<LowerReskateGrindCurves, String> {
        let number = |k: &str| -> Result<Option<f64>, String> {
            g.optional(k)
                .map(|v| v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let slopes = (number("within")?, number("flat")?, number("share")?);
        let leave_out = match (g.choice::<LeaveOutKind>("leave-out"), slopes) {
            (LeaveOutKind::Nothing, (None, None, None)) => GrindLeaveOut::Nothing,
            (LeaveOutKind::Nothing, _) => {
                return Err("within, flat and share are leave-out=on-slopes' only".into());
            }
            (LeaveOutKind::OnSlopes, (Some(within), Some(flat), Some(share))) => {
                GrindLeaveOut::OnSlopes { within, flat, share }
            }
            (LeaveOutKind::OnSlopes, _) => {
                return Err(
                    "leave-out=on-slopes needs within (metres), flat (0..1, 1 level) and share (0..1)".into(),
                );
            }
        };
        let radius = g.required("radius");
        Ok(LowerReskateGrindCurves {
            kinds: g.required("kinds").split('+').map(str::to_owned).collect(),
            radius: radius
                .parse()
                .map_err(|_| format!("radius {radius}: not a number of metres"))?,
            surface: g.required("surface").to_owned(),
            leave_out,
        })
    }

    /// The skate kind of each stated curve kind and the surface's index, refusing what Studio has no grind for.
    fn checked(&self) -> Result<Checked, String> {
        if self.kinds.is_empty() {
            return Err("lower-reskate-grind-curves needs at least one curve kind".into());
        }
        let kinds = (self.kinds.iter())
            .map(|k| {
                KINDS
                    .into_iter()
                    .find(|(name, _)| name == k)
                    .ok_or_else(|| format!("curve kind {k}: not one a grind can be (rail, ledge, coping)"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !(0.005..=0.25).contains(&self.radius) {
            return Err(format!(
                "radius {} is not within Studio's 0.005-0.25 m",
                self.radius
            ));
        }
        let surface = (SURFACES.iter())
            .position(|(name, _)| *name == self.surface)
            .ok_or_else(|| {
                format!(
                    "surface {}: not one of Studio's smooth-grind surfaces",
                    self.surface
                )
            })?;
        if let GrindLeaveOut::OnSlopes { within, flat, share } = self.leave_out
            && !(within.is_finite()
                && within >= 0.0
                && (0.0..=1.0).contains(&flat)
                && (0.0..=1.0).contains(&share))
        {
            return Err(format!(
                "leave-out=on-slopes: within {within} must be metres at least 0, flat {flat} and share {share} within 0..1"
            ));
        }
        Ok((kinds, u32::try_from(surface).map_err(|e| e.to_string())?))
    }
}

/// Curve `c`'s samples in the core frame, through `t` when it has one: its points and the midpoint of each segment
/// (the closing one too when it is closed).
fn samples(c: &Curve, t: Option<&Transform>) -> Vec<[f64; 3]> {
    let points: Vec<[f64; 3]> = c.points.iter().map(|&p| at(t, p)).collect();
    let mut out = points.clone();
    let segments = if c.closed {
        points.len()
    } else {
        points.len().saturating_sub(1)
    };
    for i in 0..segments {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        out.push(std::array::from_fn(|k| (a[k] + b[k]) / 2.0));
    }
    out
}

/// Whether the curve with samples `samples` rests on sloped collision ([`GrindLeaveOut::OnSlopes`]).
fn rests_on_slopes(ground: &Ground, samples: &[[f64; 3]], within: f64, flat: f64, share: f64) -> bool {
    let (mut below, mut sloped) = (0usize, 0usize);
    for &p in samples {
        let Some(s) = ground.surface_below(p, within) else {
            continue;
        };
        below += 1;
        if p[1] - s.height <= within && s.level < flat {
            sloped += 1;
        }
    }
    below > 0 && sloped as f64 >= share * below as f64
}

/// `leave-out`'s values: what [`GrindLeaveOut`] is, without its numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeaveOutKind {
    Nothing,
    OnSlopes,
}

impl Choice for LeaveOutKind {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("nothing", LeaveOutKind::Nothing),
        ("on-slopes", LeaveOutKind::OnSlopes),
    ];
}

/// An option of `leave-out=on-slopes`.
const fn on_slopes(key: &'static str, example: &'static str, doc: &'static str) -> OptionSpec {
    OptionSpec::optional(
        key,
        example,
        doc,
        "required with leave-out=on-slopes, not taken with nothing",
    )
}

/// `lower-reskate-grind-curves`' declaration ([`primase_core::pass::PassRegistry::add`]): which curves grind, how thick,
/// on what and which it leaves out are the program's, no default.
pub const LOWER_RESKATE_GRIND_CURVES: PassSpec = PassSpec {
    name: "lower-reskate-grind-curves",
    options: &[
        OptionSpec::required(
            "kinds",
            "rail",
            "the curve kinds that grind (rail, ledge, coping), joined by +",
        ),
        OptionSpec::required(
            "radius",
            "0.03",
            "metres, Studio's 0.005-0.25: the grind prism's radius",
        ),
        OptionSpec::required(
            "surface",
            "metal_rail",
            "Studio's smooth-grind surface: metal_rail, metal, metal_thin, concrete or wood",
        ),
        OptionSpec::choice::<LeaveOutKind>(
            "leave-out",
            "every curve of the kinds grinds (nothing), or those resting on sloped collision are left out (on-slopes)",
        ),
        on_slopes(
            "within",
            "0.5",
            "metres: how near the surface below a sample must be to hold it",
        ),
        on_slopes(
            "flat",
            "0.9",
            "0..1, 1 level: the least vertical part of a level surface's normal",
        ),
        on_slopes(
            "share",
            "0.5",
            "0..1: the least share of a curve's samples on slopes that leaves it out",
        ),
    ],
    make: |g| Ok(Box::new(LowerReskateGrindCurves::from_given(g)?)),
};

impl Pass for LowerReskateGrindCurves {
    fn name(&self) -> &'static str {
        "lower-reskate-grind-curves"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "kinds".to_owned(),
                self.kinds.iter().cloned().collect::<Vec<_>>().join("+"),
            ),
            ("radius".to_owned(), self.radius.to_string()),
            ("surface".to_owned(), self.surface.clone()),
        ])
        .into_iter()
        .chain(match self.leave_out {
            GrindLeaveOut::Nothing => vec![("leave-out".to_owned(), LeaveOutKind::Nothing.text().to_owned())],
            GrindLeaveOut::OnSlopes { within, flat, share } => vec![
                ("leave-out".to_owned(), LeaveOutKind::OnSlopes.text().to_owned()),
                ("within".to_owned(), within.to_string()),
                ("flat".to_owned(), flat.to_string()),
                ("share".to_owned(), share.to_string()),
            ],
        })
        .collect()
    }
    fn provides(&self) -> &[&'static str] {
        &[SkateGrindCurve::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Curve::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let (kinds, surface) = self.checked()?;
        // the world's collision, once, for the curves' samples
        let ground = matches!(self.leave_out, GrindLeaveOut::OnSlopes { .. }).then(|| Ground::of(w));
        let curves: Vec<(primase_core::Entity, Curve, u8)> = (w.iter::<Curve>())
            .filter_map(|(e, c)| {
                let kind = kinds.iter().find(|(name, _)| *name == c.kind)?.1;
                Some((e, c.clone(), kind))
            })
            .collect();
        for (e, c, kind) in curves {
            if c.tangents.is_some() {
                return Err(format!(
                    "a {} curve with Bezier tangents: run flatten-curves before lower-reskate-grind-curves",
                    c.kind
                ));
            }
            if let (Some(ground), GrindLeaveOut::OnSlopes { within, flat, share }) = (&ground, self.leave_out)
                && rests_on_slopes(ground, &samples(&c, w.get::<Transform>(e)), within, flat, share)
            {
                w.remove::<Curve>(e);
                log.record_removed(Curve::NAME);
                log.omit(
                    &format!(
                        "{} curves resting on sloped collision left out (leave-out=on-slopes)",
                        c.kind
                    ),
                    1,
                );
                continue;
            }
            log.count(&format!("{} curves made grind curves", c.kind), 1);
            w.remove::<Curve>(e);
            log.record_removed(Curve::NAME);
            w.insert(
                e,
                SkateGrindCurve {
                    points: c.points,
                    closed: c.closed,
                    kind,
                    radius: self.radius,
                    surface,
                    unknown_flags: 0,
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::Collision;

    use super::*;

    fn curve(w: &mut World, kind: &str) -> primase_core::Entity {
        let e = w.spawn();
        w.insert(
            e,
            Curve {
                kind: kind.into(),
                points: vec![[0.0, 1.0, 0.0], [2.0, 1.0, 0.0]],
                closed: false,
                tangents: None,
                up: None,
            },
        );
        e
    }

    /// A rail curve becomes a rail grind with the stated radius and surface; a path curve stays; bad choices are
    /// refused when it runs, and the options read back.
    #[test]
    fn stated_curve_kinds_become_grind_curves() {
        let mut w = World::new();
        let (rail, path) = (curve(&mut w, "rail"), curve(&mut w, "path"));
        let o = Options::from([
            ("kinds".to_owned(), "rail".to_owned()),
            ("radius".to_owned(), "0.03".to_owned()),
            ("surface".to_owned(), "metal_rail".to_owned()),
            ("leave-out".to_owned(), "nothing".to_owned()),
        ]);
        let made = LOWER_RESKATE_GRIND_CURVES.instance(&o).unwrap();
        assert_eq!(made.options(), o);
        made.run(&mut w, &mut PassLog::default()).unwrap();
        let pass = LowerReskateGrindCurves {
            kinds: BTreeSet::from(["rail".to_owned()]),
            radius: 0.03,
            surface: "metal_rail".into(),
            leave_out: GrindLeaveOut::Nothing,
        };
        assert_eq!(pass.options(), o);
        let g = w.get::<SkateGrindCurve>(rail).expect("a grind curve");
        assert_eq!((g.kind, g.radius, g.surface, g.points.len()), (0, 0.03, 0, 2));
        assert!(w.get::<Curve>(rail).is_none());
        assert!(w.get::<Curve>(path).is_some() && w.get::<SkateGrindCurve>(path).is_none());
        for bad in [
            LowerReskateGrindCurves {
                radius: 0.5,
                ..pass.clone()
            },
            LowerReskateGrindCurves {
                surface: "glass".into(),
                ..pass.clone()
            },
            LowerReskateGrindCurves {
                kinds: BTreeSet::from(["path".to_owned()]),
                ..pass.clone()
            },
        ] {
            assert!(bad.run(&mut World::new(), &mut PassLog::default()).is_err());
        }
        assert!(LOWER_RESKATE_GRIND_CURVES.instance(&Options::new()).is_err());
    }

    /// Over a 45-degree ramp with a level deck behind it: a rail along the ramp's lip (a coping) is left out and counted,
    /// a rail standing 50 cm over the deck, one with only part of it on the ramp and one over nothing grind; the
    /// slope options are required with on-slopes, refused with nothing, and read back.
    #[test]
    fn curves_resting_on_slopes_are_left_out() {
        let mut w = World::new();
        let collision = w.spawn();
        w.insert(
            collision,
            Collision {
                // ramp up +x from (0, 0) to (2, 2), deck at 2 m from x 2 to 6, z 0 to 4
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [2.0, 2.0, 0.0],
                    [2.0, 2.0, 4.0],
                    [0.0, 0.0, 4.0],
                    [6.0, 2.0, 0.0],
                    [6.0, 2.0, 4.0],
                ],
                triangles: vec![[0, 1, 2], [0, 2, 3], [1, 4, 5], [1, 5, 2]],
                surfaces: vec![0; 4],
            },
        );
        let rail = |w: &mut World, points: Vec<[f32; 3]>| {
            let e = curve(w, "rail");
            w.get_mut::<Curve>(e).unwrap().points = points;
            e
        };
        // just under the lip on the ramp face, then along it: every sample on the ramp
        let coping = rail(&mut w, vec![[1.9995, 1.999, 0.5], [1.9995, 1.999, 3.5]]);
        let over_deck = rail(&mut w, vec![[3.0, 2.5, 1.0], [5.0, 2.5, 1.0]]);
        // one end on the ramp, the rest over the deck at 30 cm
        let partly = rail(&mut w, vec![[1.0, 1.0, 2.0], [3.0, 2.3, 2.0], [5.0, 2.3, 2.0]]);
        let rope = rail(&mut w, vec![[10.0, 3.0, 0.0], [12.0, 3.0, 0.0]]);
        // placed by its entity's transform: stored 2 m to the left of the lip, moved onto it
        let moved = rail(&mut w, vec![[-0.0005, 1.999, 0.5], [-0.0005, 1.999, 3.5]]);
        let mut t = Transform::IDENTITY;
        t.cols[3] = [2.0, 0.0, 0.0];
        w.insert(moved, t);
        let o = Options::from([
            ("kinds".to_owned(), "rail".to_owned()),
            ("radius".to_owned(), "0.03".to_owned()),
            ("surface".to_owned(), "metal_rail".to_owned()),
            ("leave-out".to_owned(), "on-slopes".to_owned()),
            ("within".to_owned(), "0.01".to_owned()),
            ("flat".to_owned(), "0.95".to_owned()),
            ("share".to_owned(), "0.5".to_owned()),
        ]);
        let pass = LOWER_RESKATE_GRIND_CURVES.instance(&o).unwrap();
        assert_eq!(pass.options(), o);
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        for e in [coping, moved] {
            assert!(w.get::<Curve>(e).is_none() && w.get::<SkateGrindCurve>(e).is_none());
        }
        for e in [over_deck, partly, rope] {
            assert!(w.get::<Curve>(e).is_none() && w.get::<SkateGrindCurve>(e).is_some());
        }
        let text = format!("{log:?}");
        assert!(
            text.contains("rail curves resting on sloped collision left out"),
            "{text}"
        );

        for (k, v) in [("within", None), ("share", Some("2"))] {
            let mut bad = o.clone();
            match v {
                Some(v) => bad.insert(k.to_owned(), v.to_owned()),
                None => bad.remove(k),
            };
            let refused = LOWER_RESKATE_GRIND_CURVES
                .instance(&bad)
                .and_then(|p| p.run(&mut World::new(), &mut PassLog::default()));
            assert!(refused.is_err(), "{k}");
        }
        let mut nothing = o.clone();
        nothing.insert("leave-out".to_owned(), "nothing".to_owned());
        assert!(LOWER_RESKATE_GRIND_CURVES.instance(&nothing).is_err());
    }
}
