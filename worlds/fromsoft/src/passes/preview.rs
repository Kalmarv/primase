//! The preview a lowering draws a FromSoft material with, chosen from what the reader states: the single-layer
//! preview textures (base, normal, metallic and emissive maps), the tiling of its base map, and the name-based
//! collision surface. These are approximation choices; `layered` keeps the source for a shader-aware bake.
use crate::materials::Material;

/// A material's preview textures by stem, and the tiling of the base map's binding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub albedo: Option<String>,
    pub normal: Option<String>,
    pub metallic: Option<String>,
    pub emissive: Option<String>,
    /// The base map binding's scale where it is finite and positive; 1 otherwise.
    pub uv_scale: [f32; 2],
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
/// Deterministic single-layer preview policy: slot 8, otherwise the lowest available base-colour slot; the
/// available normal map of the same texture family, otherwise slots 3, 0, then the lowest; the first metallic and
/// emissive bindings. Among bindings of one slot, the selected MTD's own declarations come first.
pub fn preview(m: &Material) -> Preview {
    let mut p = Preview {
        uv_scale: [1.; 2],
        ..Preview::default()
    };
    let mut metallic = None;
    let mut emissive = None;
    let mut albedo = Vec::new();
    let mut normal = Vec::new();
    for (kind, b) in &m.bindings {
        let (s, role) = slot(kind);
        match role {
            "AlbedoMap" | "g_DiffuseTexture" | "Albedo1" if b.available => albedo.push((s, &b.texture, kind)),
            "NormalMap" | "g_BumpmapTexture" if b.available => normal.push((s, &b.texture, kind)),
            "MetallicMap" => {
                metallic.get_or_insert_with(|| b.texture.clone());
            }
            "Emissive1" | "g_EmissiveTexture" => {
                emissive.get_or_insert_with(|| b.texture.clone());
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
            !m.definition.as_ref().is_some_and(|d| d.declares(kind)),
            (*texture).clone(),
            (*kind).clone(),
        )
    };
    albedo.sort_by_key(rank);
    normal.sort_by_key(rank);
    if let Some(a) = albedo.iter().find(|a| a.0 == 8).or(albedo.first()) {
        p.albedo = Some(a.1.clone());
        let scale = m.bindings[a.2].scale;
        if scale.iter().all(|x| x.is_finite() && *x > 0.) {
            p.uv_scale = scale;
        }
        let n = normal
            .iter()
            .find(|n| texture_family(n.1) == texture_family(a.1))
            .or_else(|| normal.iter().find(|n| n.0 == 3))
            .or_else(|| normal.iter().find(|n| n.0 == 0))
            .or(normal.first());
        p.normal = n.map(|n| n.1.clone());
    }
    p.metallic = metallic;
    p.emissive = emissive;
    p
}

/// Whether the material draws its emissive map: an emissive shader with an emissive binding.
pub fn is_emissive(m: &Material, p: &Preview) -> bool {
    m.shader.contains("Emissive") && p.emissive.is_some()
}

/// The material's name-based collision surface ([`surface`] of its MTD, preview base map and class).
pub fn named_surface(m: &Material) -> (String, bool) {
    surface(&m.mtd, preview(m).albedo.as_deref().unwrap_or(""), &m.class)
}

/// Name-based collision-surface approximation, explicitly a lowering policy.
pub fn surface(mtd: &str, albedo: &str, class: &str) -> (String, bool) {
    if matches!(class, "leaves" | "decal" | "alpha" | "additive" | "water") {
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
