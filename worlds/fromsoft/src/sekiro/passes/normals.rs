//! Static normal-map contribution shared by the inspected G-buffer families.
//! The shaders read R/G as `2*v-1`, reconstruct positive Z and form
//! `R * normalize(cross(N,T)*w) + G * normalize(T) + Z * normalize(N)`.
//! Therefore glTF with the stored tangent uses swapped R/G, green-up, and
//! reflected tangent handedness. This is not a texture-authorship curl rule.
//!
//! Source normal maps carry [`SOURCE`] against the stored tangents the geometry pass keeps: `normalize-normals`
//! converts them for a writer that reads supplied tangents (glTF), and `uv-frame-normals` re-expresses them for a
//! target that derives its frame from texture coordinates. Neither choice belongs to the source.
use crate::invalid;
use fromsoft_formats::Result;
use primase_core::{NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ};

/// Whether this family's static normal basis was observed in installed shaders.
/// The three legacy names are an explicit assumption: their shader bundles are
/// absent from the inspected shaderbdle bundle; the separate GX shader bundle
/// was not part of that inspection. Dynamic water buffers and layer blending
/// are outside this static texture conversion.
pub fn consumer_basis(shader: &str) -> Result<bool> {
    Ok(match shader {
        // The GX G-buffer program samples t3 RG as 2*v-1: red multiplies
        // normalized cross(N,T)*w, green multiplies normalized T. Positive Z
        // is rebuilt from RG; sampled blue is written separately to o1.x.
        "GXFlver_ColDifSpcBumpEmiIbl"
        | "M[Multiple]"
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
/// Source coordinates relative to the stored tangent carried by the reader.
/// Green is positive along that tangent; this does not claim a UV-frame sign.
pub const SOURCE: NormalConvention = NormalConvention {
    green: NormalGreen::Up,
    z: NormalZ::Rebuild,
    x: NormalX::Red,
    basis: NormalBasis::BitangentTangent,
};
