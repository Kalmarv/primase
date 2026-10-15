//! The map's GLB files: one mesh, one primitive per material, in Studio's y-up frame (x, z, -y from the writer's
//! z-up frame), the layout and JSON of the reference writer exactly (compact JSON, buffer views 4-aligned, the JSON
//! chunk padded with spaces).

use crate::dotnet::JsonWriter;

/// One primitive, in the writer's frame (z up). `nrm` and `uv` may be absent; `material` names a map material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlbPrim {
    pub material: Option<String>,
    pub pos: Vec<f32>,
    pub nrm: Option<Vec<f32>>,
    pub uv: Option<Vec<f32>>,
    pub idx: Vec<u32>,
}

fn f32_bytes(v: &[f32], out: &mut Vec<u8>) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// The GLB of `prims`, its node and mesh named `name`.
pub fn write(name: &str, prims: &[GlbPrim]) -> Vec<u8> {
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
            idx: a_idx,
            mat,
        });
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let mut w = JsonWriter::new(false);
    w.start_object();
    w.start_object_named("asset");
    w.string("version", "2.0");
    w.string("generator", "gta-skate");
    w.end_object();
    w.int("scene", 0);
    w.start_array_named("scenes");
    w.start_object();
    w.start_array_named("nodes");
    w.int_value(0);
    w.end_array();
    w.end_object();
    w.end_array();
    w.start_array_named("nodes");
    w.start_object();
    w.string("name", name);
    w.int("mesh", 0);
    w.end_object();
    w.end_array();
    if !mat_names.is_empty() {
        w.start_array_named("materials");
        for m in &mat_names {
            w.start_object();
            w.string("name", m);
            w.end_object();
        }
        w.end_array();
    }
    w.start_array_named("meshes");
    w.start_object();
    w.string("name", name);
    w.start_array_named("primitives");
    for PrimJson {
        pos,
        nrm,
        uv,
        idx,
        mat,
    } in &prim_json
    {
        w.start_object();
        w.start_object_named("attributes");
        w.int("POSITION", *pos as i64);
        if let Some(n) = nrm {
            w.int("NORMAL", *n as i64);
        }
        if let Some(u) = uv {
            w.int("TEXCOORD_0", *u as i64);
        }
        w.end_object();
        w.int("indices", *idx as i64);
        if let Some(m) = mat {
            w.int("material", *m as i64);
        }
        w.int("mode", 4);
        w.end_object();
    }
    w.end_array();
    w.end_object();
    w.end_array();
    w.start_array_named("accessors");
    for a in &accs {
        w.start_object();
        w.int("bufferView", a.view as i64);
        w.int("componentType", i64::from(a.comp));
        w.int("count", a.count as i64);
        w.string("type", a.kind);
        if let Some((mn, mx)) = a.bounds {
            w.start_array_named("min");
            for v in mn {
                w.f32_value(v);
            }
            w.end_array();
            w.start_array_named("max");
            for v in mx {
                w.f32_value(v);
            }
            w.end_array();
        }
        w.end_object();
    }
    w.end_array();
    w.start_array_named("bufferViews");
    for (off, len, target) in &views {
        w.start_object();
        w.int("buffer", 0);
        w.int("byteOffset", *off as i64);
        w.int("byteLength", *len as i64);
        w.int("target", i64::from(*target));
        w.end_object();
    }
    w.end_array();
    w.start_array_named("buffers");
    w.start_object();
    w.int("byteLength", bin.len() as i64);
    w.end_object();
    w.end_array();
    w.end_object();
    let mut js = w.finish().into_bytes();
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
    out
}
