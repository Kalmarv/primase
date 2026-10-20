//! `set-reskate-shadow-casting{materials=<name>;...,placements=<prefix>;...,shared=copy|refuse}`: materials that cast
//! no shadow (`skate.material` `cast_shadows` false, which the writer honours), after `raise-skate`. An indoor level's
//! room shell (SkateBIRD's `Level Elements/Bounds/`) shadows the whole room from the sun and sky; drawn without casting,
//! the sun and sky light the inside. The selection is the program's, at least one of:
//!
//! - `materials`: the materials of these names (a material's `core.name`, else its key), wherever they are drawn;
//! - `placements`: the materials drawn by the placements whose name starts with one of these prefixes (a placement's
//!   prototype's materials, its variant's where it has one). A placement's name is its reader's: the Unity reader's is
//!   the GameObject path joined by a bare `/` (`Level Elements/Bounds/Walls`), so a prefix ending in `/` selects a
//!   subtree.
//!
//! A name that matches no material, or a prefix that matches no placement, is an error naming it: a selection that
//! selects nothing is a mistake, not an empty change.
//!
//! A material the selected placements draw that a placement outside the selection draws too (and no name selects) is
//! the required `shared` choice: `refuse` is an error naming it; `copy` gives the selected placements a copy that casts
//! no shadow (`<key>~noshadow`), leaving the others casting. Their prototype draws the copy: in place when only
//! selected placements place it, else through a copy of the prototype (`<id>~noshadow`) that the selected placements
//! place instead. Each copy is declared as made from its source ([`PassLog::place_from`]) and counted; a prototype to
//! copy that has levels of detail is refused. Each material changed and each copy is reported by name.

use std::collections::{BTreeMap, BTreeSet};

use crate::components::{SkateMaterial, SkatePrototype};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, Lods, MaterialVariants, Mesh, MeshFile, Name, World};

/// What becomes of a selected placement's material that other placements draw too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedMaterials {
    /// The selected placements draw a copy that casts no shadow.
    Copy,
    /// An error naming it.
    Refuse,
}

/// See the module documentation. Made by [`SetReskateShadowCasting::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetReskateShadowCasting {
    materials: BTreeSet<String>,
    placements: BTreeSet<String>,
    shared: SharedMaterials,
}

impl SetReskateShadowCasting {
    /// The materials named `materials` and those drawn by placements named with a prefix in `placements` (at least one
    /// of the two non-empty, no empty name or prefix) cast no shadow; `shared` as the module documentation says.
    pub fn new(
        materials: BTreeSet<String>,
        placements: BTreeSet<String>,
        shared: SharedMaterials,
    ) -> Result<SetReskateShadowCasting, String> {
        if materials.is_empty() && placements.is_empty() {
            return Err("no selection: name materials, placements (by name prefix), or both".into());
        }
        if materials.iter().chain(&placements).any(String::is_empty) {
            return Err("an empty material name or placement prefix".into());
        }
        Ok(SetReskateShadowCasting {
            materials,
            placements,
            shared,
        })
    }

    pub fn materials(&self) -> &BTreeSet<String> {
        &self.materials
    }

    pub fn placements(&self) -> &BTreeSet<String> {
        &self.placements
    }

    pub fn shared(&self) -> SharedMaterials {
        self.shared
    }

    /// From its options: `materials` (names separated by `;`) and/or `placements` (name prefixes separated by `;`),
    /// and `shared=copy|refuse`, required.
    pub fn from_options(o: &Options) -> Result<SetReskateShadowCasting, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "materials" | "placements" | "shared"))
        {
            return Err(format!("unknown option {k}"));
        }
        let list = |k: &str| -> BTreeSet<String> {
            o.get(k)
                .map(|v| v.split(';').map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let shared = match o.get("shared").map(String::as_str) {
            Some("copy") => SharedMaterials::Copy,
            Some("refuse") => SharedMaterials::Refuse,
            Some(v) => return Err(format!("shared={v}: copy or refuse")),
            None => {
                return Err(
                    "shared is required: copy or refuse (a selected placement's material other placements draw)"
                        .into(),
                );
            }
        };
        SetReskateShadowCasting::new(list("materials"), list("placements"), shared)
    }
}

/// [`SetReskateShadowCasting::from_options`] as a pass factory.
pub fn set_reskate_shadow_casting(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SetReskateShadowCasting::from_options(o)?))
}

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

impl Pass for SetReskateShadowCasting {
    fn name(&self) -> &'static str {
        "set-reskate-shadow-casting"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([(
            "shared".to_owned(),
            match self.shared {
                SharedMaterials::Copy => "copy",
                SharedMaterials::Refuse => "refuse",
            }
            .to_owned(),
        )]);
        for (k, set) in [("materials", &self.materials), ("placements", &self.placements)] {
            if !set.is_empty() {
                o.insert(k.to_owned(), set.iter().cloned().collect::<Vec<_>>().join(";"));
            }
        }
        o
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
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: BTreeSet<Entity> = w.iter::<SkateMaterial>().map(|(e, _)| e).collect();
        // the named materials, wherever they are drawn
        let named: BTreeSet<Entity> = materials
            .iter()
            .copied()
            .filter(|&e| self.materials.contains(&name_of(w, e)))
            .collect();
        let found: BTreeSet<String> = named.iter().map(|&e| name_of(w, e)).collect();
        if let Some(missing) = self.materials.difference(&found).next() {
            return Err(format!(
                "set-reskate-shadow-casting: no material is named {missing}"
            ));
        }
        // a prefix no placement's name starts with
        let names: Vec<&str> = w
            .iter::<Instance>()
            .filter_map(|(e, _)| w.get::<Name>(e).map(|n| n.0.as_str()))
            .collect();
        if let Some(p) = self
            .placements
            .iter()
            .find(|p| !names.iter().any(|n| n.starts_with(p.as_str())))
        {
            return Err(format!(
                "set-reskate-shadow-casting: no placement's name starts with {p} (a name is matched as its reader wrote it, path separators included)"
            ));
        }
        // the placements selected and the others, each prototype's of each
        let selects = |e: Entity| {
            w.get::<Name>(e)
                .is_some_and(|n| self.placements.iter().any(|p| n.0.starts_with(p.as_str())))
        };
        let mut by_proto: BTreeMap<Entity, (Vec<Entity>, Vec<Entity>)> = BTreeMap::new();
        let (mut inside, mut outside) = (BTreeSet::new(), BTreeSet::new());
        for (e, i) in w.iter::<Instance>() {
            let drawing = drawn(w, i.prototype, i.variant)
                .into_iter()
                .filter(|m| materials.contains(m));
            let entry = by_proto.entry(i.prototype).or_default();
            if selects(e) {
                inside.extend(drawing);
                entry.0.push(e);
            } else {
                outside.extend(drawing);
                entry.1.push(e);
            }
        }
        let placed = by_proto.values().map(|(s, _)| s.len() as u64).sum::<u64>();
        log.count("placements selected", placed);
        let shared: BTreeSet<Entity> = inside
            .intersection(&outside)
            .copied()
            .filter(|m| !named.contains(m))
            .collect();
        if self.shared == SharedMaterials::Refuse
            && let Some(&m) = shared.first()
        {
            return Err(format!(
                "set-reskate-shadow-casting: material {} is drawn by selected placements and by others (shared=refuse)",
                name_of(w, m)
            ));
        }
        // every material that stops casting where it is
        let whole: BTreeSet<Entity> = named
            .iter()
            .copied()
            .chain(inside.difference(&shared).copied())
            .collect();
        for &m in &whole {
            let name = name_of(w, m);
            let s = w.get_mut::<SkateMaterial>(m).expect("a material");
            if s.cast_shadows == Some(false) {
                log.count("selected materials already casting no shadow", 1);
                continue;
            }
            s.cast_shadows = Some(false);
            log.count(&format!("material made to cast no shadow: {name}"), 1);
        }
        if shared.is_empty() {
            return Ok(());
        }
        // the shared ones: a copy casting no shadow, which the selected placements' prototypes draw
        let keys: BTreeSet<String> = w
            .iter::<SkateMaterial>()
            .map(|(_, m)| m.key.to_lowercase())
            .collect();
        let mut copies = BTreeMap::new();
        for &m in &shared {
            let mut s = w.get::<SkateMaterial>(m).expect("a material").clone();
            s.key = format!("{}~noshadow", s.key);
            if keys.contains(&s.key.to_lowercase()) {
                return Err(format!(
                    "set-reskate-shadow-casting: material key {} is taken",
                    s.key
                ));
            }
            s.cast_shadows = Some(false);
            let c = w.spawn();
            w.copy_components(m, c);
            w.insert(c, s);
            log.place_from(c, m);
            log.count(
                &format!(
                    "material copy casting no shadow, for the selected placements: {}",
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
                    "set-reskate-shadow-casting: prototype {} has levels of detail and would need a copy drawing \
                     materials that cast no shadow: select a level first",
                    name_of(w, proto)
                ));
            }
            let copy = w.spawn();
            w.copy_components(proto, copy);
            if let Some(mut p) = w.get::<SkatePrototype>(proto).cloned() {
                p.id = format!("{}~noshadow", p.id);
                if ids.contains(&p.id) {
                    return Err(format!(
                        "set-reskate-shadow-casting: prototype id {} is taken",
                        p.id
                    ));
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

    fn run(w: &mut World, pass: SetReskateShadowCasting) -> Result<PassLog, String> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_conservation(Conservation::Enforce)
            .run(
                w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )
            .map_err(|e| e.to_string())?;
        Ok(report.steps[0].log.clone())
    }

    fn pass(materials: &[&str], placements: &[&str], shared: SharedMaterials) -> SetReskateShadowCasting {
        let set = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        SetReskateShadowCasting::new(set(materials), set(placements), shared).unwrap()
    }

    fn casts(w: &World, m: Entity) -> Option<bool> {
        w.get::<SkateMaterial>(m).unwrap().cast_shadows
    }

    /// A room shell: its wall material drawn by the shell only, its floor material shared with a table outside it.
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
        place(&mut w, "Level Elements/Bounds/Walls", walls);
        let shell = place(&mut w, "Level Elements/Bounds/Floor", slab);
        let table = place(&mut w, "Level Elements/Thin Table", slab);
        Room {
            w,
            wall,
            floor,
            shell,
            slab,
            table,
        }
    }

    /// Named materials cast no shadow wherever they are drawn, each reported; a name matching no material, and a
    /// prefix matching no placement (a path written with spaced slashes), are refused with nothing changed.
    #[test]
    fn named_materials_cast_no_shadow() {
        let Room {
            mut w, wall, floor, ..
        } = room();
        let e = run(&mut w, pass(&["floor", "ceiling"], &[], SharedMaterials::Refuse)).unwrap_err();
        assert!(e.contains("no material is named ceiling"), "{e}");
        let e = run(
            &mut w,
            pass(&[], &["Level Elements / Bounds"], SharedMaterials::Copy),
        )
        .unwrap_err();
        assert!(
            e.contains("no placement's name starts with Level Elements / Bounds"),
            "{e}"
        );
        assert_eq!((casts(&w, wall), casts(&w, floor)), (None, None));
        let log = run(&mut w, pass(&["floor"], &[], SharedMaterials::Refuse)).unwrap();
        assert_eq!((casts(&w, wall), casts(&w, floor)), (None, Some(false)));
        assert_eq!(log.counts["material made to cast no shadow: floor"], 1);
    }

    /// By placements: a material drawn outside the selection too is refused under shared=refuse, and nothing changes.
    #[test]
    fn a_shared_material_is_refused() {
        let Room { mut w, wall, .. } = room();
        let e = run(
            &mut w,
            pass(&[], &["Level Elements/Bounds/"], SharedMaterials::Refuse),
        )
        .unwrap_err();
        assert!(
            e.contains("material floor is drawn by selected placements and by others"),
            "{e}"
        );
        assert_eq!(casts(&w, wall), None);
    }

    /// shared=copy: the shell's own material stops casting; the shared one is copied for the shell, whose prototype
    /// (placed outside the shell too) is copied to draw it, both declared, and the shell placement moved to the copy;
    /// the table keeps the casting original.
    #[test]
    fn a_shared_material_is_copied_for_the_selection() {
        let Room {
            mut w,
            wall,
            floor,
            shell,
            slab,
            table,
        } = room();
        let log = run(
            &mut w,
            pass(&[], &["Level Elements/Bounds/"], SharedMaterials::Copy),
        )
        .unwrap();
        assert_eq!((casts(&w, wall), casts(&w, floor)), (Some(false), None));
        assert_eq!(w.get::<Instance>(table).unwrap().prototype, slab);
        let copy = w.get::<Instance>(shell).unwrap().prototype;
        assert_ne!(copy, slab);
        let drawn = w.get::<Mesh>(copy).unwrap().primitives()[0].material.unwrap();
        assert_ne!(drawn, floor);
        let m = w.get::<SkateMaterial>(drawn).unwrap();
        assert_eq!((m.key.as_str(), m.cast_shadows), ("floor~noshadow", Some(false)));
        let p = w.get::<SkatePrototype>(copy).unwrap();
        assert_eq!((p.id.as_str(), p.key), ("slab~noshadow", None));
        assert_eq!(log.placed_from.get(&copy), Some(&slab));
        assert_eq!(log.placed_from.get(&drawn), Some(&floor));
        assert_eq!(log.counts["material made to cast no shadow: wall"], 1);
        assert_eq!(
            log.counts["material copy casting no shadow, for the selected placements: floor"],
            1
        );
        assert_eq!(
            log.counts["prototype copies drawing materials that cast no shadow"],
            1
        );
    }

    /// A prototype placed by selected placements only draws the copy in place.
    #[test]
    fn a_prototype_only_the_selection_places_draws_the_copy_in_place() {
        let mut w = World::new();
        let floor = material(&mut w, "floor");
        let shell_slab = prototype(&mut w, "shell slab", floor);
        let table_slab = prototype(&mut w, "table slab", floor);
        let shell = place(&mut w, "Bounds/Floor", shell_slab);
        place(&mut w, "Table", table_slab);
        let log = run(&mut w, pass(&[], &["Bounds"], SharedMaterials::Copy)).unwrap();
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
    }

    /// A selection is required, with no empty name, and shared is required; the options print the pass again.
    #[test]
    fn the_choices_are_required_and_printed() {
        let o =
            |kv: &[(&str, &str)]| -> Options { kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect() };
        assert!(SetReskateShadowCasting::from_options(&o(&[("shared", "copy")])).is_err());
        assert!(SetReskateShadowCasting::from_options(&o(&[("materials", "wall")])).is_err());
        assert!(
            SetReskateShadowCasting::from_options(&o(&[("materials", "wall;"), ("shared", "copy")])).is_err()
        );
        assert!(
            SetReskateShadowCasting::from_options(&o(&[("placements", "a"), ("shared", "some")])).is_err()
        );
        let p = SetReskateShadowCasting::from_options(&o(&[
            ("placements", "Level Elements/Bounds/"),
            ("shared", "copy"),
        ]))
        .unwrap();
        assert_eq!(SetReskateShadowCasting::from_options(&p.options()).unwrap(), p);
    }
}
