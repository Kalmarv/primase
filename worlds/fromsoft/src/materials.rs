//! FLVER materials resolved with their MTD definitions as source records ([`resolve`]); the records themselves are
//! components ([`Material`]). Complete MTD parameters and resolved slots survive for a later layered bake.
pub use crate::components::materials::{Binding, LayeredMaterial, Material, SkipClass, identity_digest};
use crate::{
    flver,
    mtd::{MaterialDefinition, Value},
};
use std::collections::BTreeMap;

pub fn stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_lowercase()
}
fn leaf_stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_owned()
}
/// A FLVER material entry with its MTD `definition` as the source states it: its bindings (each with whether
/// `exists` finds its texture, the caller's texture catalog), its shader, the class its shader family states
/// (water, decal, leaves, alpha by blend mode, cutout by alpha reference) or the skip class of one the game draws as
/// something other than a surface, its alpha reference and, for the layered shaders, its layered definition. No
/// texture is picked here: the preview a lowering draws is [`crate::preview`]'s.
pub fn resolve(
    source: &flver::Material,
    definition: Option<&MaterialDefinition>,
    exists: impl Fn(&str) -> bool,
) -> Material {
    let mut bindings = BTreeMap::new();
    if let Some(d) = definition {
        for t in &d.textures {
            if !t.path.trim().is_empty() {
                bindings.insert(
                    t.kind.clone(),
                    Binding {
                        available: exists(&stem(&t.path)),
                        texture: stem(&t.path),
                        scale: [1.; 2],
                        uv_number: t.uv_number,
                    },
                );
            }
        }
    }
    for t in &source.textures {
        if !t.path.trim().is_empty() {
            let uv_number = definition
                .and_then(|d| d.textures.iter().find(|x| x.kind == t.kind))
                .map_or(1, |t| t.uv_number);
            bindings.insert(
                t.kind.clone(),
                Binding {
                    available: exists(&stem(&t.path)),
                    texture: stem(&t.path),
                    scale: t.scale,
                    uv_number,
                },
            );
        }
    }
    let parameter = |name: &str| {
        definition
            .and_then(|d| d.parameters.iter().find(|p| p.name == name))
            .and_then(|p| match &p.value {
                Value::Int(x) => x.first().copied(),
                _ => None,
            })
            .unwrap_or(0)
    };
    let blend = parameter("g_BlendMode");
    let aref = parameter("g_AlphaRef");
    let mut m = Material {
        mtd: leaf_stem(&source.mtd),
        definition: definition.cloned(),
        gx: source.gx.clone(),
        shader: definition.map_or(String::new(), |d| leaf_stem(&d.shader)),
        class: "opaque".into(),
        skip: None,
        cutoff: 0.5,
        bindings,
        layered: None,
    };
    let sh = &m.shader;
    if sh.starts_with("M[Sky]") || sh.contains("CloudFog") || sh.contains("GroundFog") {
        m.skip = Some(SkipClass::Sky);
    } else if sh.starts_with("VolumeTest") || blend == 4 {
        m.skip = Some(SkipClass::LightVolume);
    } else if sh.starts_with("GXFlver_Shadow") || m.mtd.starts_with("M[Shadow]") {
        m.skip = Some(SkipClass::Shadow);
    } else if sh.contains("Tessellation_Water") {
        m.class = "water".into();
    } else if sh.contains("MeshDecal") || m.mtd.ends_with("_md") {
        m.class = "decal".into();
    } else if sh.starts_with("M[Translucency]") {
        m.class = "leaves".into();
        if aref > 0 {
            m.cutoff = aref as f32 / 255.;
        }
    } else if blend == 2 {
        m.class = "alpha".into();
    } else if aref > 0 {
        m.class = "cutout".into();
        m.cutoff = aref as f32 / 255.;
    }
    if let Some(d) = definition
        && sh.starts_with("M[Multiple")
        && m.skip.is_none()
        && d.parameters.iter().any(|p| {
            p.name == "GXFT_SATStaticSwitch"
                && matches!(&p.value,Value::Int(v) if v.first().is_some_and(|x|*x>=0))
        })
    {
        m.layered = Some(LayeredMaterial {
            mtd: m.mtd.clone(),
            definition: d.clone(),
            bindings: m.bindings.clone(),
        });
    }
    m
}
