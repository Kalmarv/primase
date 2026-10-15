//! Static normal-map contribution shared by the inspected G-buffer families.
//! The shaders read R/G as `2*v-1`, reconstruct positive Z and form
//! `R * normalize(cross(N,T)*w) + G * normalize(T) + Z * normalize(N)`.
//! Therefore glTF with the stored tangent uses swapped R/G, green-up, and
//! reflected tangent handedness. This is not a texture-authorship curl rule.
use crate::{Result, invalid};
use primase_core::{NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, Texture};
use primase_texcodec::{Decoder, TexDecoder};

/// Whether this family's static normal basis was observed in installed shaders.
/// The three legacy names are an explicit assumption: their shader bundles are
/// absent from the inspected install. Dynamic water buffers and layer blending
/// are outside this static texture conversion.
pub fn consumer_basis(shader: &str) -> Result<bool> {
    Ok(match shader {
        "M[Multiple]"
        | "M[MultipleGround]"
        | "MetallicValue"
        | "MetallicTex"
        | "MetallicValue_MeshDecal"
        | "M[Multiple]_Emissive"
        | "M[Translucency]_e"
        | "M[Translucency]_e_cloth"
        | "M[AMSN]_VA_e"
        | "M[Emissive]_Simple"
        | "M[Interaction]_Tessellation_Water"
        | "M[Mb1]_Wetness_CoverdWorld"
        | "M[Shell]"
        | "SnowTest_mb1"
        | "SnowTest_mb1_ver1"
        | "SnowTest_mb2"
        | "SnowTest_mb2_ver1"
        | "WallTest_dobei"
        | "WallTest_dobei_ver1"
        | "WallTest_dobei_ver2" => true,
        "GXFlver_Col" | "GXFlver_ColDif" | "GXFlver_ColDifSpcBumpIbl" => false,
        _ => {
            return Err(invalid(
                "Sekiro normal basis",
                0,
                format!("unclassified shader {shader}"),
            ));
        }
    })
}
/// Swap the two encoded tangent-plane coordinates once, preserving every mip.
/// Z remains explicitly marked for reconstruction by the shared image pass.
pub fn swap_normal_axes(texture: &mut Texture) -> Result<()> {
    let mut levels = Vec::with_capacity(texture.levels.len());
    for (i, data) in texture.levels.iter().enumerate() {
        let w = texture.width.checked_shr(i as u32).unwrap_or(0).max(1);
        let h = texture.height.checked_shr(i as u32).unwrap_or(0).max(1);
        let mut rgba = Decoder::default()
            .decode_rgba8(texture.format, w, h, data)
            .map_err(|e| invalid("Sekiro normal map", 0, e))?;
        for p in rgba.as_chunks_mut::<4>().0 {
            p.swap(0, 1);
        }
        levels.push(rgba);
    }
    texture.format = TexFormat::Rgba8Unorm;
    texture.levels = levels;
    texture.normal = Some(NormalConvention {
        green: NormalGreen::Up,
        z: NormalZ::Rebuild,
        x: NormalX::Red,
    });
    Ok(())
}
