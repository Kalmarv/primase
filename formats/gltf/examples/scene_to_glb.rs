//! A skate-scene v1 folder -> one GLB, for Blender.
//!
//! `cargo run --release -p primase-gltf --example scene_to_glb -- <scene folder> <out.glb> [prototype|tile|none]`
//!
//! Interim: the scene reading here is a minimal stand-in until `primase-skate-scene` has its reader; then this
//! example becomes reader + [`lower_skate_material`] + writer, and the lowering moves into that crate, which owns
//! `skate.*`. It reads the manifest's textures, materials and prototypes and the `PLAC` table, and attaches:
//! textures as `core.texture_file` (+ name, source), materials as an opaque `skate.material` (the manifest's JSON)
//! lowered to `pbr.material`, prototypes as `core.mesh_file` (+ `core.material_variants`, an opaque
//! `skate.prototype` with the rest of the entry), placements as `core.instance` + `core.transform` (+ `core.tile`,
//! `core.source`, an opaque `skate.placement` with the flags and radius when set). Lights, collision, grind curves,
//! markers and water are not read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, Wrap};
use primase_core::{
    Entity, Instance, MaterialVariants, MeshFile, Name, Opaque, OpaqueData, Source, TextureFile, Tile,
    Transform, World,
};
use primase_gltf::{GroupBy, WriteOptions, load_mesh_files, write_file};
use serde_json::{Map, Value, json};

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: scene_to_glb <scene folder> <out.glb> [prototype|tile|none]".into());
    }
    let folder = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    let group_by = match args.get(3).map(String::as_str) {
        None | Some("prototype") => GroupBy::Prototype,
        Some("tile") => GroupBy::Tile,
        Some("none") => GroupBy::None,
        Some(other) => return Err(format!("unknown grouping {other}").into()),
    };
    let t0 = std::time::Instant::now();
    let (mut world, scene, lowering) = read_scene(&folder)?;
    let load = load_mesh_files(&mut world, &folder)?;
    let options = WriteOptions {
        group_by,
        base_dir: folder.clone(),
        include_unplaced: false,
        scene_name: Some(scene),
    };
    let report = write_file(&world, &options, &out)?;
    println!("read: {} entities; meshes loaded {load:?}", world.entity_count());
    println!("lowering skate.material -> pbr.material: {lowering:?}");
    println!("{report}");
    println!("wrote {} in {:.1} s", out.display(), t0.elapsed().as_secs_f64());
    Ok(())
}

/// What the lowering approximated, counted over all materials.
#[derive(Debug, Default)]
struct Lowering {
    materials: u64,
    /// `mask` textures (Studio MSK layout: R metallic, G smoothness, B AO) left out: glTF packs metal-rough
    /// differently, so they need a texture pass first.
    mask_not_lowered: u64,
    /// `wrap: clip` written as clamp (glTF has no clip-to-transparent).
    clip_as_clamp: u64,
    /// Normal maps whose convention is not glTF's (`green: down` or `z: rebuild`): left out until a texture pass
    /// converts them.
    normal_not_gltf: u64,
}

fn file_ref(v: &Value) -> Result<Vec<String>, BoxError> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Object(o) => Ok(vec![
            o["file"]
                .as_str()
                .ok_or("file reference without file")?
                .to_owned(),
        ]),
        Value::Array(a) => Ok(a.iter().map(file_ref).collect::<Result<Vec<_>, _>>()?.concat()),
        _ => Err(format!("bad file reference {v}").into()),
    }
}

fn opaque(world: &mut World, e: Entity, name: &str, v: Value) {
    world.insert_opaque(
        e,
        name,
        Opaque {
            version: 1,
            data: OpaqueData::Json(v),
        },
    );
}

fn read_scene(folder: &Path) -> Result<(World, String, Lowering), BoxError> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(folder.join("scene.json"))?)?;
    if manifest["format"] != "skate-scene" || manifest["version"] != 1 {
        return Err("not a skate-scene version 1 manifest".into());
    }
    let mut w = World::new();
    let mut scene_extras = Map::new();
    for (k, v) in manifest.as_object().ok_or("manifest is not an object")? {
        if !matches!(
            k.as_str(),
            "materials" | "textures" | "prototypes" | "placement_sources" | "light_sources"
        ) {
            scene_extras.insert(k.clone(), v.clone());
        }
    }
    w.insert_opaque_resource(
        "skate.scene",
        Opaque {
            version: 1,
            data: OpaqueData::Json(Value::Object(scene_extras)),
        },
    );

    let mut textures: BTreeMap<String, (Entity, Value)> = BTreeMap::new();
    for (key, t) in manifest["textures"].as_object().into_iter().flatten() {
        let e = w.spawn();
        let file = t["file"]
            .as_str()
            .ok_or_else(|| format!("texture {key} without file"))?;
        w.insert(e, TextureFile { path: file.into() });
        w.insert(e, Name(t["name"].as_str().unwrap_or(key).to_owned()));
        if let Some(s) = t.get("source").and_then(Value::as_str) {
            w.insert(e, Source(s.to_owned()));
        }
        opaque(&mut w, e, "skate.texture", t.clone());
        textures.insert(key.clone(), (e, t.clone()));
    }

    let mut lowering = Lowering::default();
    let mut materials: BTreeMap<String, Entity> = BTreeMap::new();
    for (key, m) in manifest["materials"].as_object().into_iter().flatten() {
        let e = w.spawn();
        w.insert(e, Name(key.clone()));
        if let Some(s) = m.get("source").and_then(Value::as_str) {
            w.insert(e, Source(s.to_owned()));
        }
        w.insert(e, lower_skate_material(m, &textures, &mut lowering)?);
        opaque(&mut w, e, "skate.material", m.clone());
        materials.insert(key.clone(), e);
    }
    let material = |k: &Value| -> Result<Option<Entity>, BoxError> {
        match k {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(
                *materials.get(s).ok_or_else(|| format!("unknown material {s}"))?,
            )),
            _ => Err(format!("bad material key {k}").into()),
        }
    };

    let mut prototypes = Vec::new();
    for p in manifest["prototypes"].as_array().into_iter().flatten() {
        let e = w.spawn();
        let id = p["id"].as_str().ok_or("prototype without id")?;
        w.insert(e, Name(p["name"].as_str().unwrap_or(id).to_owned()));
        if let Some(s) = p.get("source").and_then(Value::as_str) {
            w.insert(e, Source(s.to_owned()));
        }
        let parts = p["parts"]
            .as_array()
            .ok_or_else(|| format!("prototype {id} without parts"))?;
        if let Some(glb) = p["glb"].as_str() {
            let mats = parts
                .iter()
                .map(|q| material(&q["material"]))
                .collect::<Result<Vec<_>, _>>()?;
            w.insert(
                e,
                MeshFile {
                    path: glb.into(),
                    mesh: p.get("mesh").and_then(Value::as_u64).unwrap_or(0) as u32,
                    materials: mats,
                },
            );
            if parts.iter().any(|q| q.get("variants").is_some()) {
                let mut variants = Vec::new();
                for q in parts {
                    let mut map = BTreeMap::new();
                    for (v, k) in q.get("variants").and_then(Value::as_object).into_iter().flatten() {
                        if let Some(m) = material(k)? {
                            map.insert(v.parse::<u32>()?, m);
                        }
                    }
                    variants.push(map);
                }
                w.insert(e, MaterialVariants(variants));
            }
        }
        let mut rest = p.as_object().cloned().unwrap_or_default();
        for k in ["glb", "mesh", "name", "source"] {
            rest.remove(k);
        }
        opaque(&mut w, e, "skate.prototype", Value::Object(rest));
        prototypes.push(e);
    }

    let sources: Vec<String> = manifest["placement_sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| s.as_str().unwrap_or_default().to_owned())
        .collect();
    let files = match manifest.get("placements") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => file_ref(v)?,
    };
    for f in files {
        let b = std::fs::read(folder.join(&f))?;
        if b.len() < 16 || &b[..4] != b"SKST" || &b[8..12] != b"PLAC" || u32_at(&b, 4) != 1 {
            return Err(format!("{f}: not a PLAC table").into());
        }
        let n = u32_at(&b, 12) as usize;
        if b.len() != 16 + 76 * n {
            return Err(format!("{f}: {} bytes for {n} placements", b.len()).into());
        }
        for i in 0..n {
            let r = &b[16 + 76 * i..16 + 76 * (i + 1)];
            let proto = *prototypes
                .get(u32_at(r, 0) as usize)
                .ok_or_else(|| format!("{f}: placement {i} prototype"))?;
            let f32_at = |o: usize| f32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
            let cols: [[f32; 3]; 4] =
                std::array::from_fn(|c| std::array::from_fn(|k| f32_at(8 + 12 * c + 4 * k)));
            let flags = u32_at(r, 68);
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: u32_at(r, 4),
                },
            );
            w.insert(e, Transform { cols });
            if flags & 1 != 0 {
                w.insert(
                    e,
                    Tile {
                        col: u32_at(r, 56) as i32,
                        row: u32_at(r, 60) as i32,
                    },
                );
            }
            let src = u32_at(r, 72);
            if src != u32::MAX {
                let s = sources
                    .get(src as usize)
                    .ok_or_else(|| format!("{f}: placement {i} source"))?;
                w.insert(e, Source(s.clone()));
            }
            if flags != 0 {
                let mut extra = json!({"flags": flags});
                if flags & 2 != 0 {
                    extra["radius"] = json!(f32_at(64));
                }
                opaque(&mut w, e, "skate.placement", extra);
            }
        }
    }
    let name = manifest["display_name"]
        .as_str()
        .or(manifest["name"].as_str())
        .unwrap_or("scene")
        .to_owned();
    Ok((w, name, lowering))
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// sRGB-encoded byte -> linear (IEC 61966-2-1 piecewise curve), in f64, rounded once to f32.
fn srgb_to_linear(c: f64) -> f32 {
    let c = c / 255.0;
    (if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }) as f32
}

/// `skate.material` (skate-scene v1 manifest entry) -> `pbr.material`, with the class defaults of skate-scene v1:
/// alpha `mask` for cutout/leaves, `blend` for decal/glass/water/alpha; cutoff 0.5 (leaves 0.33); two-sided for
/// cutout/leaves; metallic 0; roughness 0.85 (glass/water 0.15). `base` -> base colour texture (sRGB); without it
/// `base_color` (sRGB bytes, default 128 grey) -> the factor; `tint` (sRGB bytes, multiplied in linear light)
/// -> the factor, which glTF multiplies into the texture in linear light too; `normal` -> normal texture when the
/// texture is in glTF's convention; `wrap` -> the base texture's sampler. Not lowered (counted): `mask`.
fn lower_skate_material(
    m: &Value,
    textures: &BTreeMap<String, (Entity, Value)>,
    count: &mut Lowering,
) -> Result<PbrMaterial, BoxError> {
    count.materials += 1;
    let class = m["class"].as_str().ok_or("material without class")?;
    let alpha = m["alpha"].as_str().unwrap_or(match class {
        "cutout" | "leaves" => "mask",
        "decal" | "glass" | "water" | "alpha" => "blend",
        _ => "opaque",
    });
    let cutoff = m["cutoff"]
        .as_f64()
        .unwrap_or(if class == "leaves" { 0.33 } else { 0.5 }) as f32;
    let tex = |k: &str| -> Result<&(Entity, Value), BoxError> {
        textures
            .get(k)
            .ok_or_else(|| format!("unknown texture {k}").into())
    };
    let wrap = match m["wrap"].as_str().unwrap_or("repeat") {
        "repeat" => Wrap::Repeat,
        "extend" => Wrap::Clamp,
        "clip" => {
            count.clip_as_clamp += 1;
            Wrap::Clamp
        }
        "mirror" => Wrap::Mirror,
        other => return Err(format!("unknown wrap {other}").into()),
    };
    let bytes = |v: &Value| -> Option<Vec<f64>> { v.as_array()?.iter().map(Value::as_f64).collect() };
    let mut base_color = [1.0f32; 4];
    let base_color_texture = match m["base"].as_str() {
        Some(k) => Some(TextureRef {
            texture: tex(k)?.0,
            texcoord: 0,
            wrap: [wrap; 2],
        }),
        None => {
            let c = bytes(&m["base_color"]).unwrap_or_else(|| vec![128.0, 128.0, 128.0, 255.0]);
            for i in 0..3 {
                base_color[i] = srgb_to_linear(c[i]);
            }
            base_color[3] = (c.get(3).copied().unwrap_or(255.0) / 255.0) as f32;
            None
        }
    };
    if let Some(t) = bytes(&m["tint"]) {
        for i in 0..3 {
            base_color[i] *= srgb_to_linear(t[i]);
        }
    }
    let normal_texture = match m["normal"].as_str() {
        Some(k) => {
            let (e, t) = tex(k)?;
            if t["green"].as_str().unwrap_or("up") == "up" && t["z"].as_str().unwrap_or("stored") == "stored"
            {
                Some(TextureRef {
                    texture: *e,
                    texcoord: 0,
                    wrap: [Wrap::Repeat; 2],
                })
            } else {
                count.normal_not_gltf += 1;
                None
            }
        }
        None => None,
    };
    if m.get("mask").is_some() {
        count.mask_not_lowered += 1;
    }
    let glassy = matches!(class, "glass" | "water");
    Ok(PbrMaterial {
        base_color,
        base_color_texture,
        metallic: m["metallic"].as_f64().unwrap_or(0.0) as f32,
        roughness: m["roughness"]
            .as_f64()
            .unwrap_or(if glassy { 0.15 } else { 0.85 }) as f32,
        normal_texture,
        alpha_mode: match alpha {
            "mask" => AlphaMode::Mask { cutoff },
            "blend" => AlphaMode::Blend,
            "opaque" => AlphaMode::Opaque,
            other => return Err(format!("unknown alpha {other}").into()),
        },
        double_sided: m["two_sided"]
            .as_bool()
            .unwrap_or(matches!(class, "cutout" | "leaves")),
        ..PbrMaterial::default()
    })
}
