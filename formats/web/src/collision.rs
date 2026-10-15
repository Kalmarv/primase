use crate::{Error, Result, ViewerItem};
use primase_core::{Collision, Entity, Instance, Name, Surfaces, Transform, World};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Soup {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    surfaces: Vec<u16>,
    /// The surfaces' names by index; `surfaces` index them.
    #[serde(default)]
    names: Vec<String>,
}

/// The world's `core.collision` as collision items in world space: a prototype's once per instance through the
/// placement's transform, any other entity's through its own (or none), triangles flipped under a mirroring transform
/// so fronts stay counter-clockwise. Surfaces are the world's indices, named.
pub(super) fn world_items(world: &World) -> Result<Vec<ViewerItem>> {
    let names: Vec<String> = world
        .resource::<Surfaces>()
        .map(|s| s.0.iter().map(|x| x.name.clone()).collect())
        .unwrap_or_default();
    let mut placed: BTreeMap<Entity, Vec<Entity>> = BTreeMap::new();
    for (e, i) in world.iter::<Instance>() {
        placed.entry(i.prototype).or_default().push(e);
    }
    let name = |e: Entity| {
        world
            .get::<Name>(e)
            .map_or_else(|| format!("collision {e:?}"), |n| n.0.clone())
    };
    let mut items = Vec::new();
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
            None => vec![(
                e,
                world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY),
            )],
        };
        for (at, t) in by {
            let positions: Vec<[f32; 3]> = c.positions.iter().map(|&p| t.apply(p)).collect();
            // a mirroring transform reverses winding: flip so fronts stay counter-clockwise
            let triangles: Vec<[u32; 3]> = if t.determinant() < 0.0 {
                c.triangles.iter().map(|&[a, b, d]| [a, d, b]).collect()
            } else {
                c.triangles.clone()
            };
            items.push(ViewerItem {
                name: name(at),
                kind: "collision".into(),
                anchor: positions[c.triangles[0][0] as usize].map(f64::from),
                data: json!({"positions": positions, "triangles": triangles, "surfaces": c.surfaces, "names": names}),
            });
        }
    }
    Ok(items)
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
    Ok(primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?)
}
