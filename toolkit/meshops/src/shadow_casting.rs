//! `set-shadow-casting{placements=<prefix>;...,cast=on|off}`: the program's choice of which drawn entities cast
//! shadows. Shadow casting is stated per drawn entity (`core.shadows` `cast`; absent casts), by a reader where its
//! source states it, else by this pass: a room's shell left out of the sun's shadows so the sun and sky light the
//! inside, as the program chooses.
//!
//! Both options are required. `placements`: name prefixes (a drawn entity's `core.name`, as its reader wrote it,
//! path separators included). A drawn entity is a placement (`core.instance`) or an entity drawing its own mesh
//! (`core.mesh` and `core.transform`, placed by no placement). Each one whose name starts with a prefix gets `cast`
//! set; its `receive` is kept. A prefix no drawn entity's name starts with is an error naming it: a selection that
//! selects nothing is a mistake, not an empty change. Counted per prefix (an entity several prefixes match is
//! counted under each) and in all.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, Mesh, Name, ShadowCast, Shadows, Transform, World};

/// See the module documentation. Made by [`SetShadowCasting::new`]; from options ([`SET_SHADOW_CASTING`]) by the same call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetShadowCasting {
    placements: BTreeSet<String>,
    cast: bool,
}

impl SetShadowCasting {
    /// The drawn entities named with a prefix in `placements` (at least one, none empty or holding `;`) cast shadows
    /// when `cast`, else none.
    pub fn new(placements: BTreeSet<String>, cast: bool) -> Result<SetShadowCasting, String> {
        if placements.is_empty() {
            return Err("set-shadow-casting needs at least one placement prefix".into());
        }
        if let Some(p) = placements.iter().find(|p| p.is_empty() || p.contains(';')) {
            return Err(format!("prefix {p:?}: empty or holding ';'"));
        }
        Ok(SetShadowCasting { placements, cast })
    }

    pub fn placements(&self) -> &BTreeSet<String> {
        &self.placements
    }

    pub fn cast(&self) -> bool {
        self.cast
    }

    /// From its declared options, through [`SetShadowCasting::new`].
    fn from_given(g: &Given) -> Result<SetShadowCasting, String> {
        SetShadowCasting::new(
            g.required("placements").split(';').map(str::to_owned).collect(),
            g.choice::<Cast>("cast") == Cast::On,
        )
    }
}

/// Option `cast`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cast {
    On,
    Off,
}

impl Choice for Cast {
    const VALUES: &'static [(&'static str, Self)] = &[("on", Cast::On), ("off", Cast::Off)];
}

/// `set-shadow-casting`'s declaration ([`primase_core::pass::PassRegistry::add`]): which entities cast is the
/// program's, both options required.
pub const SET_SHADOW_CASTING: PassSpec = PassSpec {
    name: "set-shadow-casting",
    options: &[
        OptionSpec::required(
            "placements",
            "level",
            "name prefixes of the drawn entities whose casting is set, joined by ;",
        ),
        OptionSpec::choice::<Cast>("cast", "whether they cast shadows"),
    ],
    make: |g| Ok(Box::new(SetShadowCasting::from_given(g)?)),
};

impl Pass for SetShadowCasting {
    fn name(&self) -> &'static str {
        "set-shadow-casting"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "placements".to_owned(),
                self.placements.iter().cloned().collect::<Vec<_>>().join(";"),
            ),
            (
                "cast".to_owned(),
                if self.cast { Cast::On } else { Cast::Off }.text().to_owned(),
            ),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[Shadows::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Shadows::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let prototypes: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let drawn: Vec<(Entity, String)> = w
            .iter::<Name>()
            .filter(|(e, _)| {
                w.has::<Instance>(*e)
                    || (w.has::<Mesh>(*e) && w.has::<Transform>(*e) && !prototypes.contains(e))
            })
            .map(|(e, n)| (e, n.0.clone()))
            .collect();
        let mut by_prefix: BTreeMap<&String, u64> = BTreeMap::new();
        let mut chosen: BTreeSet<Entity> = BTreeSet::new();
        for p in &self.placements {
            let matched: Vec<Entity> = drawn
                .iter()
                .filter(|(_, name)| name.starts_with(p.as_str()))
                .map(|(e, _)| *e)
                .collect();
            if matched.is_empty() {
                return Err(format!(
                    "set-shadow-casting: no drawn entity's name starts with {p} (a name is matched as its reader \
                     wrote it, path separators included)"
                ));
            }
            by_prefix.insert(p, matched.len() as u64);
            chosen.extend(matched);
        }
        let cast = if self.cast {
            ShadowCast::On
        } else {
            ShadowCast::Off
        };
        for &e in &chosen {
            let mut s = w.get::<Shadows>(e).copied().unwrap_or_default();
            s.cast = Some(cast);
            w.insert(e, s);
        }
        for (p, n) in by_prefix {
            log.count(&format!("drawn entities named {p}..."), n);
        }
        log.count(
            if self.cast {
                "drawn entities casting shadows"
            } else {
                "drawn entities casting no shadow"
            },
            chosen.len() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};

    fn run(w: &mut World, pass: SetShadowCasting) -> Result<RunReport, String> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn named(w: &mut World, name: &str) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        e
    }

    #[test]
    fn named_drawn_entities_get_the_choice_and_keep_receiving() {
        let mut w = World::new();
        let proto = named(&mut w, "Bounds/prototype");
        w.insert(proto, Mesh::new(Vec::new()));
        w.insert(proto, Transform::IDENTITY);
        let place = |w: &mut World, name: &str| {
            let e = named(w, name);
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(e, Transform::IDENTITY);
            e
        };
        let (wall, roof, park) = (
            place(&mut w, "Bounds/Walls"),
            place(&mut w, "Bounds/Roof"),
            place(&mut w, "Park/Ramp"),
        );
        w.insert(
            roof,
            Shadows {
                cast: None,
                receive: Some(false),
            },
        );
        let own = named(&mut w, "Bounds/Ceiling");
        w.insert(own, Mesh::new(Vec::new()));
        w.insert(own, Transform::IDENTITY);
        let report = run(
            &mut w,
            SetShadowCasting::new(BTreeSet::from(["Bounds/".to_owned()]), false).unwrap(),
        )
        .unwrap();
        let cast = |e| w.get::<Shadows>(e).and_then(|s| s.cast);
        assert_eq!(cast(wall), Some(ShadowCast::Off));
        assert_eq!(cast(own), Some(ShadowCast::Off), "a mesh drawn by itself");
        assert_eq!(
            w.get::<Shadows>(roof).unwrap().receive,
            Some(false),
            "receive kept"
        );
        assert_eq!(cast(park), None, "not named: casts, as it did");
        // the prototype is drawn through its placements, not itself
        assert_eq!(cast(proto), None);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["drawn entities named Bounds/..."], 3);
        assert_eq!(log.counts["drawn entities casting no shadow"], 3);
    }

    #[test]
    fn a_prefix_that_matches_nothing_is_refused() {
        let mut w = World::new();
        let e = named(&mut w, "Bounds/Walls");
        w.insert(e, Mesh::new(Vec::new()));
        w.insert(e, Transform::IDENTITY);
        let e = run(
            &mut w,
            SetShadowCasting::new(BTreeSet::from(["Bounds/".to_owned(), "Lamps/".to_owned()]), true).unwrap(),
        )
        .unwrap_err();
        assert!(e.contains("starts with Lamps/"), "{e}");
    }

    #[test]
    fn options_are_required_and_round_trip() {
        let p = SetShadowCasting::new(BTreeSet::from(["a".to_owned(), "b/".to_owned()]), false).unwrap();
        let make = |o: &Options| SET_SHADOW_CASTING.instance(o);
        assert_eq!(make(&p.options()).unwrap().options(), p.options());
        assert!(make(&Options::from([("placements".into(), "a".into())])).is_err());
        assert!(make(&Options::from([("cast".into(), "on".into())])).is_err());
        assert!(
            make(&Options::from([
                ("placements".into(), "a".into()),
                ("cast".into(), "only".into())
            ]))
            .is_err()
        );
        assert!(
            make(&Options::from([
                ("placements".into(), "a;".into()),
                ("cast".into(), "on".into())
            ]))
            .is_err()
        );
        assert!(SetShadowCasting::new(BTreeSet::new(), true).is_err());
    }
}
