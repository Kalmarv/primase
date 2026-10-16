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
    accessors: Vec<AccessorJson>,
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<ViewJson>,
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
struct AccessorJson {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<[f32; 3]>,
}

#[derive(Serialize)]
struct ViewJson {
    buffer: u32,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    target: u32,
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

fn f32_bytes(v: &[f32], out: &mut Vec<u8>) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// The GLB of `prims`, its node and mesh named `name`. A non-finite position is an error (its bounds would not be
/// JSON numbers).
pub fn write(name: &str, prims: &[GlbPrim]) -> crate::Result<Vec<u8>> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<(usize, usize, u32)> = Vec::new();
    // accessors: (view, component type, count, type, bounds)
    struct Acc {
        view: usize,
        comp: u32,
        count: usize,
        kind: &'static str,
        bounds: Option<([f32; 3], [f32; 3])>,
    }
    let mut accs: Vec<Acc> = Vec::new();
    /// Accessor indices of one primitive and its material index.
    struct PrimJson {
        pos: usize,
        nrm: Option<usize>,
        uv: Option<usize>,
        blend: Option<usize>,
        idx: usize,
        mat: Option<usize>,
    }
    let mut prim_json: Vec<PrimJson> = Vec::new();
    let mut mat_names: Vec<String> = Vec::new();
    let mut view = |bin: &mut Vec<u8>, data: Vec<u8>, target: u32| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push((bin.len(), data.len(), target));
        bin.extend_from_slice(&data);
        views.len() - 1
    };
    for p in prims {
        let nv = p.pos.len() / 3;
        let mut pos = vec![0f32; nv * 3];
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        for i in 0..nv {
            let (x, y, z) = (p.pos[i * 3], p.pos[i * 3 + 2], -p.pos[i * 3 + 1]);
            pos[i * 3] = x;
            pos[i * 3 + 1] = y;
            pos[i * 3 + 2] = z;
            for (k, v) in [x, y, z].into_iter().enumerate() {
                if v < mn[k] {
                    mn[k] = v;
                }
                if v > mx[k] {
                    mx[k] = v;
                }
            }
        }
        let mut b = Vec::with_capacity(pos.len() * 4);
        f32_bytes(&pos, &mut b);
        let v_pos = view(&mut bin, b, 34962);
        let a_pos = accs.len();
        accs.push(Acc {
            view: v_pos,
            comp: 5126,
            count: nv,
            kind: "VEC3",
            bounds: Some((mn, mx)),
        });
        let a_nrm = p.nrm.as_ref().map(|n| {
            let mut c = vec![0f32; nv * 3];
            for i in 0..nv {
                c[i * 3] = n[i * 3];
                c[i * 3 + 1] = n[i * 3 + 2];
                c[i * 3 + 2] = -n[i * 3 + 1];
            }
            let mut b = Vec::with_capacity(c.len() * 4);
            f32_bytes(&c, &mut b);
            let v = view(&mut bin, b, 34962);
            accs.push(Acc {
                view: v,
                comp: 5126,
                count: nv,
                kind: "VEC3",
                bounds: None,
            });
            accs.len() - 1
        });
        let a_uv = p.uv.as_ref().map(|uv| {
            let mut b = Vec::with_capacity(uv.len() * 4);
            f32_bytes(uv, &mut b);
            let v = view(&mut bin, b, 34962);
            accs.push(Acc {
                view: v,
                comp: 5126,
                count: nv,
                kind: "VEC2",
                bounds: None,
            });
            accs.len() - 1
        });
        let a_blend = p.blend.as_ref().map(|w| {
            let mut b = Vec::with_capacity(w.len() * 4);
            f32_bytes(w, &mut b);
            let v = view(&mut bin, b, 34962);
            accs.push(Acc {
                view: v,
                comp: 5126,
                count: nv,
                kind: "SCALAR",
                bounds: None,
            });
            accs.len() - 1
        });
        let mut b = Vec::with_capacity(p.idx.len() * 4);
        for i in &p.idx {
            b.extend_from_slice(&i.to_le_bytes());
        }
        let v_idx = view(&mut bin, b, 34963);
        let a_idx = accs.len();
        accs.push(Acc {
            view: v_idx,
            comp: 5125,
            count: p.idx.len(),
            kind: "SCALAR",
            bounds: None,
        });
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
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let gltf = GltfJson {
        asset: Asset {
            version: "2.0",
            generator: "gta-skate",
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
        accessors: accs
            .iter()
            .map(|a| AccessorJson {
                buffer_view: a.view,
                component_type: a.comp,
                count: a.count,
                kind: a.kind,
                min: a.bounds.map(|b| b.0),
                max: a.bounds.map(|b| b.1),
            })
            .collect(),
        buffer_views: views
            .iter()
            .map(|&(off, len, target)| ViewJson {
                buffer: 0,
                byte_offset: off,
                byte_length: len,
                target,
            })
            .collect(),
        buffers: [Buffer {
            byte_length: bin.len(),
        }],
    };
    let mut js = crate::json_bytes(&format!("{name}.glb"), &gltf, false)?;
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }

    let mut out = Vec::with_capacity(28 + js.len() + bin.len());
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + js.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    Ok(out)
}
