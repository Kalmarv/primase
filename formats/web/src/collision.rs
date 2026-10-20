use crate::{Error, Result, ViewerItem};
use primase_core::{Collision, Entity, Name, Surfaces, Transform, TransformKind, World};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Soup {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    surfaces: Vec<u16>,
    /// The surfaces' names by index; `surfaces` index them.
    #[serde(default)]
    names: Vec<String>,
}

/// The collision record of `c` placed through `t` (the placing entity's transform, or the holder's own), named `name`
/// and anchored at its first triangle's first vertex in world space; `None` for a soup without triangles. A mirrored or
/// sheared transform is refused (`bake-nonrigid` makes them rigid first).
pub(super) fn record(world: &World, at: Entity, c: &Collision, t: Transform) -> Result<Option<ViewerItem>> {
    if c.triangles.is_empty() {
        return Ok(None);
    }
    if matches!(t.classify(), TransformKind::Mirrored | TransformKind::Sheared) {
        return Err(Error::Invalid(format!(
            "collision of {at:?}: a non-rigid transform; run bake-nonrigid first"
        )));
    }
    let first = c
        .triangles
        .first()
        .and_then(|tri| c.positions.get(tri[0] as usize))
        .ok_or_else(|| Error::Invalid("collision whose first triangle names no vertex".into()))?;
    let anchor = t.apply(*first).map(f64::from);
    if anchor.iter().any(|v| !v.is_finite()) {
        return Err(Error::Invalid("collision with a non-finite position".into()));
    }
    let names: Vec<String> = world
        .resource::<Surfaces>()
        .map(|s| s.0.iter().map(|x| x.name.clone()).collect())
        .unwrap_or_default();
    let positions: Vec<[f32; 3]> = c.positions.iter().map(|&p| t.apply(p)).collect();
    Ok(Some(ViewerItem {
        name: world
            .get::<Name>(at)
            .map_or_else(|| format!("collision {at:?}"), |n| n.0.clone()),
        kind: "collision".into(),
        anchor,
        data: json!({"positions": positions, "triangles": c.triangles, "surfaces": c.surfaces, "names": names}),
    }))
}

/// Where a standalone holder's collision is filed: its first triangle's first vertex through its transform (`None`
/// for a soup without triangles).
pub(super) fn anchor(c: &Collision, t: Transform) -> Option<[f64; 3]> {
    let first = c.triangles.first()?;
    c.positions
        .get(first[0] as usize)
        .map(|p| t.apply(*p).map(f64::from))
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
