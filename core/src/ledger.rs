//! What a pass leaves out on purpose, by name: the measures a drop is counted in ([`Measure`]) and the helper that
//! despawns an entity declaring what it held ([`despawn`]). A pass records a drop with [`PassLog::dropped`]; a
//! conversion reports every drop as a selection notice ([`crate::convert::ConvertReport::notices`]).
//!
//! - **Placements**: entities with a `core.instance`.
//! - **Textures**: entities with a `core.texture` or a `core.texture_file`.
//! - **Primitives**: the primitives of a `core.mesh` (their triangles go with them).
//! - **Triangles**: triangles left out of a primitive that stays.
//!
//! Adding is never a drop and needs no declaration.

use std::fmt;

use crate::components::{Instance, Mesh, Texture, TextureFile};
use crate::pass::PassLog;
use crate::world::{Entity, World};

/// What a drop is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Measure {
    Placements,
    Primitives,
    Triangles,
    Textures,
}

impl Measure {
    pub const ALL: [Measure; 4] = [
        Measure::Placements,
        Measure::Primitives,
        Measure::Triangles,
        Measure::Textures,
    ];
}

impl fmt::Display for Measure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Measure::Placements => "placements",
            Measure::Primitives => "primitives",
            Measure::Triangles => "triangles",
            Measure::Textures => "textures",
        })
    }
}

/// What `e` holds of the measures: a placement, a texture, its mesh's primitives.
pub fn held(w: &World, e: Entity) -> Vec<(Measure, u64)> {
    let mut out = Vec::new();
    if w.has::<Instance>(e) {
        out.push((Measure::Placements, 1));
    }
    if w.has::<Texture>(e) || w.has::<TextureFile>(e) {
        out.push((Measure::Textures, 1));
    }
    if let Some(m) = w.get::<Mesh>(e) {
        out.push((Measure::Primitives, m.primitives().len() as u64));
    }
    out
}

/// Despawn `e` on purpose, declaring in `log` what it held ([`held`]) as left out for `why`. False, and nothing
/// declared, when it was not alive.
pub fn despawn(w: &mut World, e: Entity, log: &mut PassLog, why: &str) -> bool {
    if !w.is_alive(e) {
        return false;
    }
    for (m, n) in held(w, e) {
        log.dropped(m, why, n);
    }
    w.despawn(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Primitive, Topology};

    /// An entity's drop names what it held: a placement and its mesh's primitives; nothing for one gone already.
    #[test]
    fn despawn_declares_what_the_entity_held() {
        let mut w = World::new();
        let proto = w.spawn();
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let prim = Primitive {
            topology: Topology::Triangles,
            streams: Default::default(),
            indices: None,
            material: None,
        };
        w.insert(e, Mesh::new(vec![prim.clone(), prim]));
        let mut log = PassLog::default();
        assert!(despawn(&mut w, e, &mut log, "left out"));
        assert!(!despawn(&mut w, e, &mut log, "left out"));
        assert_eq!(log.declared(Measure::Placements), 1);
        assert_eq!(log.declared(Measure::Primitives), 2);
        assert_eq!(log.declared(Measure::Textures), 0);
        assert_eq!(
            log.drops.get(&(Measure::Primitives, "left out".to_owned())),
            Some(&2)
        );
    }
}
