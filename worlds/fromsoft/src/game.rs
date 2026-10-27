//! What primase takes of a FromSoftware game besides its files: how its renderer stores what a material program
//! writes ([`Shading`]), and the digest the key of its encrypted params must have ([`ParamKey`]). Where the game keeps
//! its files and how they are laid out is [`fromsoft_formats::Game`]'s.
use fromsoft_formats::Game;
use iridia_interface::OutputRole;

/// A game's renderer as its material programs address it.
pub trait Shading {
    /// The game's G-buffer as its deferred lighting decodes it: per render target, the lanes and what they hold. A material's
    /// G-buffer pixel program writes target `n` and the lighting reads it as `g_GBuffer<n>Texture`; every lane of the five
    /// targets has a row, its role or [`OutputRole::Unknown`] with what the lighting programs show of it (the G-buffer
    /// programs write all four lanes of each target they declare). The world normal is `n * 0.5 + 0.50048876`
    /// (`0x3f002008`) in both games' writers, decoded `x * 2 - 1` and normalised.
    ///
    /// Both games' directional light (`GXLightAcc_DDirLight1.ppo`) takes the gloss `g` to the roughness
    /// `1 - 0.95 sqrt(k g)` and squares that for its GGX distribution, with `k = 1 - 0.75 * 0.1 / w` for `w` the light's
    /// direction's w in `cbLight` (a property of the light, not of the surface, and not read back yet): the gloss is
    /// stored as the square of the smoothness over 0.95.
    ///
    /// - Sekiro, from its `GXLightAcc_DDirLight1.ppo` (Shader Model 5): `g_GBuffer0Texture.xyz` the world normal (target
    ///   0); `g_GBuffer1Texture.x` the gloss (target 1); target 2 the albedo (`rgb`); `g_GBuffer3Texture.rgb` the
    ///   reflectance stored as its square root (`F0 = rgb * rgb`, in Schlick's `F0 + (1 - F0)(1 - l.h)^5`; its `w` times
    ///   255 picks the light group); target 4 the emission (`rgb`), in the lanes the layered programs write them. The
    ///   directional and the point light (`GXLightAcc_DPointLight1.ppo`) read `g_GBuffer0Texture.xyz`,
    ///   `g_GBuffer1Texture.xy` and `g_GBuffer3Texture.rgb` (the directional light `.w` too) and neither reads targets 2
    ///   and 4: `g_GBuffer1Texture.y` is a wrap of their diffuse term where above 0 (scaled by 10 and 20 into it), the
    ///   light group `g_GBuffer3Texture.w * 255` is an integer that times 18 is the light's first row in `cbLight`.
    /// - Elden Ring, from `GXLightAcc_DDirLight1.ppo` (the directional light, `gxrenderershader`): `g_GBuffer0Texture.xyz`
    ///   the world normal; `g_GBuffer1Texture.x` the metallic (`F0 = lerp(0.04, albedo, x)`, the albedo
    ///   `g_GBuffer2Texture.rgb`), `.y` the gloss; `g_GBuffer2Texture.rgb` the albedo. `g_GBuffer1Texture.z` (read times
    ///   10) and `g_GBuffer2Texture.w` (read times 255, as an integer) are read for what is not established, and targets
    ///   3 and 4 by no lighting program. Target 4 is the emission (`rgb`), as Sekiro's is, by what the programs write
    ///   there, since no lighting program reads it. The programs that write it write a radiance: a material colour
    ///   (`color_1`) times its intensity, times the material's emissive map (the `_em` texture), times the frame's
    ///   `SC_PreExposure`. This is so in `C[c2120]_Arts_Petal_Gbuf` (Malenia's flower, whose emissive map is the pattern
    ///   it shows in game), `C[AMSN]_Glow[Int]_Gbuf`, `C[c2050]_Ghost_Gbuf` and the map programs `M[Crystal]`,
    ///   `M[AMSN_V]_Emissive_Glow`, `M[AMSN_V]_Vesitation_Emissive_Glow`, `M[WorldTree_Trunk2]` and
    ///   `M[WorldTree_Twig]_Emissive_Glow`. Of the 100 G-buffer programs dumped from c2010, c2050, c2120 and Liurnia, the
    ///   rest write zero there, and none writes another quantity (a mask, a velocity) in its `rgb`. Its `w` is the
    ///   value those programs also write to target 3's `w`; the emissive ones choose by a constant between a computed
    ///   value and a constant's `w` (`FC_GlowScale.w` in the flower's). What it holds is not established.
    fn gbuffer(self) -> Vec<(u32, u8, OutputRole)>;

    /// The container format of the game's compiled shader programs: Shader Model 5 DXBC (Sekiro), Shader Model 6 DXIL
    /// (Elden Ring).
    fn shader_format(self) -> iridia_interface::ShaderFormat;
}

impl Shading for Game {
    fn gbuffer(self) -> Vec<(u32, u8, OutputRole)> {
        use iridia_interface::{NormalEncoding, NormalSpace, ScalarEncoding};
        let normal = OutputRole::Normal {
            encoding: NormalEncoding::XyzBiased {
                bias_bits: 0x3f00_2008,
            },
            space: NormalSpace::World,
        };
        let gloss = OutputRole::Gloss {
            encoding: ScalarEncoding::square_of(0.95),
        };
        let unknown = OutputRole::unknown;
        match self {
            Game::Sekiro => vec![
                (0, 0b0111, normal),
                (
                    0,
                    0b1000,
                    unknown(
                        "Sekiro's G-buffer target 0's w: neither the directional nor the point light reads it",
                    ),
                ),
                (1, 0b0001, gloss),
                (
                    1,
                    0b0010,
                    unknown(
                        "Sekiro's G-buffer target 1's y: the directional and the point light read it where above 0 as a wrap of their diffuse term (scaled by 10 and 20 into n.l and its divisor), which no role states",
                    ),
                ),
                (
                    1,
                    0b1100,
                    unknown(
                        "Sekiro's G-buffer target 1's z and w: neither the directional nor the point light reads them",
                    ),
                ),
                (2, 0b0111, OutputRole::Albedo),
                (
                    2,
                    0b1000,
                    unknown(
                        "Sekiro's G-buffer target 2's w: neither the directional nor the point light reads target 2",
                    ),
                ),
                (
                    3,
                    0b0111,
                    OutputRole::Reflectance {
                        encoding: ScalarEncoding::square_root_of(1.0),
                    },
                ),
                (
                    3,
                    0b1000,
                    unknown(
                        "Sekiro's G-buffer target 3's w: the light group, which the directional light reads as the integer w * 255 that times 18 is the light's first row in cbLight",
                    ),
                ),
                (
                    4,
                    0b0111,
                    OutputRole::Emission {
                        encoding: ScalarEncoding::LINEAR,
                    },
                ),
                (
                    4,
                    0b1000,
                    unknown(
                        "Sekiro's G-buffer target 4's w: neither the directional nor the point light reads target 4",
                    ),
                ),
            ],
            Game::EldenRing => vec![
                (0, 0b0111, normal),
                (
                    0,
                    0b1000,
                    unknown(
                        "Elden Ring's G-buffer target 0's w: not among the lanes the directional light reads",
                    ),
                ),
                (1, 0b0001, OutputRole::Metallic),
                (1, 0b0010, gloss),
                (
                    1,
                    0b0100,
                    unknown(
                        "Elden Ring's G-buffer target 1's z: the directional light reads it times 10, for what is not established",
                    ),
                ),
                (
                    1,
                    0b1000,
                    unknown(
                        "Elden Ring's G-buffer target 1's w: not among the lanes the directional light reads",
                    ),
                ),
                (2, 0b0111, OutputRole::Albedo),
                (
                    2,
                    0b1000,
                    unknown(
                        "Elden Ring's G-buffer target 2's w: the directional light reads it times 255 as an integer, for what is not established",
                    ),
                ),
                (
                    3,
                    0b1111,
                    unknown("Elden Ring's G-buffer target 3: read by no lighting program"),
                ),
                (
                    4,
                    0b0111,
                    OutputRole::Emission {
                        encoding: ScalarEncoding::LINEAR,
                    },
                ),
                (
                    4,
                    0b1000,
                    unknown(
                        "Elden Ring's G-buffer target 4's w: the value the programs also write to target 3's w (or a constant's w), read by no lighting program, for what is not established",
                    ),
                ),
            ],
        }
    }

    fn shader_format(self) -> iridia_interface::ShaderFormat {
        match self {
            Game::Sekiro => iridia_interface::ShaderFormat::Dxbc,
            Game::EldenRing => iridia_interface::ShaderFormat::Dxil,
        }
    }
}

/// What the key of a game's encrypted params ([`fromsoft_formats::game::ParamSource::Regulation`]) must hash to: the
/// user supplies the key ([`fromsoft_formats::Keys::from_folder`]), primase holds it to this SHA-256.
pub trait ParamKey {
    /// The SHA-256 of the 32-byte params key: none for Sekiro, whose params are not encrypted; for Elden Ring, the
    /// digest of the `regulation.bin` key SoulsFormats' `RegulationDecryptor` names for the game.
    fn param_key_sha256(self) -> Option<&'static [u8; 32]>;
}

/// Elden Ring's `regulation.bin` key's SHA-256.
const ELDEN_RING_REGULATION_KEY_SHA256: [u8; 32] = [
    0xcb, 0x34, 0x9d, 0x15, 0xa6, 0x49, 0x29, 0x69, 0xe3, 0xc8, 0x01, 0xdb, 0xf4, 0xd6, 0x0c, 0x72, 0x1e,
    0x9c, 0x58, 0xab, 0xd4, 0xb4, 0x0c, 0xfc, 0x5b, 0x87, 0xaf, 0xf2, 0x53, 0xd6, 0x8c, 0x72,
];

impl ParamKey for Game {
    fn param_key_sha256(self) -> Option<&'static [u8; 32]> {
        match self {
            Game::Sekiro => None,
            Game::EldenRing => Some(&ELDEN_RING_REGULATION_KEY_SHA256),
        }
    }
}
