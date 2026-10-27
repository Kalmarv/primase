//! A Universal Render Pipeline asset's lighting and shadow settings, decoded from the bytes of its
//! `UniversalRenderPipelineAsset` MonoBehaviour (a build has no type tree for a script's fields).
//!
//! The field order is the class's declaration order in the package (checked against the 17.6.0 source and against an
//! asset of a player: the 452 bytes of its three assets read to the same values as the keywords of the variants the
//! player stores for them, `tests/resolve_shaders.rs`). It holds for `k_AssetVersion` 13 only; another version is
//! refused. A field is a 4-byte value, or a bool padded to 4 bytes; an object reference is 12 bytes (file id and a 64-bit
//! path id) and an array is its count and elements. The class's later fields (batching, post-processing, the GPU
//! resident drawer) are not read, and the layout past the first of them is not verified.

use crate::reader::Reader;
use serde::Serialize;

use crate::{Result, invalid};

/// The lighting and shadow settings of a Universal Render Pipeline asset as stored (`UniversalRenderPipelineAsset`,
/// asset version 13; [`crate::pipeline`] holds the verified layout): the light modes as `LightRenderingMode`
/// numbers them (0 disabled, 1 per pixel, 2 per vertex), the shadow cascade count (1 to 4), the settings that decide
/// which of a pass's keywords the pipeline sets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityUrpSettings {
    pub main_light_mode: i64,
    pub main_light_shadows: bool,
    pub additional_lights_mode: i64,
    pub additional_light_shadows: bool,
    pub reflection_probe_blending: bool,
    pub reflection_probe_box_projection: bool,
    pub reflection_probe_atlas: bool,
    pub shadow_distance: f32,
    pub shadow_cascades: u8,
    pub soft_shadows: bool,
    pub mixed_lighting: bool,
    pub light_cookies: bool,
    pub light_layers: bool,
    pub lod_cross_fade: bool,
    /// `ShEvalMode`: 0 auto, 1 per vertex, 2 mixed, 3 per pixel.
    pub sh_eval_mode: i64,
    /// `LightProbeSystem`: 0 legacy light probes, 1 probe volumes.
    pub light_probe_system: i64,
}

/// The asset version the layout below is verified for.
const ASSET_VERSION: i32 = 13;

/// The settings of the asset whose script fields are `bytes` (the bytes after the MonoBehaviour's base fields).
pub fn read_urp_settings(bytes: &[u8]) -> Result<UnityUrpSettings> {
    let mut r = Reader::new(bytes, false);
    let flag = |r: &mut Reader| -> Result<bool> {
        let b = r.bool()?;
        r.align(4)?;
        Ok(b)
    };
    let version = r.i32()?;
    if version != ASSET_VERSION {
        return Err(invalid(format!(
            "Universal Render Pipeline asset version {version}: only version {ASSET_VERSION} has a verified layout"
        )));
    }
    r.i32()?; // k_AssetPreviousVersion
    r.i32()?; // m_RendererType
    r.take(12)?; // m_RendererData (a reference)
    let renderers = r.u32()?;
    r.take(12 * usize::try_from(renderers).map_err(|_| invalid("renderer list too long"))?)?;
    r.i32()?; // m_DefaultRendererIndex
    r.take(8)?; // m_RequireDepthTexture, m_RequireOpaqueTexture
    r.i32()?; // m_OpaqueDownsampling
    r.take(8)?; // m_SupportsTerrainHoles, m_SupportsHDR
    r.i32()?; // m_HDRColorBufferPrecision
    r.i32()?; // m_MSAA
    r.f32()?; // m_RenderScale
    r.i32()?; // m_UpscalingFilter
    r.take(4)?; // m_FsrOverrideSharpness
    r.f32()?; // m_FsrSharpness
    let lod_cross_fade = flag(&mut r)?;
    r.i32()?; // m_LODCrossFadeDitheringType
    let sh_eval_mode = r.i32()?;
    let light_probe_system = r.i32()?;
    r.i32()?; // m_ProbeVolumeMemoryBudget
    r.i32()?; // m_ProbeVolumeBlendingMemoryBudget
    r.take(16)?; // the four probe volume streaming and scenario flags
    r.i32()?; // m_ProbeVolumeSHBands
    let main_light_mode = r.i32()?;
    let main_light_shadows = flag(&mut r)?;
    r.i32()?; // m_MainLightShadowmapResolution
    let additional_lights_mode = r.i32()?;
    r.i32()?; // m_AdditionalLightsPerObjectLimit
    let additional_light_shadows = flag(&mut r)?;
    r.take(16)?; // the additional lights' shadowmap resolution and its three tiers
    let reflection_probe_blending = flag(&mut r)?;
    let reflection_probe_box_projection = flag(&mut r)?;
    let reflection_probe_atlas = flag(&mut r)?;
    let shadow_distance = r.f32()?;
    let shadow_cascades = r.i32()?;
    r.take(4 + 8 + 12 + 4 + 4 + 4)?; // the cascade splits and border, the depth and normal bias
    let soft_shadows = flag(&mut r)?;
    r.take(4)?; // m_ConservativeEnclosingSphere
    r.i32()?; // m_NumIterationsEnclosingSphere
    r.i32()?; // m_SoftShadowQuality
    r.i32()?; // m_AdditionalLightsCookieResolution
    r.i32()?; // m_AdditionalLightsCookieFormat
    r.take(8)?; // m_UseSRPBatcher, m_SupportsDynamicBatching
    let mixed_lighting = flag(&mut r)?;
    let light_cookies = flag(&mut r)?;
    let light_layers = flag(&mut r)?;
    // a layout that does not hold reads values outside what the fields can be
    let in_range = |what: &str, ok: bool| -> Result<()> {
        ok.then_some(()).ok_or_else(|| {
            invalid(format!(
                "Universal Render Pipeline asset: {what} out of range (an unverified layout)"
            ))
        })
    };
    in_range("m_ShadowCascadeCount", (1..=4).contains(&shadow_cascades))?;
    in_range(
        "m_ShadowDistance",
        shadow_distance.is_finite() && shadow_distance >= 0.0,
    )?;
    in_range("m_MainLightRenderingMode", (0..=2).contains(&main_light_mode))?;
    in_range(
        "m_AdditionalLightsRenderingMode",
        (0..=2).contains(&additional_lights_mode),
    )?;
    in_range("m_ShEvalMode", (0..=3).contains(&sh_eval_mode))?;
    Ok(UnityUrpSettings {
        main_light_mode: i64::from(main_light_mode),
        main_light_shadows,
        additional_lights_mode: i64::from(additional_lights_mode),
        additional_light_shadows,
        reflection_probe_blending,
        reflection_probe_box_projection,
        reflection_probe_atlas,
        shadow_distance,
        shadow_cascades: shadow_cascades as u8,
        soft_shadows,
        mixed_lighting,
        light_cookies,
        light_layers,
        lod_cross_fade,
        sh_eval_mode: i64::from(sh_eval_mode),
        light_probe_system: i64::from(light_probe_system),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic asset in the layout: each field at the position the real bytes of a player's asset put it.
    fn asset(cascades: i32) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        let int = |b: &mut Vec<u8>, v: i32| b.extend(v.to_le_bytes());
        let float = |b: &mut Vec<u8>, v: f32| b.extend(v.to_le_bytes());
        let bool4 = |b: &mut Vec<u8>, v: bool| b.extend(i32::from(v).to_le_bytes());
        int(&mut b, 13); // k_AssetVersion
        int(&mut b, 13);
        int(&mut b, 1); // renderer type
        b.extend([0; 12]); // m_RendererData
        int(&mut b, 1); // one renderer
        b.extend([0; 12]);
        int(&mut b, 0); // default renderer
        bool4(&mut b, true); // depth texture
        bool4(&mut b, false); // opaque texture
        int(&mut b, 1);
        bool4(&mut b, false); // terrain holes
        bool4(&mut b, true); // HDR
        int(&mut b, 0);
        int(&mut b, 4); // MSAA
        float(&mut b, 1.0);
        int(&mut b, 0); // upscaling filter
        bool4(&mut b, false);
        float(&mut b, 0.92);
        bool4(&mut b, true); // LOD cross fade
        int(&mut b, 1);
        int(&mut b, 2); // SH evaluation: mixed
        int(&mut b, 0); // legacy light probes
        int(&mut b, 1024);
        int(&mut b, 256);
        for _ in 0..4 {
            bool4(&mut b, false);
        }
        int(&mut b, 1); // SH bands
        int(&mut b, 1); // main light: per pixel
        bool4(&mut b, true); // main light shadows
        int(&mut b, 4096);
        int(&mut b, 1); // additional lights: per pixel
        int(&mut b, 8);
        bool4(&mut b, true); // additional light shadows
        for v in [1024, 256, 512, 1024] {
            int(&mut b, v);
        }
        bool4(&mut b, true); // reflection probe blending
        bool4(&mut b, false); // box projection
        bool4(&mut b, true); // atlas
        float(&mut b, 400.0);
        int(&mut b, cascades);
        float(&mut b, 0.25);
        for v in [0.1, 0.3, 0.123, 0.29, 0.54, 0.11, 0.1, 0.5] {
            float(&mut b, v);
        }
        bool4(&mut b, true); // soft shadows
        bool4(&mut b, true); // conservative sphere
        int(&mut b, 64);
        int(&mut b, 3);
        int(&mut b, 2048);
        int(&mut b, 3);
        bool4(&mut b, true);
        bool4(&mut b, false);
        bool4(&mut b, true); // mixed lighting
        bool4(&mut b, false); // light cookies
        bool4(&mut b, true); // light layers
        b
    }

    /// The fields the keywords depend on are read from where the layout puts them, bools padded to 4 bytes and the
    /// renderer list's length moving what follows it.
    #[test]
    fn a_urp_assets_settings_are_read_by_the_verified_layout() {
        let s = read_urp_settings(&asset(4)).unwrap();
        assert_eq!((s.shadow_cascades, s.shadow_distance), (4, 400.0));
        assert!(s.main_light_shadows && s.soft_shadows && s.additional_light_shadows);
        assert!(
            s.reflection_probe_blending && !s.reflection_probe_box_projection && s.reflection_probe_atlas
        );
        assert!(s.mixed_lighting && !s.light_cookies && s.light_layers && s.lod_cross_fade);
        assert_eq!(
            (s.main_light_mode, s.additional_lights_mode, s.sh_eval_mode),
            (1, 1, 2)
        );
        // another version, and a value no field can hold
        let mut other = asset(4);
        other[0] = 12;
        assert!(read_urp_settings(&other).is_err());
        assert!(read_urp_settings(&asset(7)).is_err());
        assert!(read_urp_settings(&asset(4)[..100]).is_err());
    }
}
