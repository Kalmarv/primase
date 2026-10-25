//! `omit-placements{prefixes=P;P...,collision=keep|omit}`: every placement (an entity with `core.instance`) whose
//! `core.name` starts with one of the prefixes (names as the reader wrote them: a GameObject path joined by `/`, an
//! entity's name) stops drawing. A program states the prefixes, joined by `;` (paths hold `/`), and whether
//! their collision stays; every option is required.
//!
//! - `collision=keep`: the placement keeps colliding. Its own collision (`core.collision` on the placement) stays and
//!   only its `core.instance` goes; its prototype's collision moves with it to a collision-only copy of the prototype
//!   (`core.collision`, `core.name` and `core.source` copied, no mesh), one copy per prototype, made from the prototype
//!   ([`PassLog::place_from`]), so placements not selected keep drawing the prototype as before. A placement with no
//!   collision of either kind has nothing left and is despawned (counted).
//! - `collision=omit`: the placement is despawned.
//!
//! Counted per prefix (a placement counts for the first prefix it matches). A prefix that matches no placement is
//! refused, so a renamed path fails loudly, when the world is the whole of what is written. In a conversion in parts
//! (`core.part_scope` not whole) a part may hold none of a prefix's placements: each part counts what it matched
//! (zero included) and none is refused, so a prefix no part matched shows only as zero counts in every part.

use std::collections::BTreeMap;

use primase_core::convert::PartScope;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Asset, Collision, Component, Entity, Instance, Name, Source, World};

/// What becomes of a selected placement's collision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementCollision {
    Keep,
    Omit,
}

/// See the module documentation. `prefixes`: at least one, none empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmitPlacements {
    pub prefixes: Vec<String>,
    pub collision: PlacementCollision,
}

impl Choice for PlacementCollision {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("keep", PlacementCollision::Keep),
        ("omit", PlacementCollision::Omit),
    ];
}

impl OmitPlacements {
    /// From its declared options.
    fn from_given(g: &Given) -> OmitPlacements {
        OmitPlacements {
            prefixes: g.required("prefixes").split(';').map(str::to_owned).collect(),
            collision: g.choice("collision"),
        }
    }
}

/// `omit-placements`' declaration ([`primase_core::pass::PassRegistry::add`]): which placements stop drawing and
/// whether they keep colliding are the program's, both options required.
pub const OMIT_PLACEMENTS: PassSpec = PassSpec {
    name: "omit-placements",
    options: &[
        OptionSpec::required(
            "prefixes",
            "Level/Ceiling",
            "name prefixes of the placements that stop drawing, joined by ;",
        ),
        OptionSpec::choice::<PlacementCollision>(
            "collision",
            "whether the placements keep colliding, or are despawned",
        ),
    ],
    make: |g| Ok(Box::new(OmitPlacements::from_given(g))),
};

impl Pass for OmitPlacements {
    fn name(&self) -> &'static str {
        "omit-placements"
    }
    fn options(&self) -> Options {
        Options::from([
            ("prefixes".to_owned(), self.prefixes.join(";")),
            ("collision".to_owned(), self.collision.text().to_owned()),
        ])
    }
    /// The collision-only copies of prototypes: their collision, name and source.
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Name::NAME, Source::NAME]
    }
    /// A placement that keeps its own collision loses its placement of the prototype.
    fn removes(&self) -> &[&'static str] {
        &[Instance::NAME]
    }
    /// A placement keeping its prototype's collision is pointed at the copy; a placement despawned stops being listed by
    /// the assets that list it.
    fn modifies(&self) -> &[&'static str] {
        &[Instance::NAME, Asset::NAME]
    }
    /// It lowers nothing: what it takes away is left out.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if self.prefixes.is_empty() || self.prefixes.iter().any(String::is_empty) {
            return Err("omit-placements needs at least one prefix, none empty".into());
        }
        let mut matched: BTreeMap<&str, u64> = self.prefixes.iter().map(|p| (p.as_str(), 0)).collect();
        let mut selected: Vec<(Entity, Entity)> = Vec::new();
        for (e, i) in w.iter::<Instance>() {
            let Some(name) = w.get::<Name>(e) else { continue };
            if let Some(p) = self.prefixes.iter().find(|p| name.0.starts_with(p.as_str())) {
                *matched.entry(p.as_str()).or_default() += 1;
                selected.push((e, i.prototype));
            }
        }
        for (p, n) in &matched {
            log.count(&format!("placements named {p}... that stop drawing"), *n);
        }
        let whole = w.resource::<PartScope>().is_none_or(|s| s.whole);
        if whole && let Some((p, _)) = matched.iter().find(|(_, n)| **n == 0) {
            return Err(format!("omit-placements: no placement's name starts with {p:?}"));
        }
        // one collision-only copy per prototype
        let mut copies: BTreeMap<Entity, Entity> = BTreeMap::new();
        for (e, prototype) in selected {
            if self.collision == PlacementCollision::Omit {
                primase_core::ledger::despawn(w, e, log, "placements omitted");
                continue;
            }
            if w.has::<Collision>(e) {
                w.remove::<Instance>(e);
                log.count("placements kept for their own collision", 1);
                continue;
            }
            let Some(collision) = w.get::<Collision>(prototype).cloned() else {
                primase_core::ledger::despawn(w, e, log, "placements with no collision to keep");
                log.count("placements with no collision to keep, despawned", 1);
                continue;
            };
            let copy = match copies.get(&prototype) {
                Some(&c) => c,
                None => {
                    let c = w.spawn();
                    w.insert(c, collision);
                    if let Some(n) = w.get::<Name>(prototype).cloned() {
                        w.insert(c, n);
                    }
                    if let Some(s) = w.get::<Source>(prototype).cloned() {
                        w.insert(c, s);
                    }
                    log.place_from(c, prototype);
                    copies.insert(prototype, c);
                    log.count("collision-only copies of prototypes made", 1);
                    c
                }
            };
            w.insert(
                e,
                Instance {
                    prototype: copy,
                    variant: 0,
                },
            );
            log.count("placements moved to a collision-only copy", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::{Mesh, Primitive, Semantic, Stream, Surfaces, Topology, Transform};

    /// A prototype drawing one triangle and colliding with it, placed as `Level/Ceiling A`, `Level/Ceiling B` and
    /// `Level/Floor`.
    fn world() -> (World, Entity, [Entity; 3]) {
        let mut w = World::new();
        w.insert_resource(Surfaces(vec![primase_core::Surface::new("plaster")]));
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let prototype = w.spawn();
        w.insert(
            prototype,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(positions.clone()))]),
                indices: Some(vec![0, 1, 2]),
                material: None,
            }]),
        );
        w.insert(
            prototype,
            Collision {
                positions,
                triangles: vec![[0, 1, 2]],
                surfaces: vec![0],
            },
        );
        w.insert(prototype, Name("slab".into()));
        let placements = ["Level/Ceiling A", "Level/Ceiling B", "Level/Floor"].map(|name| {
            let e = w.spawn();
            w.insert(e, Transform::IDENTITY);
            w.insert(
                e,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            w.insert(e, Name(name.into()));
            e
        });
        (w, prototype, placements)
    }

    fn run(w: &mut World, pass: OmitPlacements) -> Result<RunReport, String> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_pass_checks(true)
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )
            .map(|()| report)
            .map_err(|e| e.to_string())
    }

    /// Through the pass manager, checks on. keep: the ceilings, sharing the floor's prototype, move to one
    /// collision-only copy and the floor still draws the prototype. omit: the ceilings go. A prefix matching nothing
    /// is refused.
    #[test]
    fn selected_placements_stop_drawing_keeping_or_omitting_collision() {
        let (mut w, prototype, [a, b, floor]) = world();
        let o = Options::from([
            ("prefixes".to_owned(), "Level/Ceiling".to_owned()),
            ("collision".to_owned(), "keep".to_owned()),
        ]);
        let pass = OmitPlacements {
            prefixes: vec!["Level/Ceiling".into()],
            collision: PlacementCollision::Keep,
        };
        assert_eq!(pass.options(), o);
        assert_eq!(OMIT_PLACEMENTS.instance(&o).unwrap().options(), o);
        run(&mut w, pass).unwrap();
        let copy = w.get::<Instance>(a).unwrap().prototype;
        assert_ne!(copy, prototype);
        assert_eq!(w.get::<Instance>(b).unwrap().prototype, copy);
        assert!(w.has::<Collision>(copy) && !w.has::<Mesh>(copy));
        assert_eq!(w.get::<Instance>(floor).unwrap().prototype, prototype);
        assert!(w.has::<Mesh>(prototype));

        let (mut w, prototype, [a, b, floor]) = world();
        run(
            &mut w,
            OmitPlacements {
                prefixes: vec!["Level/Ceiling".into()],
                collision: PlacementCollision::Omit,
            },
        )
        .unwrap();
        assert!(!w.is_alive(a) && !w.is_alive(b));
        assert_eq!(w.get::<Instance>(floor).unwrap().prototype, prototype);

        let (mut w, _, _) = world();
        let err = run(
            &mut w,
            OmitPlacements {
                prefixes: vec!["Level/Ceiling".into(), "Level/Roof".into()],
                collision: PlacementCollision::Keep,
            },
        )
        .unwrap_err();
        assert!(err.contains("Level/Roof"), "{err}");
    }

    /// The placements it despawns are declared as left out (the conservation diagnostics enforced), including when a
    /// kept placement has no collision to keep, and an asset that listed one stops listing it.
    #[test]
    fn despawned_placements_are_declared_and_leave_their_assets() {
        use primase_core::ledger::Conservation;
        use primase_core::{Asset, Role};
        let enforced = |w: &mut World, pass: OmitPlacements| {
            let mut report = RunReport::default();
            PassManager::new(PassRegistry::new())
                .with_pass_checks(true)
                .with_conservation(Conservation::Enforce)
                .run(
                    w,
                    &Pipeline {
                        passes: vec![Box::new(pass)],
                    },
                    &mut report,
                )
                .map(|()| report)
                .map_err(|e| e.to_string())
        };
        let omit = || OmitPlacements {
            prefixes: vec!["Level/Ceiling".into()],
            collision: PlacementCollision::Omit,
        };
        let (mut w, _, [a, b, _]) = world();
        let asset = w.spawn();
        w.insert(
            asset,
            Asset {
                members: vec![(a, Role::Part), (b, Role::Part)],
            },
        );
        enforced(&mut w, omit()).unwrap();
        assert!(!w.is_alive(a) && !w.is_alive(b));
        assert_eq!(w.get::<Asset>(asset).unwrap().members, []);
        // kept for their collision, with none to keep: despawned, and declared
        let (mut w, prototype, [a, b, floor]) = world();
        w.remove::<Collision>(prototype);
        enforced(
            &mut w,
            OmitPlacements {
                prefixes: vec!["Level/Ceiling".into()],
                collision: PlacementCollision::Keep,
            },
        )
        .unwrap();
        assert!(!w.is_alive(a) && !w.is_alive(b) && w.is_alive(floor));
    }
}
