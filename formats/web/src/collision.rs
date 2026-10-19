use crate::{Error, Result, ViewerItem};
use primase_core::{Collision, Entity, Instance, Name, Surfaces, Transform, TransformKind, World};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct Soup {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    surfaces: Vec<u16>,
    /// The surfaces' names by index; `surfaces` index them.
    #[serde(default)]
    names: Vec<String>,
}

/// A collision record planned by reference: which entity's `core.collision` it is, placed by which transform, under
/// which name and anchor. The soup itself is made from the cell's world when the cell is written ([`made`]), so the
/// plan holds no geometry.
#[derive(Clone, Debug)]
pub(super) struct Planned {
    /// The entity holding the `core.collision` (a prototype, or an entity placed as itself).
    pub holder: Entity,
    /// The record's name: its placement's (or the holder's) `core.name`.
    pub name: String,
    /// Into world space: the placement's transform, or the holder's own.
    pub transform: Transform,
    /// Its first triangle's first vertex in world space: where it is filed.
    pub anchor: [f64; 3],
}

/// The world's `core.collision` as collision records by reference: a prototype's once per instance through the
/// placement's transform, any other entity's through its own (or none). A mirrored or sheared transform is refused
/// (`bake-nonrigid` makes them rigid first).
pub(super) fn world_refs(world: &World) -> Result<Vec<Planned>> {
    let mut out = Vec::new();
    for (holder, c, at, transform) in placed_collision(world, None)? {
        out.push(Planned {
            holder,
            name: name(world, at),
            transform,
            anchor: anchor(c, transform)?,
        });
    }
    Ok(out)
}

/// The collision record `r` from the cell's world `world` (where `r.holder` is loaded), its surfaces named by
/// `names` (the planned world's).
pub(super) fn made(world: &World, r: &Planned, names: &[String]) -> Result<ViewerItem> {
    let c = world.get::<Collision>(r.holder).ok_or_else(|| {
        Error::Invalid(format!(
            "collision of {:?} planned but not in the cell's world",
            r.holder
        ))
    })?;
    Ok(item(r.name.clone(), r.anchor, c, r.transform, names))
}

/// The collision of the prototypes `placements` place, once per placement of them in `placements`, as records: the
/// collision a cell's world brought with its prototypes.
pub(super) fn placement_items(world: &World, placements: &BTreeSet<Entity>) -> Result<Vec<ViewerItem>> {
    if placements.is_empty() {
        return Ok(Vec::new());
    }
    let names = surface_names(world);
    placed_collision(world, Some(placements))?
        .into_iter()
        .map(|(_, c, at, t)| Ok(item(name(world, at), anchor(c, t)?, c, t, &names)))
        .collect()
}

/// The names of `world`'s surfaces, by index.
pub(super) fn surface_names(world: &World) -> Vec<String> {
    world
        .resource::<Surfaces>()
        .map(|s| s.0.iter().map(|x| x.name.clone()).collect())
        .unwrap_or_default()
}

/// Every soup of `world` with what places it: (holder, its collision, the entity placing it, the transform), a
/// prototype's once per placement (with `only`, of those placements alone), any other entity's through its own.
fn placed_collision<'w>(
    world: &'w World,
    only: Option<&BTreeSet<Entity>>,
) -> Result<Vec<(Entity, &'w Collision, Entity, Transform)>> {
    let mut placed: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
    for (e, i) in world.iter::<Instance>() {
        if only.is_none_or(|o| o.contains(&e)) {
            placed.entry(i.prototype).or_default().push(e);
        }
    }
    let mut out = Vec::new();
    for (e, c) in world.iter::<Collision>() {
        if c.triangles.is_empty() {
            continue;
        }
        let by: Vec<(Entity, Transform)> = match placed.get(&e) {
            Some(ps) => ps
                .iter()
                .map(|&p| {
                    (
                        p,
                        world.get::<Transform>(p).copied().unwrap_or(Transform::IDENTITY),
                    )
                })
                .collect(),
            // with `only`, a prototype's collision for those placements alone
            None if only.is_some() => continue,
            None => vec![(
                e,
                world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY),
            )],
        };
        for (at, t) in by {
            if matches!(t.classify(), TransformKind::Mirrored | TransformKind::Sheared) {
                return Err(Error::Invalid(format!(
                    "collision of {at:?}: a non-rigid transform; run bake-nonrigid first"
                )));
            }
            out.push((e, c, at, t));
        }
    }
    Ok(out)
}

fn name(world: &World, e: Entity) -> String {
    world
        .get::<Name>(e)
        .map_or_else(|| format!("collision {e:?}"), |n| n.0.clone())
}

/// A soup's first triangle's first vertex through `t`.
fn anchor(c: &Collision, t: Transform) -> Result<[f64; 3]> {
    let first = c
        .triangles
        .first()
        .and_then(|tri| c.positions.get(tri[0] as usize))
        .ok_or_else(|| Error::Invalid("collision whose first triangle names no vertex".into()))?;
    Ok(t.apply(*first).map(f64::from))
}

/// The record of soup `c` in world space through `t`.
fn item(name: String, anchor: [f64; 3], c: &Collision, t: Transform, names: &[String]) -> ViewerItem {
    let positions: Vec<[f32; 3]> = c.positions.iter().map(|&p| t.apply(p)).collect();
    ViewerItem {
        name,
        kind: "collision".into(),
        anchor,
        data: json!({"positions": positions, "triangles": c.triangles, "surfaces": c.surfaces, "names": names}),
    }
}

/// Exact collision geometry with one surface index (u16) per triangle, the names in the extras. A surface is a
/// face attribute, so its accessor is referenced by extras, not vertex attributes.
pub(super) fn write(items: &[&ViewerItem]) -> Result<Vec<u8>> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    for item in items {
        let soup: Soup = serde_json::from_value(item.data.clone())?;
        if soup.positions.is_empty()
            || soup.triangles.is_empty()
            || soup.surfaces.len() != soup.triangles.len()
            || (!soup.names.is_empty() && soup.surfaces.iter().any(|&s| usize::from(s) >= soup.names.len()))
            || soup.positions.iter().flatten().any(|v| !v.is_finite())
            || soup
                .triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= soup.positions.len())
        {
            return Err(Error::Invalid("invalid collision soup".into()));
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let start = bin.len();
        for p in &soup.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
                bin.extend_from_slice(&p[k].to_le_bytes());
            }
        }
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bin.len()-start,"target":34962}));
        let position = accessors.len();
        accessors.push(json!({"bufferView":views.len()-1,"componentType":5126,"count":soup.positions.len(),"type":"VEC3","min":min,"max":max}));
        let start = bin.len();
        for &i in soup.triangles.iter().flatten() {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bin.len()-start,"target":34963}));
        let indices = accessors.len();
        accessors.push(json!({"bufferView":views.len()-1,"componentType":5125,"count":soup.triangles.len()*3,"type":"SCALAR"}));
        let start = bin.len();
        for s in &soup.surfaces {
            bin.extend_from_slice(&s.to_le_bytes());
        }
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bin.len()-start}));
        let surfaces = accessors.len();
        accessors.push(json!({"bufferView":views.len()-1,"componentType":5123,"count":soup.surfaces.len(),"type":"SCALAR"}));
        meshes.push(json!({"primitives":[{"attributes":{"POSITION":position},"indices":indices,"mode":4,"extras":{"primase.collision_surfaces":{"accessor":surfaces,"domain":"triangle","version":2,"names":soup.names}}}]}));
        nodes.push(json!({"mesh":meshes.len()-1,"name":item.name}));
    }
    let doc = json!({"asset":{"version":"2.0","generator":"primase-web"},"scene":0,"scenes":[{"nodes":(0..nodes.len()).collect::<Vec<_>>()}],"nodes":nodes,"meshes":meshes,"accessors":accessors,"bufferViews":views,"buffers":[{"byteLength":bin.len()}]});
    Ok(primase_glb::assemble(&serde_json::to_vec(&doc)?, &bin)?)
}
