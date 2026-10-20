//! Collision soups between the world's `core.collision` and scene-interchange v1.
//!
//! v1 stores a soup's surface per triangle as a byte: bits 0-6 index the scene's surfaces (its `surfaces` list, or
//! [`STANDARD_SURFACES`] without one), bit 7 marks natural rock. The reader turns a soup into `core.collision` (each
//! surface by its name in `core.surfaces`) and, when any triangle is rock, [`SkateRock`]; the writers turn it back,
//! each core surface onto the first scene surface of its name.

use std::borrow::Cow;

use primase_core::{Collision, Entity, Surfaces, World};

use crate::components::{STANDARD_SURFACES, SkateRock, SkateScene, SkateSoup};

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
    Ok(soup_view(w, e, scene)?.map(|s| s.to_soup()))
}

/// A soup as v1 writes it ([`soup`]), its positions and triangles the world's own (borrowed, not copied).
#[derive(Debug)]
pub struct SoupView<'w> {
    pub positions: &'w [[f32; 3]],
    /// Counter-clockwise fronts.
    pub triangles: &'w [[u32; 3]],
    /// Per triangle: bits 0-6 index the scene's surfaces, bit 7 natural rock (made from `core.collision`'s surfaces,
    /// or a legacy soup's own).
    pub surfaces: Cow<'w, [u8]>,
}

impl SoupView<'_> {
    /// The soup, copied out of the world.
    pub fn to_soup(&self) -> SkateSoup {
        SkateSoup {
            positions: self.positions.to_vec(),
            triangles: self.triangles.to_vec(),
            surfaces: self.surfaces.to_vec(),
        }
    }
}

/// Entity `e`'s collision as [`soup`] gives it, borrowed from the world: only the surface bytes of a `core.collision`
/// are made.
pub fn soup_view<'w>(
    w: &'w World,
    e: Entity,
    scene: Option<&SkateScene>,
) -> Result<Option<SoupView<'w>>, String> {
    let Some(c) = w.get::<Collision>(e) else {
        return Ok(w.get::<SkateSoup>(e).map(|s| SoupView {
            positions: &s.positions,
            triangles: &s.triangles,
            surfaces: Cow::Borrowed(&s.surfaces),
        }));
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
    Ok(Some(SoupView {
        positions: &c.positions,
        triangles: &c.triangles,
        surfaces: Cow::Owned(bytes),
    }))
}
