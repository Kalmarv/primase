//! What a GTA shader draws, from its effect name and render bucket: the earlier skate exporter's shader table, on
//! Enhanced's effect names.
//!
//! Enhanced stores no preset (`.sps`) name: every shader's preset word is the same (census), so the effect name and
//! the render bucket are all there is, which is also what that exporter fell back to on Enhanced. The rules and the
//! evidence for each:
//!
//! - opacity: the pixel shaders of every effect but [`NO_VERTEX_ALPHA`] and the terrain, tree, grass, water, ped and
//!   vehicle families multiply the texture's alpha by vertex colour 0's alpha (dxc's dependency table of every pixel
//!   shader of the game's shader database); GTA fades overlay edges and weathering that way;
//! - two-sided: only the four `cloth_*` effects set the effect record's flag 8 after their shader lists (seen from
//!   behind in the game, a cloth banner's text is mirrored);
//! - `*_geomnightonly`: the vertex shader multiplies the position by (night factor > 0 ? 1 : 0), so by day the
//!   geometry is not there;
//! - `*_tnt`: the vertex shader samples the tint palette at u = vertex colour blue / 255 (the second vertex colour's
//!   for three tree effects), v = the entity's tint;
//! - effects with nothing a surface material can draw: decals that only change the normal, specular, shadow or
//!   ambient of what is under them, the dirt decal (a darkening mask), cables (lines widened by the camera distance in
//!   the vertex shader), instanced grass batches and shell fur (both built in the vertex shader), the tree shadow
//!   proxy (drawn into shadow maps only) and effects that are not world geometry (particles, clouds, sky, radar,
//!   minimap, silhouettes, `albedo_alpha`).

/// How a shader's surface is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Opaque.
    Opaque,
    /// Alpha tested.
    Cutout,
    /// Alpha blended.
    Alpha,
    /// Blended over the surface it lies on (render bucket 2, `*decal*` but the ped effects, `distance_map`).
    Decal,
    /// Blended glass (`glass*`).
    Glass,
    /// Water (`water*`, render bucket 6).
    Water,
    /// A blend of four layers weighted per vertex (`terrain_cb_*`).
    Terrain,
    /// Alpha-tested foliage (`trees*`, `grass*`).
    Foliage,
}

/// Why a shader's geometry has no surface material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// `*_geomnightonly`: drawn only at night.
    NightOnlyGeometry,
    /// A decal that changes only the normal, specular, shadow or ambient light of the surface under it.
    NoColourDecal,
    /// `decal_dirt`: a darkening mask.
    DirtDecal,
    /// `cable`: lines the vertex shader widens by camera distance.
    Cable,
    /// `grass_batch*`: instanced grass built in the vertex shader.
    GrassBatch,
    /// `grass_fur*`: shell fur built in the vertex shader.
    Fur,
    /// `trees_shadow_proxy`: drawn into shadow maps only.
    ShadowProxy,
    /// Particles, clouds, sky, radar, minimap, silhouettes, `albedo_alpha`.
    NotWorldGeometry,
}

impl Skip {
    /// A pass log line.
    pub fn reason(self) -> &'static str {
        match self {
            Skip::NightOnlyGeometry => "geometry drawn only at night",
            Skip::NoColourDecal => "decal without a colour (normal, specular, shadow or ambient only)",
            Skip::DirtDecal => "dirt decal (a darkening mask)",
            Skip::Cable => "cable (lines widened by camera distance)",
            Skip::GrassBatch => "instanced grass batch",
            Skip::Fur => "shell fur",
            Skip::ShadowProxy => "tree shadow proxy (shadow maps only)",
            Skip::NotWorldGeometry => "not world geometry",
        }
    }
}

/// Where a tinted shader's palette column comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TintColumn {
    /// Vertex colour 0's blue.
    Color0,
    /// Vertex colour 1's blue (`trees_tnt`, `trees_normal_diffspec_tnt`, `trees_normal_spec_tnt`).
    Color1,
}

/// What a shader draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderClass {
    pub class: Class,
    /// Set when the geometry has no surface material.
    pub skip: Option<Skip>,
    /// The texture's alpha is multiplied by vertex colour 0's alpha.
    pub vertex_alpha: bool,
    /// Drawn from both sides.
    pub two_sided: bool,
    /// Emits light (`*emissive*`).
    pub emissive: bool,
    /// Emits only at night (`*emissivenight*`).
    pub night_emissive: bool,
    /// Coloured from the tint palette (`*_tnt`, `*_tnt_*`).
    pub tint: Option<TintColumn>,
    /// A tree LOD billboard (`trees_lod*`).
    pub lod_billboard: bool,
}

/// Effects whose pixel shaders' alpha does not depend on vertex colour 0's alpha (besides the families
/// [`ShaderClass::vertex_alpha`] leaves out by prefix).
pub const NO_VERTEX_ALPHA: [&str; 10] = [
    "blend_2lyr",
    "cable",
    "decal_normal_blend_2lyr",
    "distance_map",
    "glass_breakable",
    "glass_pv",
    "glass_pv_env",
    "silhouettelayer",
    "vfx_decaldyn",
    "vfx_decaldyn_steep",
];

fn uses_vertex_alpha(effect: &str) -> bool {
    !NO_VERTEX_ALPHA.contains(&effect)
        && !["terrain_", "trees", "grass", "water", "ped", "vehicle_"]
            .iter()
            .any(|p| effect.starts_with(p))
}

/// The class of a shader with effect `effect` (its name) drawn in render bucket `bucket`.
pub fn classify(effect: &str, bucket: u8) -> ShaderClass {
    let f = effect;
    let mut skip = None;
    if f.ends_with("_geomnightonly") {
        skip = Some(Skip::NightOnlyGeometry);
    }
    let mut tint = (f.ends_with("_tnt") || f.contains("_tnt_")).then_some(TintColumn::Color0);
    let class = if f.starts_with("terrain_cb") {
        Class::Terrain
    } else if f.starts_with("water") {
        Class::Water
    } else if f.starts_with("trees") {
        if f == "trees_shadow_proxy" {
            skip = Some(Skip::ShadowProxy);
        }
        if matches!(
            f,
            "trees_tnt" | "trees_normal_diffspec_tnt" | "trees_normal_spec_tnt"
        ) {
            tint = Some(TintColumn::Color1);
        }
        Class::Foliage
    } else if f.starts_with("grass_fur") {
        skip = Some(Skip::Fur);
        Class::Foliage
    } else if f.starts_with("grass") {
        if f.starts_with("grass_batch") {
            skip = Some(Skip::GrassBatch);
        }
        Class::Foliage
    } else if f.starts_with("glass") {
        Class::Glass
    } else {
        if f == "cable" {
            skip = Some(Skip::Cable);
        } else if matches!(
            f,
            "decal_normal_only"
                | "decal_spec_only"
                | "decal_shadow_only"
                | "decal_amb_only"
                | "mirror_decal"
                | "reflect_decal"
                | "spec_decal"
        ) {
            skip = Some(Skip::NoColourDecal);
        } else if f == "decal_dirt" {
            skip = Some(Skip::DirtDecal);
        } else if ["ptfx", "clouds", "sky_"].iter().any(|p| f.starts_with(p))
            || matches!(f, "radar" | "minimap" | "silhouettelayer" | "albedo_alpha")
        {
            skip = Some(Skip::NotWorldGeometry);
        }
        // the ped effects (`ped_decal*`, `ped_nopeddamagedecals`) draw characters, not world surfaces
        if (f.contains("decal") && !f.starts_with("ped_")) || f == "distance_map" {
            Class::Decal
        } else if f.starts_with("cutout") || f.ends_with("_cutout") || f.contains("cutout_") {
            Class::Cutout
        } else if f.ends_with("_alpha") || f.ends_with("_screendooralpha") {
            if bucket == 3 { Class::Cutout } else { Class::Alpha }
        } else {
            match bucket {
                1 => Class::Alpha,
                2 => Class::Decal,
                3 => Class::Cutout,
                6 => Class::Water,
                _ => Class::Opaque,
            }
        }
    };
    ShaderClass {
        class,
        skip,
        vertex_alpha: uses_vertex_alpha(f),
        two_sided: f.starts_with("cloth_"),
        emissive: f.contains("emissive"),
        night_emissive: f.contains("emissivenight"),
        tint,
        lod_billboard: f.starts_with("trees_lod"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_follow_the_effect_then_the_bucket() {
        assert_eq!(classify("default", 0).class, Class::Opaque);
        assert_eq!(classify("default", 3).class, Class::Cutout);
        assert_eq!(classify("normal_spec", 1).class, Class::Alpha);
        assert_eq!(classify("decal_tnt", 2).class, Class::Decal);
        assert_eq!(classify("distance_map", 2).class, Class::Decal);
        assert_eq!(classify("ped_decal", 0).class, Class::Opaque);
        assert_eq!(classify("ped_nopeddamagedecals", 0).class, Class::Opaque);
        assert_eq!(classify("cutout_fence", 3).class, Class::Cutout);
        assert_eq!(classify("cloth_spec_alpha", 3).class, Class::Cutout);
        assert_eq!(classify("glass_pv_env", 1).class, Class::Glass);
        assert_eq!(classify("water_fountain", 6).class, Class::Water);
        assert_eq!(classify("terrain_cb_w_4lyr_pxm", 0).class, Class::Terrain);
        assert_eq!(classify("trees_lod2", 3).class, Class::Foliage);
    }

    #[test]
    fn flags() {
        let c = classify("cloth_normal_spec_tnt", 0);
        assert!(c.two_sided && c.vertex_alpha && c.tint == Some(TintColumn::Color0));
        let t = classify("trees_normal_spec_tnt", 3);
        assert!(!t.vertex_alpha && t.tint == Some(TintColumn::Color1));
        let e = classify("decal_emissivenight_only", 2);
        assert!(e.emissive && e.night_emissive && e.skip.is_none());
        assert!(!classify("glass_pv", 1).vertex_alpha);
        assert!(classify("decal", 2).vertex_alpha);
        assert_eq!(classify("decal_shadow_only", 2).skip, Some(Skip::NoColourDecal));
        assert_eq!(classify("cable", 1).skip, Some(Skip::Cable));
        assert_eq!(classify("grass_fur_mask", 3).skip, Some(Skip::Fur));
        assert_eq!(
            classify("emissivenight_geomnightonly", 0).skip,
            Some(Skip::NightOnlyGeometry)
        );
        assert_eq!(classify("decal_dirt", 2).skip, Some(Skip::DirtDecal));
    }
}
