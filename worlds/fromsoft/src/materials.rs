//! Explicit main-layer lowering policy, not a substitute for the game's shaders.
//! Complete MTD parameters and resolved slots survive for a later layered bake.
use crate::{
    flver,
    mtd::{MaterialDefinition, Value},
};
use primase_core::world::{Component, Entity};
use primase_skate_scene::components::{MaterialClass, SkateMaterial};
use std::collections::BTreeMap;

pub fn stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_lowercase()
}
fn leaf_stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf.rsplit_once('.').map_or(leaf, |x| x.0).to_owned()
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Binding {
    pub texture: String,
    pub scale: [f32; 2],
    pub uv_number: i32,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct LayeredMaterial {
    pub mtd: String,
    pub definition: MaterialDefinition,
    pub bindings: BTreeMap<String, Binding>,
}
impl Component for LayeredMaterial {
    const NAME: &'static str = "fromsoft.layered_material";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Material {
    pub mtd: String,
    pub shader: String,
    pub class: String,
    pub skip: Option<String>,
    pub albedo: Option<String>,
    pub normal: Option<String>,
    pub metallic: Option<String>,
    pub emissive: Option<String>,
    pub cutoff: f32,
    pub two_sided: bool,
    pub is_emissive: bool,
    pub uv_scale: [f32; 2],
    pub surface: String,
    pub natural: bool,
    pub bindings: BTreeMap<String, Binding>,
    pub layered: Option<LayeredMaterial>,
}
impl Component for Material {
    const NAME: &'static str = "fromsoft.material";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
fn slot(kind: &str) -> (u32, &str) {
    if let Some((_, rest)) = kind.rsplit_once("Texture2D_")
        && let Some((index, role)) = rest.split_once('_')
        && let Ok(index) = index.parse()
    {
        return (index, role.split('_').next().unwrap_or(role));
    }
    (0, kind)
}
fn texture_family(name: &str) -> &str {
    name.rsplit_once('_').map_or(name, |x| x.0)
}
/// Deterministic single-layer preview policy: slot 8, otherwise lowest albedo
/// slot; matching normal stem, otherwise slots 3, 0, then lowest normal slot.
/// Availability comes from the caller's texture index. These are approximation
/// choices; `layered` retains the source for shader-aware lowering later.
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
        shader: definition.map_or(String::new(), |d| leaf_stem(&d.shader)),
        class: "opaque".into(),
        skip: None,
        albedo: None,
        normal: None,
        metallic: None,
        emissive: None,
        cutoff: 0.5,
        two_sided: false,
        is_emissive: false,
        uv_scale: [1.; 2],
        surface: "default".into(),
        natural: false,
        bindings,
        layered: None,
    };
    let mut albedo = Vec::new();
    let mut normal = Vec::new();
    for (kind, b) in &m.bindings {
        let (s, role) = slot(kind);
        match role {
            "AlbedoMap" | "g_DiffuseTexture" | "Albedo1" if exists(&b.texture) => {
                albedo.push((s, &b.texture, kind))
            }
            "NormalMap" | "g_BumpmapTexture" if exists(&b.texture) => normal.push((s, &b.texture, kind)),
            "MetallicMap" => {
                m.metallic.get_or_insert_with(|| b.texture.clone());
            }
            "Emissive1" | "g_EmissiveTexture" => {
                m.emissive.get_or_insert_with(|| b.texture.clone());
            }
            _ => {}
        }
    }
    // Distinct shader-qualified names can share a graph slot number. Prefer a
    // declaration of the selected MTD over a binding left from another family;
    // retain both in the source component for a later shader-aware bake.
    let rank = |(slot, texture, kind): &(u32, &String, &String)| {
        (
            *slot,
            !definition.is_some_and(|d| d.textures.iter().any(|t| t.kind == **kind)),
            (*texture).clone(),
            (*kind).clone(),
        )
    };
    albedo.sort_by_key(rank);
    normal.sort_by_key(rank);
    if let Some(a) = albedo.iter().find(|a| a.0 == 8).or(albedo.first()) {
        m.albedo = Some(a.1.clone());
        let scale = m.bindings[a.2].scale;
        if scale.iter().all(|x| x.is_finite() && *x > 0.) {
            m.uv_scale = scale;
        }
        let n = normal
            .iter()
            .find(|n| texture_family(n.1) == texture_family(a.1))
            .or_else(|| normal.iter().find(|n| n.0 == 3))
            .or_else(|| normal.iter().find(|n| n.0 == 0))
            .or(normal.first());
        m.normal = n.map(|n| n.1.clone());
    }
    let sh = &m.shader;
    if sh.starts_with("M[Sky]") || sh.contains("CloudFog") || sh.contains("GroundFog") {
        m.skip = Some("sky / cloud / fog card".into());
    } else if sh.starts_with("VolumeTest") || blend == 4 {
        m.skip = Some("additive light volume".into());
    } else if sh.starts_with("GXFlver_Shadow") || m.mtd.starts_with("M[Shadow]") {
        m.skip = Some("shadow proxy".into());
    } else if sh.contains("Tessellation_Water") {
        m.class = "water".into();
    } else if sh.contains("MeshDecal") || m.mtd.ends_with("_md") {
        m.class = "decal".into();
    } else if sh.starts_with("M[Translucency]") {
        m.class = "leaves".into();
        m.two_sided = true;
        if aref > 0 {
            m.cutoff = aref as f32 / 255.;
        }
    } else if blend == 2 {
        m.class = "alpha".into();
    } else if aref > 0 {
        m.class = "cutout".into();
        m.two_sided = true;
        m.cutoff = aref as f32 / 255.;
    }
    m.is_emissive = sh.contains("Emissive") && m.emissive.is_some();
    (m.surface, m.natural) = surface(&m.mtd, m.albedo.as_deref().unwrap_or(""), &m.class);
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
/// Name-based collision-surface approximation, explicitly a lowering policy.
pub fn surface(mtd: &str, albedo: &str, class: &str) -> (String, bool) {
    if matches!(class, "leaves" | "decal" | "alpha" | "water") {
        return ("default".into(), false);
    }
    let text = format!("{mtd} {albedo}").to_lowercase();
    for (words, surface, natural) in [
        (&["roof", "tile_", "kawara"][..], "roofing", false),
        (&["rock", "cliff", "mountain", "landscape"][..], "concrete", true),
        (
            &["snow", "ground", "soil", "earth", "moss", "mud"][..],
            "earth",
            false,
        ),
        (&["sand"][..], "sand", false),
        (&["grass"][..], "grass", false),
        (
            &[
                "wood", "floor", "log", "bamboo", "saku", "tatami", "plank", "branch", "bark",
            ][..],
            "wood",
            false,
        ),
        (&["metal", "iron", "armor", "cannon"][..], "metal", false),
        (
            &["stone", "step", "wall", "dobei", "plaster", "pot", "jizo"][..],
            "concrete",
            false,
        ),
    ] {
        if words.iter().any(|w| text.contains(w)) {
            return (surface.into(), natural);
        }
    }
    ("default".into(), false)
}
impl Material {
    /// Resolve roles separately: a normal and colour use of the same file need
    /// different texture metadata. Missing base data gets a deterministic colour.
    pub fn skate_material(
        &self,
        key: String,
        lookup: impl Fn(&str, bool) -> Option<Entity>,
    ) -> SkateMaterial {
        let base = self.albedo.as_deref().and_then(|n| lookup(n, false));
        let normal = self.normal.as_deref().and_then(|n| lookup(n, true));
        let class = match self.class.as_str() {
            "cutout" => MaterialClass::Cutout,
            "leaves" => MaterialClass::Leaves,
            "decal" => MaterialClass::Decal,
            "alpha" => MaterialClass::Alpha,
            "water" => MaterialClass::Water,
            _ => MaterialClass::Opaque,
        };
        let masked = matches!(class, MaterialClass::Cutout | MaterialClass::Leaves);
        SkateMaterial {
            key,
            class,
            base,
            normal,
            mask: None,
            base_color: base.is_none().then_some(if self.class == "water" {
                [48, 98, 122, 190]
            } else {
                [128, 128, 128, 255]
            }),
            tint: None,
            alpha: None,
            cutoff: masked.then_some(f64::from(self.cutoff)),
            two_sided: (self.two_sided != masked).then_some(self.two_sided),
            domain: None,
            metallic: None,
            roughness: None,
            wrap: None,
            ocean: None,
            cast_shadows: None,
            emissive: self.is_emissive.then_some(true),
            other: Default::default(),
        }
    }
}
