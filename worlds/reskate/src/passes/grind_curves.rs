//! `lower-reskate-grind-curves{kinds=K+K...,radius=M,surface=S}`: each `core.curve` of the stated kinds (`rail`,
//! `ledge`, `coping`) becomes a `skate.grind_curve` on the same entity, its `core.curve` taken off: the same points and
//! closure, the skate kind of the curve's kind, `radius` metres (Studio's 5-250 mm) and Studio's smooth-grind `surface`
//! (`metal_rail`, `metal`, `metal_thin`, `concrete`, `wood`). A source that knows a curve is grindable (SkateBIRD's
//! `GrindableRail` chains, a game's own rail splines) states it as a curve; which kinds grind, how thick and on what
//! surface are the program's, every option stated (no default). A curve with Bezier tangents is refused: run
//! `flatten-curves` first. Curves of other kinds stay. Run it before `raise-skate`, so the curves are raised with the
//! rest of the scene; `prepare-reskate-grinds` then builds their prisms.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Curve, World};

use crate::components::SkateGrindCurve;
use crate::grind::SURFACES;

/// The core curve kinds a grind curve can be, as the skate kind each becomes.
const KINDS: [(&str, u8); 3] = [("rail", 0), ("ledge", 1), ("coping", 2)];

/// The skate kind of each stated curve kind, and the surface's index.
type Checked = (Vec<(&'static str, u8)>, u32);

/// See the module documentation. `kinds`: at least one of `rail`, `ledge`, `coping`; `radius`: 0.005-0.25 m;
/// `surface`: one of Studio's smooth-grind surfaces ([`SURFACES`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerReskateGrindCurves {
    pub kinds: BTreeSet<String>,
    pub radius: f32,
    pub surface: String,
}

impl LowerReskateGrindCurves {
    /// From its options, every one required: `kinds` (joined by `+`), `radius` (metres), `surface`.
    pub fn from_options(o: &Options) -> Result<LowerReskateGrindCurves, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !["kinds", "radius", "surface"].contains(&k.as_str()))
        {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!("{k} is required: kinds=rail+ledge+coping (some), radius=metres, surface=metal_rail|metal|metal_thin|concrete|wood")
            })
        };
        let radius = get("radius")?;
        Ok(LowerReskateGrindCurves {
            kinds: get("kinds")?.split('+').map(str::to_owned).collect(),
            radius: radius
                .parse()
                .map_err(|_| format!("radius {radius}: not a number of metres"))?,
            surface: get("surface")?.to_owned(),
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
        Ok((kinds, u32::try_from(surface).map_err(|e| e.to_string())?))
    }
}

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
    }
    fn provides(&self) -> &[&'static str] {
        &[SkateGrindCurve::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Curve::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let (kinds, surface) = self.checked()?;
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
        ]);
        let pass = LowerReskateGrindCurves::from_options(&o).unwrap();
        assert_eq!(pass.options(), o);
        pass.run(&mut w, &mut PassLog::default()).unwrap();
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
        assert!(LowerReskateGrindCurves::from_options(&Options::new()).is_err());
    }
}
