use crate::{Error, Result, ViewerItem};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Soup {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    surfaces: Vec<u8>,
}

/// Exact collision geometry with one surface byte per triangle. A surface is a
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
        bin.extend_from_slice(&soup.surfaces);
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":soup.surfaces.len()}));
        let surfaces = accessors.len();
        accessors.push(json!({"bufferView":views.len()-1,"componentType":5121,"count":soup.surfaces.len(),"type":"SCALAR"}));
        meshes.push(json!({"primitives":[{"attributes":{"POSITION":position},"indices":indices,"mode":4,"extras":{"primase.collision_surfaces":{"accessor":surfaces,"domain":"triangle","version":1}}}]}));
        nodes.push(json!({"mesh":meshes.len()-1,"name":item.name}));
    }
    let doc = json!({"asset":{"version":"2.0","generator":"primase-web"},"scene":0,"scenes":[{"nodes":(0..nodes.len()).collect::<Vec<_>>()}],"nodes":nodes,"meshes":meshes,"accessors":accessors,"bufferViews":views,"buffers":[{"byteLength":bin.len()}]});
    Ok(primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?)
}
