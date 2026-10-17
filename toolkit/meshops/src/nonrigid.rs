//! `bake-nonrigid`: authored mirrors and shears moved out of transforms into geometry, so every transform a writer
//! sees is a rotation with positive per-axis scale and a translation ([`TransformKind::Regular`]).
//!
//! Readers undo their own frame's mirror in geometry; what is left non-rigid is what the source authored (a negative
//! scale, a non-uniform scale under a rotation in a hierarchy). By [`Transform::classify`]:
//! - **Mirrored** (`M = M' F`, `F` = x negated, `M'` regular): the entity keeps `M'`. A placement draws its
//!   prototype's mirror twin instead: the prototype copied ([`World::duplicate`]) with its local space mapped by `F`
//!   ([`World::apply_linear`]), made once per prototype and shared by all its mirrored placements; its LOD forms are
//!   twinned the same way; the twin's name is the prototype's with `.mirrored`. An entity drawing its own geometry
//!   (a mesh, a collision soup) is mapped in place. A light or a marker only takes `M'`: `F` keeps its local +Y and
//!   -Z, all it is oriented by.
//! - **Sheared**: the whole 3x3 part goes into a copy of the geometry made for that placement (or into the entity's own
//!   geometry), and the entity keeps only its translation; the copy's name is the prototype's with `.baked.e<n>`, `n`
//!   the placement's entity index.
//! - **Regular** and **degenerate** transforms are left as they are (a writer skips a degenerate one).
//!
//! It runs after every lowering and before the writers: a dialect component that describes geometry as stored
//! (relative to the stored transform) is dropped from a mapped entity, so the lowering that turns it into core
//! geometry (`lower-unity-collision` for `unity.collider`) must have run.
//!
//! Mapping is all or nothing per entity: when a component cannot follow ([`primase_core::Linearity::Unsupported`]),
//! or a prototype's geometry is still in a file, the placement is left as it was and counted by what stopped it.
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::{
    Collision, Entity, Instance, Light, Linear, Lods, Marker, Mesh, MeshFile, Name, Transform, TransformKind,
    World,
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeNonrigid;

impl Pass for BakeNonrigid {
    fn name(&self) -> &'static str {
        "bake-nonrigid"
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            "core.transform",
            "core.instance",
            "core.mesh",
            "core.collision",
            "core.lods",
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        for (k, n) in bake_nonrigid(w).counts {
            log.count(&k, n);
        }
        Ok(())
    }
}

/// Register `bake-nonrigid` (no options).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register("bake-nonrigid", |o| {
        if let Some(k) = o.keys().next() {
            return Err(format!("bake-nonrigid: unknown option {k}"));
        }
        Ok(Box::new(BakeNonrigid))
    });
}

/// `M F`: the transform with its local x negated (exact: negation only).
fn without_mirror(t: &Transform) -> Transform {
    let mut out = *t;
    out.cols[0] = t.cols[0].map(|x| -x);
    out
}

/// The translation alone.
fn translation_only(t: &Transform) -> Transform {
    let mut out = Transform::IDENTITY;
    out.cols[3] = t.cols[3];
    out
}

/// A copy of `proto` (and of its LOD forms) with local space mapped by `l`, or why it cannot be made (nothing is left
/// behind then).
fn mapped_copy(w: &mut World, proto: Entity, l: &Linear, suffix: &str) -> Result<Entity, String> {
    if w.has::<MeshFile>(proto) && !w.has::<Mesh>(proto) {
        return Err("prototype geometry not loaded".into());
    }
    let mut made = Vec::new();
    let result = (|| {
        let copy = w.duplicate(proto).ok_or("prototype not alive")?;
        made.push(copy);
        w.apply_linear(copy, l).map_err(|names| names.join(", "))?;
        if let Some(Name(n)) = w.get::<Name>(copy) {
            let n = format!("{n}.{suffix}");
            w.insert(copy, Name(n));
        }
        if let Some(mut lods) = w.get::<Lods>(copy).cloned() {
            for f in &mut lods.forms {
                if w.has::<MeshFile>(f.form) && !w.has::<Mesh>(f.form) {
                    return Err("LOD form geometry not loaded".to_string());
                }
                let form = w.duplicate(f.form).ok_or("LOD form not alive")?;
                made.push(form);
                w.apply_linear(form, l).map_err(|names| names.join(", "))?;
                f.form = form;
            }
            w.insert(copy, lods);
        }
        Ok(copy)
    })();
    if result.is_err() {
        for e in made {
            w.despawn(e);
        }
    }
    result
}

/// Bake every non-rigid transform (see the module documentation).
pub fn bake_nonrigid(w: &mut World) -> PassLog {
    let mut log = PassLog::default();
    let mut twins: BTreeMap<Entity, Result<Entity, String>> = BTreeMap::new();
    let placed: Vec<(Entity, Transform, Option<Entity>)> = w
        .iter::<Transform>()
        .map(|(e, t)| (e, *t, w.get::<Instance>(e).map(|i| i.prototype)))
        .collect();
    for (e, t, proto) in placed {
        let kind = t.classify();
        if !matches!(kind, TransformKind::Mirrored | TransformKind::Sheared) {
            continue;
        }
        let what = if kind == TransformKind::Mirrored {
            "mirrored"
        } else {
            "sheared"
        };
        let (linear, rigid) = match kind {
            TransformKind::Mirrored => (Linear::mirror_x(), without_mirror(&t)),
            _ => (Linear::of(&t), translation_only(&t)),
        };
        let done = match proto {
            Some(p) => {
                let copy = if kind == TransformKind::Mirrored {
                    twins
                        .entry(p)
                        .or_insert_with(|| {
                            let r = mapped_copy(w, p, &linear, "mirrored");
                            if r.is_ok() {
                                log.count("mirror twins of prototypes made", 1);
                            }
                            r
                        })
                        .clone()
                } else {
                    let r = mapped_copy(w, p, &linear, &format!("baked.e{}", e.index()));
                    if r.is_ok() {
                        log.count("prototype copies made for sheared placements", 1);
                    }
                    r
                };
                copy.map(|c| {
                    if let Some(i) = w.get_mut::<Instance>(e) {
                        i.prototype = c;
                    }
                })
            }
            None if w.has::<Mesh>(e) || w.has::<Collision>(e) => {
                w.apply_linear(e, &linear).map_err(|names| names.join(", "))
            }
            None if kind == TransformKind::Mirrored && (w.has::<Light>(e) || w.has::<Marker>(e)) => Ok(()),
            None => Err("nothing to map it into".into()),
        };
        match done {
            Ok(()) => {
                w.insert(e, rigid);
                log.count(&format!("{what} transforms made rigid"), 1);
            }
            Err(why) => log.count(&format!("{what} transforms left: {why}"), 1),
        }
    }
    log
}
