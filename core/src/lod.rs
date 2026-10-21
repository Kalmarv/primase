//! A prototype's levels of detail ([`Lods`], `core.lods`) and the pass that chooses one (`select-lod`). The forms
//! stay in the world as the source authored them; a writer sees the one form a pipeline's pass leaves it, so no
//! writer chooses on its own.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::pass::{Options, Pass, PassLog};
use crate::{Component, Entity, Instance, JsonError, MaterialVariants, Mesh, MeshFile, Transform, World};

/// A prototype's levels of detail (`core.lods`), on the prototype. Its own mesh is one form, at level `detail` (0 the
/// finest, larger coarser: GTA's 0 high to 3 very low); `forms` are its other forms, as the source lists them
/// (finest first by convention; nothing depends on it).
///
/// A form is an entity of its own that holds everything the prototype holds for its mesh: `core.mesh_file` or
/// `core.mesh`, `core.material_variants` when it has variants, and every dialect component that goes with that mesh
/// (a reader that attaches per-primitive data to a prototype attaches it to each form too). A form with neither mesh
/// component draws nothing. Forms are not placed and have no forms of their own; what stays with the prototype
/// whichever form is drawn (its name, collision, identity) is on the prototype only.
///
/// `default` is the form the source itself takes: 0 the prototype's own mesh, `k` `forms[k - 1]`; `None` when the
/// source authors none (GTA switches between its levels by distance).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Lods {
    pub detail: u8,
    pub forms: Vec<LodForm>,
    pub default: Option<usize>,
}

/// One other form of a prototype: its level and the entity holding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LodForm {
    pub detail: u8,
    pub form: Entity,
}

impl Lods {
    /// The levels of [own mesh, forms...], in that order.
    pub fn levels(&self) -> impl Iterator<Item = u8> + '_ {
        std::iter::once(self.detail).chain(self.forms.iter().map(|f| f.detail))
    }

    /// The form `choice` takes, as an index into [own mesh, forms...] (the reference skate writer's rule for
    /// [`LodChoice::AtMost`]); [`LodChoice::Default`] without an authored default takes the finest.
    pub fn choose(&self, choice: LodChoice) -> usize {
        match choice {
            LodChoice::Finest => 0,
            LodChoice::Default => self.default.unwrap_or(0),
            LodChoice::AtMost(n) => {
                let mut best: Option<(usize, u8)> = None;
                for (i, d) in self.levels().enumerate() {
                    if d <= n && best.is_none_or(|(_, b)| d > b) {
                        best = Some((i, d));
                    }
                }
                best.map_or(0, |(i, _)| i)
            }
        }
    }
}

impl Component for Lods {
    const NAME: &'static str = "core.lods";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// The forms are entities of their own (a pass mapping a prototype maps its forms itself).
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.forms.iter().map(|f| f.form));
    }
    /// `default`, when there is one, one of the forms; with other forms, on an entity with a mesh; each form another
    /// alive entity, listed once, not placed and without forms of its own. [`SelectLod`] checks it before it changes
    /// anything, so it checks liveness itself, beside the world's check of every reference.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if let Some(d) = self.default
            && d > self.forms.len()
        {
            return Err(format!("default form {d} of {}", self.forms.len() + 1));
        }
        if !self.forms.is_empty() && !(w.has::<Mesh>(e) || w.has::<MeshFile>(e)) {
            return Err("other forms of an entity without a mesh".into());
        }
        let mut seen = BTreeSet::new();
        for f in &self.forms {
            if f.form == e || !w.is_alive(f.form) {
                return Err(format!("form {:?} is not another alive entity", f.form));
            }
            if !seen.insert(f.form) {
                return Err(format!("form {:?} is listed twice", f.form));
            }
            if w.has::<Instance>(f.form) || w.has::<Transform>(f.form) || w.has::<Lods>(f.form) {
                return Err(format!("form {:?} is placed or has forms of its own", f.form));
            }
        }
        Ok(())
    }
}

/// Which form [`SelectLod`] takes of each prototype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodChoice {
    /// The prototype's own mesh.
    Finest,
    /// [`Lods::default`], the source's own choice; the finest where it authored none (a named, counted fallback).
    Default,
    /// The coarsest form at level `N` or finer (the first listed of equal levels); the finest when none is.
    AtMost(u8),
}

/// `select-lod{form=finest|default|N}` (no options: `default`): every prototype with [`Lods`] is left with the one
/// form [`Lods::choose`] takes (with `default`, the finest where the source authored no default, counted as an
/// approximation). Another form than its own takes the prototype's own geometry away with everything that describes
/// it ([`World::remove_mesh`]: `core.mesh`, `core.mesh_file` and every per-primitive component, dialect data
/// included, so nothing describing the discarded form stays behind when the chosen form lacks it), then every
/// component the form holds replaces the prototype's of that name ([`World::move_components`]), so the form's
/// dialect data travels with it without the pass knowing it; a form that draws nothing leaves the prototype without a
/// mesh. The other forms are despawned and `core.lods` removed. The whole selection is checked before anything
/// changes: an invalid one (a prototype drawn with another form while it holds an opaque component, which can't be
/// told to follow, among them) is an error and leaves the world as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectLod {
    pub choice: LodChoice,
}

impl Default for SelectLod {
    fn default() -> Self {
        SelectLod {
            choice: LodChoice::Default,
        }
    }
}

/// The pass factory (`PassRegistry::register("select-lod", select_lod)`).
pub fn select_lod(o: &Options) -> Result<Box<dyn Pass>, String> {
    let mut pass = SelectLod::default();
    for (k, v) in o {
        match k.as_str() {
            "form" => {
                pass.choice = match v.as_str() {
                    "finest" => LodChoice::Finest,
                    "default" => LodChoice::Default,
                    n => LodChoice::AtMost(
                        n.parse()
                            .map_err(|_| format!("form={n} is not finest, default or a level 0-255"))?,
                    ),
                }
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(pass))
}

impl Pass for SelectLod {
    fn name(&self) -> &'static str {
        "select-lod"
    }
    fn options(&self) -> Options {
        let form = match self.choice {
            LodChoice::Finest => "finest".to_owned(),
            LodChoice::Default => return Options::new(),
            LodChoice::AtMost(n) => n.to_string(),
        };
        Options::from([("form".to_owned(), form)])
    }
    /// A prototype drawn with another form loses its own mesh and variants (and keeps the form's).
    fn removes(&self) -> &[&'static str] {
        &[Lods::NAME, Mesh::NAME, MeshFile::NAME, MaterialVariants::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, MeshFile::NAME, MaterialVariants::NAME]
    }
    /// It lowers the levels of detail; the meshes and variants it removes are the forms it discards.
    fn legalizes(&self) -> &[&'static str] {
        &[Lods::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    /// The per-primitive data of a prototype's own form ([`World::remove_mesh`]) is whatever its reader attached; each
    /// name taken from a prototype the chosen form doesn't give back is recorded.
    fn removes_by_log(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        match self.choice {
            LodChoice::Default => "the finest form where the source authored no default",
            _ => "",
        }
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, Lods)> = w.iter::<Lods>().map(|(e, l)| (e, l.clone())).collect();
        let mut forms = BTreeSet::new();
        for (proto, lods) in &all {
            lods.verify(*proto, w).map_err(|m| format!("{proto:?}: {m}"))?;
            for f in &lods.forms {
                if !forms.insert(f.form) {
                    return Err(format!("form {:?} belongs to more than one prototype", f.form));
                }
            }
            if lods.choose(self.choice) != 0 {
                w.check_remove_mesh(*proto)?;
            }
        }
        if let Some((name, holder, form)) = w
            .references_to(&forms)
            .into_iter()
            .find(|(name, _, _)| *name != Lods::NAME)
        {
            return Err(format!("{name} on {holder:?} refers to form {form:?}"));
        }
        for (proto, lods) in all {
            let chosen = lods.choose(self.choice);
            if self.choice == LodChoice::Default && lods.default.is_none() {
                log.approximate("prototypes without an authored default: the finest form", 1);
            }
            match chosen.checked_sub(1).map(|k| lods.forms[k]) {
                None => log.count("prototypes drawn with their own form", 1),
                Some(f) => {
                    if let Some(own) = w.get::<Mesh>(proto) {
                        let n = own.primitives().len() as u64;
                        log.dropped(
                            crate::ledger::Measure::Primitives,
                            "the prototype's own form left out",
                            n,
                        );
                    }
                    for name in w.remove_mesh(proto)? {
                        log.record_removed(name);
                    }
                    let moved = w.move_components(f.form, proto);
                    debug_assert!(moved, "checked alive above");
                    let draws = w.has::<Mesh>(proto) || w.has::<MeshFile>(proto);
                    log.count(
                        if draws {
                            "prototypes drawn with another form"
                        } else {
                            "prototypes whose chosen form draws nothing"
                        },
                        1,
                    );
                }
            }
            for (k, f) in lods.forms.iter().enumerate() {
                if k + 1 != chosen {
                    crate::ledger::despawn(w, f.form, log, "forms left out");
                    log.count("forms left out", 1);
                }
            }
            w.remove::<Lods>(proto);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::{PassManager, PassRegistry, RunReport};
    use crate::{Name, Opaque, OpaqueData};

    /// A prototype at level 0 with two other forms (level 1 from a file, level 3 drawing nothing), default the
    /// level-1 form; each form names itself and the coarse one carries opaque game data, so a test sees what moved.
    fn world() -> (World, Entity, [Entity; 2]) {
        let mut w = World::new();
        let proto = w.spawn();
        w.insert(proto, Mesh::default());
        w.insert(proto, Name("finest".into()));
        let coarse = w.spawn();
        w.insert(
            coarse,
            MeshFile {
                path: "p.glb".into(),
                mesh: 1,
                materials: vec![],
            },
        );
        w.insert(coarse, Name("coarse".into()));
        w.insert_opaque(
            coarse,
            "game.form",
            Opaque {
                version: 1,
                data: OpaqueData::Json(serde_json::json!({"lod": 1})),
            },
        );
        let nothing = w.spawn();
        w.insert(nothing, Name("nothing".into()));
        w.insert(
            proto,
            Lods {
                detail: 0,
                forms: vec![
                    LodForm {
                        detail: 1,
                        form: coarse,
                    },
                    LodForm {
                        detail: 3,
                        form: nothing,
                    },
                ],
                default: Some(1),
            },
        );
        (w, proto, [coarse, nothing])
    }

    fn manager() -> PassManager {
        let mut r = PassRegistry::new();
        r.register("select-lod", select_lod);
        PassManager::new(r)
    }

    #[test]
    fn select_lod_refuses_a_dead_form_before_it_changes_anything() {
        // run, unlike convert, does not verify its input: the pass checks its forms itself
        let (mut w, proto, forms) = world();
        w.despawn(forms[0]);
        let m = manager();
        let err = m
            .run(
                &mut w,
                &m.registry.parse("select-lod").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("not another alive entity"), "{err}");
        assert!(w.has::<Mesh>(proto), "the prototype keeps its geometry");
        assert!(w.has::<Lods>(proto) && w.is_alive(forms[1]));
    }

    #[test]
    fn lods_verify_and_keep_their_forms_in_closures_and_subsets() {
        let (w, proto, forms) = world();
        assert_eq!(w.verify_components(), vec![]);
        let keep = w.closure([proto]);
        assert_eq!(keep, [proto, forms[0], forms[1]].into());
        assert_eq!(w.subset(&keep).verify_components(), vec![]);
    }

    #[test]
    fn choice_follows_the_reference_rule() {
        let lods = |detail: u8, levels: &[u8], default: Option<usize>| Lods {
            detail,
            forms: levels
                .iter()
                .enumerate()
                .map(|(i, &d)| LodForm {
                    detail: d,
                    form: Entity::from_index(10 + i as u32),
                })
                .collect(),
            default,
        };
        let l = lods(0, &[1, 1, 3], Some(2));
        assert_eq!(l.choose(LodChoice::Finest), 0);
        assert_eq!(l.choose(LodChoice::Default), 2);
        // no authored default: the finest
        assert_eq!(lods(0, &[1, 3], None).choose(LodChoice::Default), 0);
        // coarsest at most N; of equal levels the first listed; none at most N: the finest
        assert_eq!(l.choose(LodChoice::AtMost(0)), 0);
        assert_eq!(l.choose(LodChoice::AtMost(2)), 1);
        assert_eq!(l.choose(LodChoice::AtMost(255)), 3);
        assert_eq!(lods(2, &[3], None).choose(LodChoice::AtMost(1)), 0);
        // the order does not matter to the levels
        assert_eq!(lods(0, &[3, 1], None).choose(LodChoice::AtMost(2)), 2);
    }

    #[test]
    fn default_without_an_authored_default_takes_the_finest_named_and_counted() {
        let (mut w, proto, forms) = world();
        w.get_mut::<Lods>(proto).unwrap().default = None;
        assert_eq!(w.verify_components(), vec![]);
        let m = manager();
        let mut report = RunReport::default();
        m.run(&mut w, &m.registry.parse("select-lod").unwrap(), &mut report)
            .unwrap();
        assert_eq!(w.get::<Name>(proto).unwrap().0, "finest");
        assert!(forms.iter().all(|f| !w.is_alive(*f)));
        let step = &report.steps[0];
        assert_eq!(
            step.log.approximations,
            [(
                "prototypes without an authored default: the finest form".to_owned(),
                1
            )]
            .into()
        );
        assert_eq!(
            step.approximates,
            "the finest form where the source authored no default"
        );
        // an authored default is no approximation; neither are the other choices
        let (mut w, _, _) = world();
        let mut report = RunReport::default();
        m.run(&mut w, &m.registry.parse("select-lod").unwrap(), &mut report)
            .unwrap();
        assert!(report.steps[0].log.approximations.is_empty());
        assert_eq!(
            SelectLod {
                choice: LodChoice::Finest
            }
            .approximates(),
            ""
        );
    }

    #[test]
    fn select_lod_takes_one_form_with_everything_it_holds() {
        for (pipeline, name, mesh, file, opaque) in [
            ("select-lod{form=finest}", "finest", true, false, false),
            ("select-lod", "coarse", false, true, true),
            ("select-lod{form=2}", "coarse", false, true, true),
            ("select-lod{form=3}", "nothing", false, false, false),
        ] {
            let (mut w, proto, forms) = world();
            let m = manager();
            let mut report = RunReport::default();
            m.run(&mut w, &m.registry.parse(pipeline).unwrap(), &mut report)
                .unwrap();
            assert_eq!(w.get::<Name>(proto).unwrap().0, name, "{pipeline}");
            assert_eq!(
                (w.has::<Mesh>(proto), w.has::<MeshFile>(proto)),
                (mesh, file),
                "{pipeline}"
            );
            assert_eq!(w.get_opaque(proto, "game.form").is_some(), opaque, "{pipeline}");
            assert!(!w.has::<Lods>(proto));
            assert!(forms.iter().all(|f| !w.is_alive(*f)), "{pipeline}: a form left");
            assert_eq!(w.entity_count(), 1);
            let step = &report.steps[0];
            let left_out = if name == "finest" { 2 } else { 1 };
            assert_eq!(step.log.counts["forms left out"], left_out, "{pipeline}");
            // the discarded forms' dialect data goes with them, undeclared
            let mut removed = vec!["core.lods"];
            removed.extend((!mesh).then_some("core.mesh"));
            removed.extend((!file).then_some("core.mesh_file"));
            removed.extend((!opaque).then_some("game.form"));
            assert_eq!(step.removed, removed, "{pipeline}");
        }
        assert_eq!(
            select_lod(&Options::from([("form".into(), "2".into())]))
                .unwrap()
                .options()["form"],
            "2"
        );
        assert!(select_lod(&Options::new()).unwrap().options().is_empty());
        assert!(select_lod(&Options::from([("form".into(), "high".into())])).is_err());
        assert!(select_lod(&Options::from([("lod".into(), "1".into())])).is_err());
    }

    /// Dialect data per primitive of its entity's mesh, as a reader attaches it.
    #[derive(Clone, Debug, PartialEq)]
    struct PerPrimitive(Vec<u8>);
    impl Component for PerPrimitive {
        const NAME: &'static str = "game.per_primitive";
        const GEOMETRY: crate::Geometry = crate::Geometry::PerPrimitive;
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(crate::remap_list(&self.0, from).map(PerPrimitive))
        }
    }

    /// The prototype's own form takes its per-primitive data with it when another form is drawn, whether its mesh is
    /// loaded or still only in its file (a world before its load): nothing describing the discarded form stays on the
    /// prototype when the chosen form has no such data of its own.
    #[test]
    fn the_own_forms_per_primitive_data_leaves_with_it() {
        for loaded in [true, false] {
            let (mut w, proto, _) = world();
            if !loaded {
                w.remove::<Mesh>(proto);
                w.insert(
                    proto,
                    MeshFile {
                        path: "own.glb".into(),
                        mesh: 0,
                        materials: vec![],
                    },
                );
            }
            w.insert(proto, PerPrimitive(vec![]));
            w.insert(proto, MaterialVariants(vec![]));
            let m = manager().with_pass_checks(true);
            let mut report = RunReport::default();
            m.run(&mut w, &m.registry.parse("select-lod").unwrap(), &mut report)
                .unwrap();
            assert_eq!(w.get::<Name>(proto).unwrap().0, "coarse");
            assert!(
                !w.has::<PerPrimitive>(proto) && !w.has::<MaterialVariants>(proto),
                "loaded {loaded}: the own form's per-primitive data stayed"
            );
            assert_eq!(
                w.get::<MeshFile>(proto).unwrap().path,
                std::path::Path::new("p.glb")
            );
            let removed = &report.steps[0].removed;
            for name in [PerPrimitive::NAME, MaterialVariants::NAME] {
                assert!(removed.iter().any(|n| n == name), "loaded {loaded}: {removed:?}");
            }
        }
        // the own form drawn: its data stays
        let (mut w, proto, _) = world();
        w.insert(proto, PerPrimitive(vec![]));
        let m = manager().with_pass_checks(true);
        m.run(
            &mut w,
            &m.registry.parse("select-lod{form=finest}").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
        assert!(w.has::<PerPrimitive>(proto) && w.has::<Mesh>(proto));
    }

    /// An opaque component on a prototype drawn with another form can't be told to follow its own form's removal: the
    /// selection is refused before anything changes. Drawn with its own form, nothing is removed and it passes.
    #[test]
    fn an_opaque_component_on_a_prototype_drawn_with_another_form_is_refused() {
        let opaque = || Opaque {
            version: 1,
            data: OpaqueData::Bytes(vec![1]),
        };
        let (mut w, proto, forms) = world();
        w.insert_opaque(proto, "game.thing", opaque());
        let m = manager().with_pass_checks(true);
        let err = m
            .run(
                &mut w,
                &m.registry.parse("select-lod").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("game.thing"), "{err}");
        assert!(w.has::<Mesh>(proto) && w.has::<Lods>(proto));
        assert!(forms.iter().all(|f| w.is_alive(*f)));
        assert_eq!(w.get::<Name>(proto).unwrap().0, "finest");
        m.run(
            &mut w,
            &m.registry.parse("select-lod{form=finest}").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
        assert!(w.get_opaque(proto, "game.thing").is_some() && w.has::<Mesh>(proto));
    }

    #[test]
    fn an_invalid_selection_is_refused_without_mutation() {
        // an out-of-range default, also on a prototype after a valid one
        let (mut w, proto, forms) = world();
        let second = w.spawn();
        w.insert(second, Mesh::default());
        w.insert(
            second,
            Lods {
                detail: 0,
                forms: vec![],
                default: Some(99),
            },
        );
        let err = SelectLod::default()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("default form 99 of 1"), "{err}");
        assert_eq!(w.get::<Name>(proto).unwrap().0, "finest");
        assert!(forms.iter().all(|e| w.is_alive(*e)) && w.has::<Lods>(second));
        // a form shared by two prototypes
        w.insert(second, w.get::<Lods>(proto).unwrap().clone());
        let err = SelectLod::default()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("more than one prototype"), "{err}");
        // a form something else refers to
        w.remove::<Lods>(second);
        let placed = w.spawn();
        w.insert(
            placed,
            Instance {
                prototype: forms[0],
                variant: 0,
            },
        );
        let err = SelectLod::default()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(
            err.contains("core.instance") && err.contains("refers to form"),
            "{err}"
        );
        assert!(forms.iter().all(|e| w.is_alive(*e)));
        assert_eq!(w.get::<Name>(proto).unwrap().0, "finest");
    }

    #[test]
    fn malformed_lods_fail_verification() {
        let failure = |edit: &dyn Fn(&mut World, Entity, [Entity; 2])| {
            let (mut w, proto, forms) = world();
            edit(&mut w, proto, forms);
            w.verify_components()
                .into_iter()
                .map(|(_, _, m)| m)
                .collect::<Vec<_>>()
        };
        let twice = failure(&|w, proto, _| {
            let lods = w.get_mut::<Lods>(proto).unwrap();
            lods.forms[1] = lods.forms[0];
        });
        assert!(twice[0].contains("listed twice"), "{twice:?}");
        let placed = failure(&|w, _, forms| {
            w.insert(forms[0], Transform::IDENTITY);
        });
        assert!(placed[0].contains("placed"), "{placed:?}");
        let meshless = failure(&|w, proto, _| {
            w.remove::<Mesh>(proto);
        });
        assert!(meshless[0].contains("without a mesh"), "{meshless:?}");
        let gone = failure(&|w, _, forms| {
            w.despawn(forms[1]);
        });
        assert!(gone[0].contains("not another alive entity"), "{gone:?}");
    }
}
