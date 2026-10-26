//! A level cell's entities: each Entity object's world matrix and the objects its Visual components draw, and a
//! LODSelector's levels.
//!
//! An Entity object's body starts with the managed byte, its id (u32), its class (`0x0984415E`, CRC-32 of `Entity`),
//! then its GlobalMatrix: 16 f32, row by row, the images of local x, y and z in the first three rows and the
//! translation in the fourth (row vectors: `world = local * M`). Its components follow; each Visual component, inline,
//! is its class (`0xEC658D29`), then its Resource: an inline ResourceRef (id u32, class `0x3C6C77C8`, a kind byte,
//! the id of the object it shows: a Mesh, a LODSelector, a light). The visuals are found by that run of bytes in the
//! entity's body (Visual class, an id, the ResourceRef class), not by reading every component class: the component
//! classes the reader does not read are skipped this way, and [`Entity::visuals`] is what the run finds.
//!
//! Every component, inline, begins `[pointer byte 0][managed byte][id u32][class u32][ResourceRef: id u32, class
//! 0x3C6C77C8, ...]`; [`Entity::components`] lists the classes found by that run, in order (a component whose
//! fields are not read is still named by its class). A SpawnPoint component ([`SPAWN_POINT`]) marks a spawn.
//!
//! A LODSelector's body holds five LODDescriptors, each its id, class `0xA360319A`, the object of that level (u32),
//! its switch distance (f32) and a flag byte; the first is the nearest level.

use primase_bytes::{Field, FieldError, View};

const LE: primase_bytes::Endian = primase_bytes::Endian::Little;

pub const ENTITY: u32 = 0x0984_415E;
pub const VISUAL: u32 = 0xEC65_8D29;
pub const RESOURCE_REF: u32 = 0x3C6C_77C8;
pub const LOD_SELECTOR: u32 = 0x51DC_6B80;
pub const LOD_DESCRIPTOR: u32 = 0xA360_319A;
pub const GRID_CELL_DATA_BLOCK: u32 = 0xAC2B_BF68;
/// The class of a spawn node's component (`0xF63C9D7F`; the name `SpawnPoint` is the class's in later games of the
/// engine family, where the hash is named: this game's data does not name it).
pub const SPAWN_POINT: u32 = 0xF63C_9D7F;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Short(#[from] FieldError),
    #[error("entity body: class {0:#x}, not Entity")]
    NotEntity(u32),
    #[error("a non-finite world matrix")]
    Matrix,
}

/// A Visual component: its ResourceRef's kind byte and the object it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visual {
    pub kind: u8,
    pub target: u32,
}

/// An Entity as read (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub id: u32,
    /// GlobalMatrix, row by row (row vectors).
    pub matrix: [f32; 16],
    pub visuals: Vec<Visual>,
    /// The classes of its components, in order of appearance (the module documentation).
    pub components: Vec<u32>,
}

impl Entity {
    /// The Entity object whose body is `body`.
    pub fn parse(body: &[u8]) -> Result<Entity, Error> {
        let v = View::new(body);
        let id = v.u32(1, LE).field("Entity id")?;
        let class = v.u32(5, LE).field("Entity class")?;
        if class != ENTITY {
            return Err(Error::NotEntity(class));
        }
        let mut matrix = [0f32; 16];
        for (k, m) in matrix.iter_mut().enumerate() {
            *m = v
                .f32(9usize.saturating_add(k.saturating_mul(4)), LE)
                .field("Entity.GlobalMatrix")?;
        }
        if matrix.iter().any(|x| !x.is_finite()) {
            return Err(Error::Matrix);
        }
        let visual = VISUAL.to_le_bytes();
        let rref = RESOURCE_REF.to_le_bytes();
        let mut visuals = Vec::new();
        let mut at = 73usize;
        while let Some(p) = body
            .get(at..)
            .and_then(|rest| rest.windows(4).position(|w| w == visual))
        {
            let p = at.saturating_add(p);
            if body.get(p.saturating_add(8)..p.saturating_add(12)) == Some(&rref[..]) {
                let kind = v.u8(p.saturating_add(12)).field("Visual's ResourceRef kind")?;
                let target = v
                    .u32(p.saturating_add(13), LE)
                    .field("Visual's ResourceRef target")?;
                visuals.push(Visual { kind, target });
            }
            at = p.saturating_add(4);
        }
        let mut components = Vec::new();
        let mut at = 73usize;
        while let Some(p) = body
            .get(at..)
            .and_then(|rest| rest.windows(4).position(|w| w == rref))
        {
            let q = at.saturating_add(p);
            // [pointer 0][managed][id][class][ResourceRef id] precede the ResourceRef's class
            if let Some(h) = q.checked_sub(14)
                && body.get(h) == Some(&0)
                && let Ok(class) = v.u32(q.saturating_sub(8), LE)
            {
                components.push(class);
            }
            at = q.saturating_add(4);
        }
        Ok(Entity {
            id,
            matrix,
            visuals,
            components,
        })
    }
}

/// A LODSelector body's levels: each level's object and switch distance, nearest first.
pub fn lod_levels(body: &[u8]) -> Result<Vec<(u32, f32)>, Error> {
    let v = View::new(body);
    let tag = LOD_DESCRIPTOR.to_le_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(p) = body
        .get(at..)
        .and_then(|rest| rest.windows(4).position(|w| w == tag))
    {
        let p = at.saturating_add(p);
        let object = v.u32(p.saturating_add(4), LE).field("LODDescriptor.Object")?;
        let distance = v
            .f32(p.saturating_add(8), LE)
            .field("LODDescriptor.SwitchDistance")?;
        out.push((object, distance));
        at = p.saturating_add(13);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component's header as the module documentation lays it out: pointer byte, managed byte, id, class, then the
    /// ResourceRef (id, class, kind, target).
    fn component(class: u32, kind: u8, target: u32) -> Vec<u8> {
        let mut b = vec![0, 1];
        for w in [7, class, 8, RESOURCE_REF] {
            b.extend(w.to_le_bytes());
        }
        b.push(kind);
        b.extend(target.to_le_bytes());
        b.extend([0xAA; 6]);
        b
    }

    /// An Entity body, hand-laid: managed byte, id, class, the identity matrix with translation (1, 2, 3), a null
    /// hierarchy pointer, the component count and the components.
    fn entity(components: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![1];
        b.extend(0x1234u32.to_le_bytes());
        b.extend(ENTITY.to_le_bytes());
        let m: [f32; 16] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 1., 2., 3., 1.];
        m.iter().for_each(|x| b.extend(x.to_le_bytes()));
        b.push(3);
        b.extend((components.len() as u32).to_le_bytes());
        components.iter().for_each(|c| b.extend(c));
        b
    }

    /// Visuals and the classes of all components are read from the component headers, in order, and a component whose
    /// fields are not read is named by its class.
    #[test]
    fn components_are_named_by_their_headers() {
        let body = entity(&[
            component(SPAWN_POINT, 0xff, 0),
            component(VISUAL, 0xfe, 0x99),
            component(0xDEAD_BEEF, 0xff, 0),
        ]);
        let e = Entity::parse(&body).unwrap();
        assert_eq!(e.id, 0x1234);
        assert_eq!(e.matrix[12..15], [1.0, 2.0, 3.0]);
        assert_eq!(e.components, [SPAWN_POINT, VISUAL, 0xDEAD_BEEF]);
        assert_eq!(
            e.visuals,
            [Visual {
                kind: 0xfe,
                target: 0x99
            }]
        );
        // an Entity with no component has none
        assert!(Entity::parse(&entity(&[])).unwrap().components.is_empty());
    }
}
