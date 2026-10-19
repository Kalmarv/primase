//! Collision soups between the world's `core.collision` and scene-interchange v1.
//!
//! v1 stores a soup's surface per triangle as a byte: bits 0-6 index the scene's surfaces (its `surfaces` list, or
//! [`STANDARD_SURFACES`] without one), bit 7 marks natural rock. The reader turns a soup into `core.collision` (each
//! surface by its name in `core.surfaces`) and, when any triangle is rock, [`SkateRock`]; the writers turn it back,
//! each core surface onto the first scene surface of its name.

use primase_core::{Collision, Component, Entity, JsonError, Surfaces, World};
use serde_json::{Value, json};

use crate::dialect::{STANDARD_SURFACES, SkateScene, SkateSoup};

/// Which triangles of the entity's `core.collision` are natural rock (v1's surface bit 7).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SkateRock(pub Vec<bool>);

impl Component for SkateRock {
    const NAME: &'static str = "skate.rock";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(
            json!({"triangles": self.0.len(), "rock": self.0.iter().filter(|&&r| r).count()}),
        ))
    }

    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        match w.get::<Collision>(e) {
            Some(c) if c.triangles.len() == self.0.len() => Ok(()),
            Some(c) => Err(format!(
                "{} rock flags for {} triangles",
                self.0.len(),
                c.triangles.len()
            )),
            None => Err("rock flags without core.collision".into()),
        }
    }
}

/// The scene's surfaces by index: its `surfaces` list, or the standard ones without one (or with an empty one).
pub fn scene_surfaces(surfaces: Option<&[String]>) -> Vec<String> {
    match surfaces {
        Some(list) if !list.is_empty() => list.to_vec(),
        _ => STANDARD_SURFACES.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Put a v1 soup on entity `e` as `core.collision` (and [`SkateRock`] when any triangle is rock), its surface bytes
/// named by `names`, the scene's surfaces. The bytes were checked against `names` when the table was read.
pub fn insert_soup(
    w: &mut World,
    e: Entity,
    soup: SkateSoup,
    names: &[String],
) -> Result<(), primase_core::SurfaceIndexError> {
    let mut surfaces = w.resource::<Surfaces>().cloned().unwrap_or_default();
    let by_scene: Vec<u16> = names
        .iter()
        .map(|n| surfaces.index(n))
        .collect::<Result<_, _>>()?;
    w.insert_resource(surfaces);
    let rock: Vec<bool> = soup.surfaces.iter().map(|&b| b & 0x80 != 0).collect();
    if rock.iter().any(|&r| r) {
        w.insert(e, SkateRock(rock));
    }
    w.insert(
        e,
        Collision {
            surfaces: soup
                .surfaces
                .iter()
                .map(|&b| by_scene[usize::from(b & 0x7F)])
                .collect(),
            positions: soup.positions,
            triangles: soup.triangles,
        },
    );
    Ok(())
}

/// Entity `e`'s collision as v1 writes it: its `core.collision` with surface bytes for the scene's surfaces (each
/// core surface the first scene surface of its name; a name the scene has no surface for is an error), or a legacy
/// [`SkateSoup`] (worlds that still build one), or `None`.
pub fn soup(w: &World, e: Entity, scene: Option<&SkateScene>) -> Result<Option<SkateSoup>, String> {
    let Some(c) = w.get::<Collision>(e) else {
        return Ok(w.get::<SkateSoup>(e).cloned());
    };
    let names = scene_surfaces(scene.and_then(|s| s.surfaces.as_deref()));
    let world = w.resource::<Surfaces>();
    let rock = w.get::<SkateRock>(e);
    let mut bytes = Vec::with_capacity(c.surfaces.len());
    for (i, &s) in c.surfaces.iter().enumerate() {
        let name = world
            .and_then(|t| t.0.get(usize::from(s)))
            .map(|x| x.name.as_str())
            .ok_or_else(|| format!("surface {s} not in the world's surfaces"))?;
        let at = names.iter().position(|n| n == name).ok_or_else(|| {
            format!(
                "surface '{name}' is not one of the scene's surfaces ({})",
                names.join(", ")
            )
        })?;
        let rock = rock.is_some_and(|r| r.0.get(i) == Some(&true));
        bytes.push(at as u8 | if rock { 0x80 } else { 0 });
    }
    Ok(Some(SkateSoup {
        positions: c.positions.clone(),
        triangles: c.triangles.clone(),
        surfaces: bytes,
    }))
}
