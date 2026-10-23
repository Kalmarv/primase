//! The map's GLB files: one mesh, one primitive per material, in Studio's y-up frame (x, z, -y from the writer's
//! z-up frame): per primitive positions, normals, uvs, then indices, each buffer view 4-aligned; compact JSON padded
//! with spaces.

use serde::Serialize;

#[derive(Serialize)]
struct GltfJson<'a> {
    asset: Asset,
    scene: u32,
    scenes: [SceneJson; 1],
    nodes: [Node<'a>; 1],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    materials: Vec<Named<'a>>,
    meshes: [MeshJson<'a>; 1],
    accessors: Vec<serde_json::Value>,
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<serde_json::Value>,
    buffers: [Buffer; 1],
}

#[derive(Serialize)]
struct Asset {
    version: &'static str,
    generator: &'static str,
}

#[derive(Serialize)]
struct SceneJson {
    nodes: [u32; 1],
}

#[derive(Serialize)]
struct Node<'a> {
    name: &'a str,
    mesh: u32,
}

#[derive(Serialize)]
struct Named<'a> {
    name: &'a str,
}

#[derive(Serialize)]
struct MeshJson<'a> {
    name: &'a str,
    primitives: Vec<PrimitiveJson>,
}

#[derive(Serialize)]
struct PrimitiveJson {
    attributes: Attributes,
    indices: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    material: Option<usize>,
    mode: u32,
}

#[derive(Serialize)]
struct Attributes {
    #[serde(rename = "POSITION")]
    position: usize,
    #[serde(rename = "NORMAL", skip_serializing_if = "Option::is_none")]
    normal: Option<usize>,
    #[serde(rename = "TEXCOORD_0", skip_serializing_if = "Option::is_none")]
    texcoord_0: Option<usize>,
    /// Studio's per-vertex blend weight (a `vertex_blend` material): float scalar 0..1.
    #[serde(rename = "_RESKATE_BLEND", skip_serializing_if = "Option::is_none")]
    reskate_blend: Option<usize>,
}

#[derive(Serialize)]
struct Buffer {
    #[serde(rename = "byteLength")]
    byte_length: usize,
}

/// One primitive, in the writer's frame (z up). `nrm` and `uv` may be absent; `material` names a map material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlbPrim {
    pub material: Option<String>,
    pub pos: Vec<f32>,
    pub nrm: Option<Vec<f32>>,
    pub uv: Option<Vec<f32>>,
    pub idx: Vec<u32>,
    /// Per-vertex blend weight of a `vertex_blend` material, written as `_RESKATE_BLEND`.
    pub blend: Option<Vec<f32>>,
}

/// The GLB of `prims`, its node and mesh named `name`: the container, the buffer views and the float accessors are
/// `primase_glb`'s ([`primase_glb::assemble`], [`primase_glb::Buffers`]); the frame (Studio's y-up from the writer's
/// z-up), the one-primitive-per-material layout and the `_RESKATE_BLEND` attribute are this writer's. A non-finite
/// position is an error (its bounds would not be JSON numbers).
pub fn write(name: &str, prims: &[GlbPrim]) -> crate::Result<Vec<u8>> {
    use primase_glb::{Buffers, ElementType};
    use serde_json::json;
    /// Accessor indices of one primitive and its material index.
    struct PrimJson {
        pos: usize,
        nrm: Option<usize>,
        uv: Option<usize>,
        blend: Option<usize>,
        idx: usize,
        mat: Option<usize>,
    }
    let mut buffers = Buffers::default();
    let mut prim_json: Vec<PrimJson> = Vec::new();
    let mut mat_names: Vec<String> = Vec::new();
    for p in prims {
        let nv = p.pos.len() / 3;
        if let Some(k) = p.pos[..nv * 3].iter().position(|x| !x.is_finite()) {
            return Err(crate::Error::Scene(format!(
                "{name}: position value {k} is not finite, so the bounds are not JSON numbers"
            )));
        }
        let mut pos = vec![0f32; nv * 3];
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        for i in 0..nv {
            let (x, y, z) = (p.pos[i * 3], p.pos[i * 3 + 2], -p.pos[i * 3 + 1]);
            pos[i * 3] = x;
            pos[i * 3 + 1] = y;
            pos[i * 3 + 2] = z;
            for (k, v) in [x, y, z].into_iter().enumerate() {
                mn[k] = mn[k].min(v);
                mx[k] = mx[k].max(v);
            }
        }
        let a_pos =
            buffers.float_accessor(&pos, ElementType::Vec3, Some((mn.to_vec(), mx.to_vec())), true)?;
        let a_nrm = p
            .nrm
            .as_ref()
            .map(|n| {
                let mut c = vec![0f32; nv * 3];
                for i in 0..nv {
                    c[i * 3] = n[i * 3];
                    c[i * 3 + 1] = n[i * 3 + 2];
                    c[i * 3 + 2] = -n[i * 3 + 1];
                }
                buffers.float_accessor(&c, ElementType::Vec3, None, true)
            })
            .transpose()?;
        let a_uv =
            p.uv.as_ref()
                .map(|uv| buffers.float_accessor(uv, ElementType::Vec2, None, true))
                .transpose()?;
        let a_blend = p
            .blend
            .as_ref()
            .map(|w| buffers.float_accessor(w, ElementType::Scalar, None, true))
            .transpose()?;
        // unsigned int indices always (Studio's writer does not narrow them)
        let idx_bytes: Vec<u8> = p.idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let v_idx = buffers.view(&idx_bytes, Some(34963));
        buffers.accessors.push(json!({
            "bufferView": v_idx, "componentType": 5125, "count": p.idx.len(), "type": "SCALAR",
        }));
        let a_idx = buffers.accessors.len() - 1;
        let mat = p
            .material
            .as_ref()
            .map(|m| match mat_names.iter().position(|x| x == m) {
                Some(i) => i,
                None => {
                    mat_names.push(m.clone());
                    mat_names.len() - 1
                }
            });
        prim_json.push(PrimJson {
            pos: a_pos,
            nrm: a_nrm,
            uv: a_uv,
            blend: a_blend,
            idx: a_idx,
            mat,
        });
    }
    let byte_length = buffers.bin.len().next_multiple_of(4);
    let gltf = GltfJson {
        asset: Asset {
            version: "2.0",
            generator: "primase",
        },
        scene: 0,
        scenes: [SceneJson { nodes: [0] }],
        nodes: [Node { name, mesh: 0 }],
        materials: mat_names.iter().map(|m| Named { name: m }).collect(),
        meshes: [MeshJson {
            name,
            primitives: prim_json
                .iter()
                .map(|p| PrimitiveJson {
                    attributes: Attributes {
                        position: p.pos,
                        normal: p.nrm,
                        texcoord_0: p.uv,
                        reskate_blend: p.blend,
                    },
                    indices: p.idx,
                    material: p.mat,
                    mode: 4,
                })
                .collect(),
        }],
        accessors: buffers.accessors,
        buffer_views: buffers.views,
        buffers: [Buffer { byte_length }],
    };
    let js = crate::json_bytes(&format!("{name}.glb"), &gltf, false)?;
    Ok(primase_glb::assemble(&js, &buffers.bin)?)
}
