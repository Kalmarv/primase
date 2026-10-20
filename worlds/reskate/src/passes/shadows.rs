//! `lower-reskate-shadows`: shadow casting as core states it (`core.shadows` on a placement, on a prototype for its
//! placements stating none, a placement's own winning) lowered to what skate draws, after `raise-skate`. skate states
//! casting per material only (`skate.material` `cast_shadows`; a map object has no flag), so the materials a placement
//! draws are made to cast as it states, `Off` none and `On` shadows, whether a reader's fact or the program's choice
//! (`set-shadow-casting`). A material only placements wanting one state draw is set in place; one that placements
//! wanting different states draw stays as it is for the placements stating nothing (the writer's default holds for
//! them), or, when all state, serves the first state, and is copied (`<key>~noshadow`, `<key>~shadow`) for each other
//! state; their prototype draws the copies in place when only they place it, else through a copy of the prototype
//! (`<id>~noshadow`, `<id>~shadow`) they are moved to. Each copy is declared as made from its source
//! ([`PassLog::place_from`]) and counted; a prototype to copy that has levels of detail is refused. Each material set
//! and each copy is reported by name.
//!
//! What skate cannot draw is reported, never claimed: a decal or water material (the writer draws those without
//! shadows) asked to cast is left as it is, counted by name as unsupported; there is no shadow proxy, so a placement
//! that only casts (`cast: Only`, drawn nowhere) is drawn with its materials as they are, reported as an
//! approximation; nor a per-object receive flag, so a stated `receive` is reported. `core.shadows` is then removed (it
//! is lowered).

use std::collections::{BTreeMap, BTreeSet};

use crate::components::{MaterialClass, SkateMaterial, SkatePrototype};
use primase_core::pass::{Pass, PassLog};
use primase_core::{
    Component, Entity, Instance, Lods, MaterialVariants, Mesh, MeshFile, Name, ShadowCast, Shadows, World,
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerReskateShadows;

fn name_of(w: &World, e: Entity) -> String {
    w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_else(|| {
        w.get::<SkateMaterial>(e)
            .map_or_else(|| format!("{e:?}"), |m| m.key.clone())
    })
}

/// The materials prototype `p` draws for `variant`: each part's override, else its own (its mesh's, else its file's).
pub(crate) fn drawn(w: &World, p: Entity, variant: u32) -> Vec<Entity> {
    let own: Vec<Option<Entity>> = match (w.get::<Mesh>(p), w.get::<MeshFile>(p)) {
        (Some(m), _) => m.primitives().iter().map(|p| p.material).collect(),
        (None, Some(f)) => f.materials.clone(),
        (None, None) => Vec::new(),
    };
    let variants = w.get::<MaterialVariants>(p);
    own.into_iter()
        .enumerate()
        .filter_map(|(i, m)| {
            variants
                .and_then(|v| v.0.get(i))
                .and_then(|v| v.get(&variant).copied())
                .or(m)
        })
        .collect()
}

/// Prototype `p`'s materials replaced by their copies in `copies`: its mesh's, its file's and its variants'.
fn redraw(w: &mut World, p: Entity, copies: &BTreeMap<Entity, Entity>) {
    let swap = |m: &mut Option<Entity>| {
        if let Some(c) = m.and_then(|e| copies.get(&e)) {
            *m = Some(*c);
        }
    };
    if let Some(mut mesh) = w.get::<Mesh>(p).cloned() {
        for mut part in mesh.primitives_mut() {
            swap(part.material_mut());
        }
        w.insert(p, mesh);
    }
    if let Some(mut f) = w.get::<MeshFile>(p).cloned() {
        f.materials.iter_mut().for_each(swap);
        w.insert(p, f);
    }
    if let Some(mut v) = w.get::<MaterialVariants>(p).cloned() {
        for part in &mut v.0 {
            for m in part.values_mut() {
                if let Some(c) = copies.get(m) {
                    *m = *c;
                }
            }
        }
        w.insert(p, v);
    }
}

/// What placement `e` (of prototype `p`) casts: its own statement, else its prototype's; `None`: not stated (casts).
fn cast_of(w: &World, e: Entity, p: Entity) -> Option<ShadowCast> {
    w.get::<Shadows>(e)
        .and_then(|s| s.cast)
        .or_else(|| w.get::<Shadows>(p).and_then(|s| s.cast))
}

/// The state a placement asks of the materials it draws: `Some(cast)` as stated (`Off`: false, `On`: true), `None`
/// not stated (the material as it is, the writer's default included).
type Wanted = Option<bool>;

/// Whether skate draws material `m` without shadows whatever it states: the writer's decal and water classes.
fn shadowless_class(w: &World, m: Entity) -> bool {
    w.get::<SkateMaterial>(m)
        .is_some_and(|s| matches!(s.class, MaterialClass::Decal | MaterialClass::Water))
}

/// Material `m` (one entity: the original or a copy) set to cast as `cast` for its placements, counted and reported
/// by name; a decal or water material asked to cast is left as it is, reported as unsupported.
fn set_cast(w: &mut World, m: Entity, cast: bool, copy: bool, log: &mut PassLog) {
    let name = name_of(w, m);
    if cast && shadowless_class(w, m) {
        log.omit(
            &format!("decal or water materials asked to cast a shadow, drawn without (skate draws them shadowless): {name}"),
            1,
        );
        return;
    }
    let s = w.get_mut::<SkateMaterial>(m).expect("a material");
    if s.cast_shadows == Some(cast) && !copy {
        log.count("materials already as stated", 1);
        return;
    }
    s.cast_shadows = Some(cast);
    if !copy {
        let what = if cast { "cast shadows" } else { "cast no shadow" };
        log.count(&format!("material made to {what}: {name}"), 1);
    }
}

/// The materials each placement draws made to cast as `wanted` says of it: a material only placements wanting one
/// state draw is set in place; one placements wanting different states draw stays as it is for those not stating
/// (or, when all state, for those wanting the first state) and is copied for each other state, their prototype
/// drawing the copies in place when only they place it, else through a copy of it (module documentation).
fn cast_as(w: &mut World, wanted: &BTreeMap<Entity, bool>, log: &mut PassLog) -> Result<(), String> {
    let materials: BTreeSet<Entity> = w.iter::<SkateMaterial>().map(|(e, _)| e).collect();
    // each prototype's placements by the state they want, and the states each material is wanted in
    let mut by_proto: BTreeMap<Entity, BTreeMap<Wanted, Vec<Entity>>> = BTreeMap::new();
    let mut by_mat: BTreeMap<Entity, BTreeSet<Wanted>> = BTreeMap::new();
    for (e, i) in w.iter::<Instance>() {
        let want = wanted.get(&e).copied();
        for m in drawn(w, i.prototype, i.variant)
            .into_iter()
            .filter(|m| materials.contains(m))
        {
            by_mat.entry(m).or_default().insert(want);
        }
        by_proto
            .entry(i.prototype)
            .or_default()
            .entry(want)
            .or_default()
            .push(e);
    }
    // each material set for the state its original serves, copied for the others
    let keys: BTreeSet<String> = w
        .iter::<SkateMaterial>()
        .map(|(_, m)| m.key.to_lowercase())
        .collect();
    let mut copies: BTreeMap<(Entity, Wanted), Entity> = BTreeMap::new();
    for (&m, states) in &by_mat {
        // the original serves the placements stating nothing, else the first state wanted
        let holder = *states.first().expect("one state at least");
        if let Some(cast) = holder {
            set_cast(w, m, cast, false, log);
        }
        for &state in states.iter().filter(|&&s| s != holder) {
            let Some(cast) = state else { continue };
            if cast && shadowless_class(w, m) {
                // no copy can draw it casting: the placements draw the original, reported once per material
                set_cast(w, m, true, false, log);
                continue;
            }
            let mut s = w.get::<SkateMaterial>(m).expect("a material").clone();
            s.key = format!("{}{}", s.key, if cast { "~shadow" } else { "~noshadow" });
            if keys.contains(&s.key.to_lowercase()) {
                return Err(format!("lower-reskate-shadows: material key {} is taken", s.key));
            }
            let c = w.spawn();
            w.copy_components(m, c);
            w.insert(c, s);
            set_cast(w, c, cast, true, log);
            log.place_from(c, m);
            let what = if cast {
                "casting shadows, for the placements casting"
            } else {
                "casting no shadow, for the placements casting none"
            };
            log.count(&format!("material copy {what}: {}", name_of(w, m)), 1);
            copies.insert((m, state), c);
        }
    }
    if copies.is_empty() {
        return Ok(());
    }
    // each prototype: its placements of one state draw that state's copies, through a copy of the prototype unless
    // they are all its placements or the original already draws what they want
    let ids: BTreeSet<String> = w.iter::<SkatePrototype>().map(|(_, p)| p.id.clone()).collect();
    for (proto, groups) in by_proto {
        let remap = |state: Wanted| -> BTreeMap<Entity, Entity> {
            copies
                .iter()
                .filter(|((_, s), _)| *s == state)
                .map(|((m, _), c)| (*m, *c))
                .collect()
        };
        // the materials this prototype's placements draw (its variants' included)
        let drawing: BTreeSet<Entity> = groups
            .values()
            .flatten()
            .flat_map(|&e| {
                let i = w.get::<Instance>(e).expect("a placement");
                drawn(w, i.prototype, i.variant)
            })
            .collect();
        let needs: Vec<(Wanted, BTreeMap<Entity, Entity>)> = groups
            .keys()
            .map(|&s| {
                let r: BTreeMap<Entity, Entity> = remap(s)
                    .into_iter()
                    .filter(|(m, _)| drawing.contains(m))
                    .collect();
                (s, r)
            })
            .collect();
        let moving: Vec<&(Wanted, BTreeMap<Entity, Entity>)> =
            needs.iter().filter(|(_, r)| !r.is_empty()).collect();
        if moving.is_empty() {
            continue;
        }
        // all its placements want one state: the prototype draws that state's copies itself
        if groups.len() == 1 {
            redraw(w, proto, &moving[0].1);
            log.count("prototypes drawing the copies in place", 1);
            continue;
        }
        if w.has::<Lods>(proto) {
            return Err(format!(
                "lower-reskate-shadows: prototype {} has levels of detail and would need a copy drawing materials \
                 that cast otherwise: select a level first",
                name_of(w, proto)
            ));
        }
        // the group the original keeps: one needing no copy, else the first; every other group needing copies moves
        let stays = needs
            .iter()
            .find(|(_, r)| r.is_empty())
            .map_or(needs[0].0, |(s, _)| *s);
        for (state, r) in needs.iter().filter(|(s, r)| *s != stays && !r.is_empty()) {
            let suffix = if *state == Some(true) {
                "~shadow"
            } else {
                "~noshadow"
            };
            let copy = w.spawn();
            w.copy_components(proto, copy);
            if let Some(mut p) = w.get::<SkatePrototype>(proto).cloned() {
                p.id = format!("{}{suffix}", p.id);
                if ids.contains(&p.id) {
                    return Err(format!("lower-reskate-shadows: prototype id {} is taken", p.id));
                }
                p.key = None;
                w.insert(copy, p);
            }
            if let Some(n) = w.get::<Name>(proto).map(|n| n.0.clone()) {
                let what = if *state == Some(true) {
                    "casting shadows"
                } else {
                    "casting no shadow"
                };
                w.insert(copy, Name(format!("{n} ({what})")));
            }
            redraw(w, copy, r);
            log.place_from(copy, proto);
            let what = if *state == Some(true) {
                "cast shadows"
            } else {
                "cast no shadow"
            };
            log.count(&format!("prototype copies drawing materials that {what}"), 1);
            for &e in &groups[state] {
                let variant = w.get::<Instance>(e).expect("a placement").variant;
                w.insert(
                    e,
                    Instance {
                        prototype: copy,
                        variant,
                    },
                );
            }
        }
        // the group that stays, when it needs copies too (every group did): the original draws them
        if let Some((_, r)) = needs.iter().find(|(s, _)| *s == stays)
            && !r.is_empty()
        {
            redraw(w, proto, r);
            log.count("prototypes drawing the copies in place", 1);
        }
    }
    Ok(())
}

impl Pass for LowerReskateShadows {
    fn name(&self) -> &'static str {
        "lower-reskate-shadows"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            "skate.material",
            "core.instance",
            "core.mesh",
            "core.mesh_file",
            "core.material_variants",
            "core.name",
            "skate.prototype",
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[Shadows::NAME]
    }
    fn approximates(&self) -> &'static str {
        "shadow-only placements drawn with their materials as they are; stated shadow receiving not drawn"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut wanted = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            match cast_of(w, e, i.prototype) {
                Some(ShadowCast::Off) => {
                    wanted.insert(e, false);
                }
                Some(ShadowCast::On) => {
                    wanted.insert(e, true);
                }
                Some(ShadowCast::Only) => log.approximate(
                    "placements casting shadows only, drawn and casting (skate has no shadow proxy)",
                    1,
                ),
                None => {}
            }
        }
        let off = wanted.values().filter(|c| !**c).count() as u64;
        log.count("placements casting no shadow", off);
        log.count("placements stated casting", wanted.len() as u64 - off);
        cast_as(w, &wanted, log)?;
        let stated: Vec<Entity> = w.iter::<Shadows>().map(|(e, _)| e).collect();
        for e in stated {
            if w.get::<Shadows>(e).is_some_and(|s| s.receive.is_some()) {
                log.approximate("stated shadow receiving not drawn (skate has no receive flag)", 1);
            }
            w.remove::<Shadows>(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::MaterialClass;
    use primase_core::ledger::Conservation;
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use primase_core::{Primitive, Semantic, Stream, Topology, Transform};

    fn material(w: &mut World, name: &str) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            SkateMaterial {
                key: name.into(),
                class: MaterialClass::Opaque,
                base: None,
                normal: None,
                mask: None,
                base_color: None,
                tint: None,
                alpha: None,
                cutoff: None,
                two_sided: None,
                domain: None,
                metallic: None,
                roughness: None,
                wrap: None,
                ocean: None,
                cast_shadows: None,
                emissive: None,
                vertex_alpha: None,
                other: Default::default(),
            },
        );
        e
    }

    /// A one-triangle prototype `id` drawn with `m`.
    fn prototype(w: &mut World, id: &str, m: Entity) -> Entity {
        let p = w.spawn();
        w.insert(p, Name(id.into()));
        w.insert(
            p,
            SkatePrototype {
                id: id.into(),
                glb: None,
                key: Some(7),
                foliage: None,
                ground: None,
                collision: None,
                other: Default::default(),
            },
        );
        w.insert(
            p,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                )]),
                indices: None,
                material: Some(m),
            }]),
        );
        p
    }

    fn place(w: &mut World, name: &str, prototype: Entity) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(e, Transform::IDENTITY);
        e
    }

    fn run(w: &mut World) -> Result<PassLog, String> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_conservation(Conservation::Enforce)
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(LowerReskateShadows)],
                },
                &mut report,
            )
            .map_err(|e| e.to_string())?;
        Ok(report.steps[0].log.clone())
    }

    fn casts(w: &World, m: Entity) -> Option<bool> {
        w.get::<SkateMaterial>(m).unwrap().cast_shadows
    }

    fn off(w: &mut World, e: Entity) {
        w.insert(
            e,
            Shadows {
                cast: Some(ShadowCast::Off),
                receive: None,
            },
        );
    }

    /// A room shell casting no shadow: its wall material drawn by the shell only, its floor material shared with a
    /// table that casts.
    struct Room {
        w: World,
        wall: Entity,
        floor: Entity,
        shell: Entity,
        slab: Entity,
        table: Entity,
    }

    fn room() -> Room {
        let mut w = World::new();
        let (wall, floor) = (material(&mut w, "wall"), material(&mut w, "floor"));
        let walls = prototype(&mut w, "walls", wall);
        let slab = prototype(&mut w, "slab", floor);
        let walls_at = place(&mut w, "Level Elements/Bounds/Walls", walls);
        let shell = place(&mut w, "Level Elements/Bounds/Floor", slab);
        let table = place(&mut w, "Level Elements/Thin Table", slab);
        off(&mut w, walls_at);
        off(&mut w, shell);
        Room {
            w,
            wall,
            floor,
            shell,
            slab,
            table,
        }
    }

    /// The shell's own material stops casting; the shared one is copied for the shell, whose prototype (placed by the
    /// table too) is copied to draw it, both declared, the shell placement moved to the copy; the table keeps the
    /// casting original; core.shadows is lowered away.
    #[test]
    fn placements_casting_none_draw_materials_casting_none() {
        let Room {
            mut w,
            wall,
            floor,
            shell,
            slab,
            table,
        } = room();
        let log = run(&mut w).unwrap();
        assert_eq!((casts(&w, wall), casts(&w, floor)), (Some(false), None));
        assert_eq!(w.get::<Instance>(table).unwrap().prototype, slab);
        let copy = w.get::<Instance>(shell).unwrap().prototype;
        assert_ne!(copy, slab);
        let drawn = w.get::<Mesh>(copy).unwrap().primitives()[0].material.unwrap();
        let m = w.get::<SkateMaterial>(drawn).unwrap();
        assert_eq!((m.key.as_str(), m.cast_shadows), ("floor~noshadow", Some(false)));
        let p = w.get::<SkatePrototype>(copy).unwrap();
        assert_eq!((p.id.as_str(), p.key), ("slab~noshadow", None));
        assert_eq!(log.placed_from.get(&copy), Some(&slab));
        assert_eq!(log.placed_from.get(&drawn), Some(&floor));
        assert_eq!(log.counts["material made to cast no shadow: wall"], 1);
        assert_eq!(log.counts["placements casting no shadow"], 2);
        assert_eq!(w.iter::<Shadows>().count(), 0);
    }

    /// A prototype's statement holds for its placements stating none; a placement's own wins.
    #[test]
    fn a_placements_own_statement_wins_over_its_prototypes() {
        let mut w = World::new();
        let floor = material(&mut w, "floor");
        let slab = prototype(&mut w, "slab", floor);
        off(&mut w, slab);
        let quiet = place(&mut w, "a", slab);
        let loud = place(&mut w, "b", slab);
        w.insert(
            loud,
            Shadows {
                cast: Some(ShadowCast::On),
                receive: None,
            },
        );
        let log = run(&mut w).unwrap();
        assert_eq!(log.counts["placements casting no shadow"], 1);
        assert_eq!(log.counts["placements stated casting"], 1);
        // both state: the original serves the first state (none), the other placement draws a copy that casts
        assert_eq!(w.get::<Instance>(quiet).unwrap().prototype, slab);
        assert_ne!(w.get::<Instance>(loud).unwrap().prototype, slab);
        assert_eq!(casts(&w, drawn_by(&w, quiet)), Some(false));
        assert_eq!(casts(&w, drawn_by(&w, loud)), Some(true));
        assert_eq!(casts(&w, floor), Some(false));
    }

    /// The material placement `e` draws.
    fn drawn_by(w: &World, e: Entity) -> Entity {
        let i = w.get::<Instance>(e).unwrap();
        drawn(w, i.prototype, i.variant)[0]
    }

    fn on(w: &mut World, e: Entity) {
        w.insert(
            e,
            Shadows {
                cast: Some(ShadowCast::On),
                receive: None,
            },
        );
    }

    /// On is lowered too: a material stated not to cast is switched on for a placement stating On; a material shared
    /// by a placement wanting On and one wanting Off is copied, each drawing its state, and one stating nothing keeps
    /// the original as it was.
    #[test]
    fn on_and_off_are_both_lowered() {
        let mut w = World::new();
        let wall = material(&mut w, "wall");
        w.get_mut::<SkateMaterial>(wall).unwrap().cast_shadows = Some(false);
        let walls = prototype(&mut w, "walls", wall);
        let lit = place(&mut w, "lit", walls);
        on(&mut w, lit);
        let log = run(&mut w).unwrap();
        assert_eq!(casts(&w, wall), Some(true));
        assert_eq!(log.counts["material made to cast shadows: wall"], 1);
        // shared: On, Off and unstated placements of one prototype
        let mut w = World::new();
        let floor = material(&mut w, "floor");
        let slab = prototype(&mut w, "slab", floor);
        let (a, b, c) = (
            place(&mut w, "a", slab),
            place(&mut w, "b", slab),
            place(&mut w, "c", slab),
        );
        on(&mut w, a);
        off(&mut w, b);
        let log = run(&mut w).unwrap();
        assert_eq!(casts(&w, drawn_by(&w, a)), Some(true));
        assert_eq!(casts(&w, drawn_by(&w, b)), Some(false));
        assert_eq!(drawn_by(&w, c), floor);
        assert_eq!(
            casts(&w, floor),
            None,
            "the unstated placement's material is as it was"
        );
        assert_eq!(w.get::<Instance>(c).unwrap().prototype, slab);
        let keys: BTreeSet<String> = [a, b]
            .iter()
            .map(|&e| w.get::<SkateMaterial>(drawn_by(&w, e)).unwrap().key.clone())
            .collect();
        assert_eq!(
            keys,
            ["floor~noshadow".to_owned(), "floor~shadow".to_owned()].into()
        );
        assert_eq!(
            log.counts["prototype copies drawing materials that cast shadows"],
            1
        );
        assert_eq!(
            log.counts["prototype copies drawing materials that cast no shadow"],
            1
        );
    }

    /// A decal placement stating On: skate draws decals without shadows, so the material is left as it is and the
    /// case reported as unsupported, never claimed lowered.
    #[test]
    fn a_decal_asked_to_cast_is_reported_not_lowered() {
        let mut w = World::new();
        let paint = material(&mut w, "paint");
        w.get_mut::<SkateMaterial>(paint).unwrap().class = MaterialClass::Decal;
        let decal = prototype(&mut w, "decal", paint);
        let p = place(&mut w, "stripe", decal);
        on(&mut w, p);
        let log = run(&mut w).unwrap();
        assert_eq!(casts(&w, paint), None);
        assert_eq!(
            log.omitted["decal or water materials asked to cast a shadow, drawn without (skate draws them shadowless): paint"],
            1
        );
        assert!(
            !log.counts
                .keys()
                .any(|k| k.starts_with("material made to cast shadows"))
        );
    }

    /// A prototype only placements casting none place draws the copy in place; a shadow-only placement is reported,
    /// drawn and casting.
    #[test]
    fn in_place_and_shadow_only() {
        let mut w = World::new();
        let floor = material(&mut w, "floor");
        let shell_slab = prototype(&mut w, "shell slab", floor);
        let table_slab = prototype(&mut w, "table slab", floor);
        let shell = place(&mut w, "Bounds/Floor", shell_slab);
        off(&mut w, shell);
        let table = place(&mut w, "Table", table_slab);
        w.insert(
            table,
            Shadows {
                cast: Some(ShadowCast::Only),
                receive: Some(false),
            },
        );
        let log = run(&mut w).unwrap();
        assert_eq!(w.get::<Instance>(shell).unwrap().prototype, shell_slab);
        let drawn = w.get::<Mesh>(shell_slab).unwrap().primitives()[0]
            .material
            .unwrap();
        assert_eq!(casts(&w, drawn), Some(false));
        assert_eq!(
            w.get::<Mesh>(table_slab).unwrap().primitives()[0].material,
            Some(floor)
        );
        assert_eq!(log.counts["prototypes drawing the copies in place"], 1);
        assert_eq!(
            log.approximations["placements casting shadows only, drawn and casting (skate has no shadow proxy)"],
            1
        );
        assert_eq!(
            log.approximations["stated shadow receiving not drawn (skate has no receive flag)"],
            1
        );
    }
}
