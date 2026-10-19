//! Static normal-map contribution shared by the inspected G-buffer families.
//! The shaders read R/G as `2*v-1`, reconstruct positive Z and form
//! `R * normalize(cross(N,T)*w) + G * normalize(T) + Z * normalize(N)`.
//! Therefore glTF with the stored tangent uses swapped R/G, green-up, and
//! reflected tangent handedness. This is not a texture-authorship curl rule.
use crate::{Result, invalid};
use primase_core::{NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexRole, Texture, World};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    /// Uses the supplied TANGENT stream.
    Gltf,
    /// Regenerates a tangent frame from UVs; green is inverted relative to it.
    Skate,
}

/// Lower only after selecting the consumer. Source texture bytes remain intact
/// until this call. The shared image pass owns channel conversion and Z rebuild.
/// The reader already reflected tangent xyz and w in `tangent_to_scene`; flipping
/// w again here would undo that LH-to-RH frame conversion.
///
/// Consumer test: green is inverted relative to the UV-derived frame. Skate
/// keeps R/G unswapped, flips green and rebuilds Z for every source normal map.
/// The measured frame census has sheared/other regions, so replacing the stored
/// frame with the consumer's UV frame is a counted approximation, not a claim
/// that those frames agree everywhere. No texture-specific curl exceptions.
pub fn lower(world: &mut World, consumer: Consumer) -> Result<primase_core::pass::PassLog> {
    let mut prepared = World::default();
    let mut entities = Vec::new();
    for (e, texture) in world.iter::<Texture>().filter(|(_, t)| t.role == TexRole::Normal) {
        if texture.normal == Some(NormalConvention::GLTF) {
            continue;
        }
        if texture.normal != Some(SOURCE) {
            return Err(invalid(
                "Sekiro normal lowering",
                0,
                "unexpected source normal convention",
            ));
        }
        let mut texture = texture.clone();
        if consumer == Consumer::Skate {
            texture.normal = Some(NormalConvention {
                green: NormalGreen::Down,
                basis: NormalBasis::TangentBitangent,
                ..SOURCE
            });
        }
        let temporary = prepared.spawn();
        prepared.insert(temporary, texture);
        entities.push((e, temporary));
    }
    let mut log = primase_image::normalize_normals(&mut prepared)
        .map_err(|e| invalid("Sekiro normal lowering", 0, e))?;
    if consumer == Consumer::Skate {
        log.approximate(
            "stored frame replaced by consumer UV-derived frame",
            entities.len() as u64,
        );
    }
    for (e, temporary) in entities {
        world.insert(
            e,
            prepared
                .get::<Texture>(temporary)
                .expect("prepared texture")
                .clone(),
        );
        world.remove::<primase_core::TextureFile>(e);
        if let Some(scene) = world.get_mut::<primase_reskate::dialect::SkateTexture>(e) {
            scene.green = Some(NormalGreen::Up);
            scene.z = Some(NormalZ::Stored);
        }
    }
    Ok(log)
}
