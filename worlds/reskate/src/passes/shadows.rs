//! `lower-reskate-shadows`: shadow casting as core states it (`core.shadows` on a placement, on a prototype for its
//! placements stating none, a placement's own winning) lowered to what skate draws, after `raise-skate`. skate states
//! casting per material only (`skate.material` `cast_shadows`; a map object has no flag), so a placement that casts
//! none (`cast: Off`, whether a reader's fact or the program's choice, `set-shadow-casting`) draws materials that cast
//! none: a material only such placements draw stops casting where it is; one that casting placements draw too is
//! copied (`<key>~noshadow`) for the placements casting none, their prototype drawing the copy in place when only they
//! place it, else through a copy of the prototype (`<id>~noshadow`) they place instead. Each copy is declared as made
//! from its source ([`PassLog::place_from`]) and counted; a prototype to copy that has levels of detail is refused.
//! Each material changed and each copy is reported by name.
//!
//! skate has no shadow proxy: a placement that only casts (`cast: Only`, drawn nowhere) is drawn and casts, reported
//! as an approximation; nor a per-object receive flag: a stated `receive` is reported, not drawn. `core.shadows` is
//! then removed (it is lowered).

use std::collections::{BTreeMap, BTreeSet};

use crate::components::{SkateMaterial, SkatePrototype};
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

/// The materials the placements `quiet` draw made to cast no shadow for them: in place where only they draw a
/// material, else through copies (module documentation).
fn cast_none(w: &mut World, quiet: &BTreeSet<Entity>, log: &mut PassLog) -> Result<(), String> {
    let materials: BTreeSet<Entity> = w.iter::<SkateMaterial>().map(|(e, _)| e).collect();
    // each prototype's placements casting none and the others, and the materials each kind draws
    let mut by_proto: BTreeMap<Entity, (Vec<Entity>, Vec<Entity>)> = BTreeMap::new();
    let (mut inside, mut outside) = (BTreeSet::new(), BTreeSet::new());
    for (e, i) in w.iter::<Instance>() {
        let drawing = drawn(w, i.prototype, i.variant)
            .into_iter()
            .filter(|m| materials.contains(m));
        let entry = by_proto.entry(i.prototype).or_default();
        if quiet.contains(&e) {
            inside.extend(drawing);
            entry.0.push(e);
        } else {
            outside.extend(drawing);
            entry.1.push(e);
        }
    }
    let shared: BTreeSet<Entity> = inside.intersection(&outside).copied().collect();
    // every material that stops casting where it is
    for &m in inside.difference(&shared) {
        let name = name_of(w, m);
        let s = w.get_mut::<SkateMaterial>(m).expect("a material");
        if s.cast_shadows == Some(false) {
            log.count("materials already casting no shadow", 1);
            continue;
        }
        s.cast_shadows = Some(false);
        log.count(&format!("material made to cast no shadow: {name}"), 1);
    }
    if shared.is_empty() {
        return Ok(());
    }
    // the shared ones: a copy casting no shadow, which the quiet placements' prototypes draw
    let keys: BTreeSet<String> = w
        .iter::<SkateMaterial>()
        .map(|(_, m)| m.key.to_lowercase())
        .collect();
    let mut copies = BTreeMap::new();
    for &m in &shared {
        let mut s = w.get::<SkateMaterial>(m).expect("a material").clone();
        s.key = format!("{}~noshadow", s.key);
        if keys.contains(&s.key.to_lowercase()) {
            return Err(format!("lower-reskate-shadows: material key {} is taken", s.key));
        }
        s.cast_shadows = Some(false);
        let c = w.spawn();
        w.copy_components(m, c);
        w.insert(c, s);
        log.place_from(c, m);
        log.count(
            &format!(
                "material copy casting no shadow, for the placements casting none: {}",
                name_of(w, m)
            ),
            1,
        );
        copies.insert(m, c);
    }
    let ids: BTreeSet<String> = w.iter::<SkatePrototype>().map(|(_, p)| p.id.clone()).collect();
    for (proto, (selected, others)) in by_proto {
        let draws_shared = selected.iter().any(|&e| {
            let i = w.get::<Instance>(e).expect("a placement");
            drawn(w, i.prototype, i.variant)
                .iter()
                .any(|m| copies.contains_key(m))
        });
        if !draws_shared {
            continue;
        }
        if others.is_empty() {
            redraw(w, proto, &copies);
            log.count("prototypes drawing the copies in place", 1);
            continue;
        }
        if w.has::<Lods>(proto) {
            return Err(format!(
                "lower-reskate-shadows: prototype {} has levels of detail and would need a copy drawing materials \
                 that cast no shadow: select a level first",
                name_of(w, proto)
            ));
        }
        let copy = w.spawn();
        w.copy_components(proto, copy);
        if let Some(mut p) = w.get::<SkatePrototype>(proto).cloned() {
            p.id = format!("{}~noshadow", p.id);
            if ids.contains(&p.id) {
                return Err(format!("lower-reskate-shadows: prototype id {} is taken", p.id));
            }
            p.key = None;
            w.insert(copy, p);
        }
        if let Some(n) = w.get::<Name>(proto).map(|n| n.0.clone()) {
            w.insert(copy, Name(format!("{n} (casting no shadow)")));
        }
        redraw(w, copy, &copies);
        log.place_from(copy, proto);
        log.count("prototype copies drawing materials that cast no shadow", 1);
        for e in selected {
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
        "shadow-only placements drawn and casting; stated shadow receiving not drawn"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut quiet = BTreeSet::new();
        for (e, i) in w.iter::<Instance>() {
            match cast_of(w, e, i.prototype) {
                Some(ShadowCast::Off) => {
                    quiet.insert(e);
                }
                Some(ShadowCast::Only) => log.approximate(
                    "placements casting shadows only, drawn and casting (skate has no shadow proxy)",
                    1,
                ),
                Some(ShadowCast::On) | None => {}
            }
        }
        log.count("placements casting no shadow", quiet.len() as u64);
        cast_none(w, &quiet, log)?;
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
        assert_eq!(w.get::<Instance>(loud).unwrap().prototype, slab);
        assert_ne!(w.get::<Instance>(quiet).unwrap().prototype, slab);
        assert_eq!(casts(&w, floor), None);
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
