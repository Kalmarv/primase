//! The FromSoftware games this crate reads, and what differs between them that their files do not say about
//! themselves. Formats are read by what their bytes state (a map layout's section chain, a FLVER's version); a game's
//! own rules (its archive set, where it keeps its files) are data in that game's module (`sekiro`, `elden`), read
//! through [`Game`] by the shared readers.
use crate::bhd::{PathHash, RecordLayout};
pub use crate::sekiro::source::Resolution;
use iridia_interface::OutputRole;

/// A FromSoftware game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum Game {
    Sekiro,
    EldenRing,
}

impl Game {
    pub const ALL: [Game; 2] = [Game::Sekiro, Game::EldenRing];

    pub fn name(self) -> &'static str {
        match self {
            Game::Sekiro => "Sekiro",
            Game::EldenRing => "Elden Ring",
        }
    }

    /// The game's archive set.
    pub fn archives(self) -> &'static ArchiveSet {
        match self {
            Game::Sekiro => &crate::sekiro::ARCHIVES,
            Game::EldenRing => &crate::elden::ARCHIVES,
        }
    }

    /// The archive folder holding a map's own files (map pieces, collision, light banks): Sekiro keeps them in
    /// `/map/<map>`, Elden Ring in `/map/<mAA>/<map>`. A map name that is not ASCII is refused by name.
    pub fn map_folder(self, map: &str) -> crate::Result<String> {
        if !map.is_ascii() {
            return Err(crate::invalid("map name", 0, format!("{map:?} is not ASCII")));
        }
        Ok(match self {
            Game::Sekiro => format!("/map/{map}"),
            Game::EldenRing => format!("/map/{}/{map}", &map[..map.len().min(3)]),
        })
    }

    /// The source-identity prefix of a placed object model (`obj/<model>` in Sekiro, `asset/<model>` in Elden Ring).
    pub fn object_prefix(self) -> &'static str {
        match self {
            Game::Sekiro => "obj",
            Game::EldenRing => "asset",
        }
    }

    /// A placed object's model: the binder holding it and its FLVER member, and where its collision files are.
    pub fn object_model(self, model: &str) -> crate::Result<ObjectModel> {
        match self {
            Game::Sekiro => Ok(ObjectModel {
                binder: format!("/obj/{model}.objbnd.dcx"),
                flver: format!("{model}.flver"),
                collision: vec![CollisionSource {
                    binder: None,
                    member: format!("{model}.hkx"),
                    resolution: Resolution::High,
                }],
            }),
            Game::EldenRing => crate::elden::asset_model(model),
        }
    }

    /// The larger maps that also place `map`'s large assets (under names `<map>-<part>`, in their own frame), and
    /// where `map`'s centre lies in each one's frame (source x, z): Elden Ring's open-world tiles; none elsewhere.
    pub fn parent_maps(self, map: &str) -> Vec<(String, [f64; 2])> {
        match self {
            Game::Sekiro => Vec::new(),
            Game::EldenRing => crate::elden::parent_tiles(map),
        }
    }

    /// The binders holding the game's material binaries (Elden Ring's); none for a game whose materials are MTDs.
    pub fn material_binders(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &[],
            Game::EldenRing => crate::elden::MATERIAL_BINDERS,
        }
    }

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
    pub fn gbuffer(self) -> Vec<(u32, u8, OutputRole)> {
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

    /// The binders holding the game's shader bundles (`<shader>.shaderbdle` members), in the order a shader is looked up:
    /// Sekiro's one; Elden Ring's base game's, its two later additions' and SpeedTree's (a shader is in one of them).
    pub fn shader_binders(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &["/shader/shaderbdle.shaderbdlebnd.dcx"],
            Game::EldenRing => crate::elden::SHADER_BINDERS,
        }
    }

    /// The binder holding the programs of the fixed-function material shaders, which have no bundle of their own: Sekiro's
    /// `GXFlver_*` family (`/shader/gxflvershader.shaderbnd.dcx`: DXBC programs by file name, one per permutation of
    /// the shader's variant, effect (`Bloody`, `Burn`, `Cm`, `Df`, `Cloth`, `Phantom`, `ZFeather`, `Decal[A]`) and pass).
    pub fn fixed_function_binder(self) -> Option<&'static str> {
        match self {
            Game::Sekiro => Some("/shader/gxflvershader.shaderbnd.dcx"),
            Game::EldenRing => None,
        }
    }

    /// The container format of the game's compiled shader programs: Shader Model 5 DXBC (Sekiro), Shader Model 6 DXIL
    /// (Elden Ring).
    pub fn shader_format(self) -> iridia_interface::ShaderFormat {
        match self {
            Game::Sekiro => iridia_interface::ShaderFormat::Dxbc,
            Game::EldenRing => iridia_interface::ShaderFormat::Dxil,
        }
    }

    /// The binders holding the textures of the character `owner` (`c2010`), in the order a texture name is looked up (the
    /// first holding it gives it): Sekiro keeps them in `chr/<id>.texbnd`, Elden Ring in a high resolution
    /// `chr/<id>_h.texbnd` and a low resolution `chr/<id>_l.texbnd` (4.6 MB and 0.3 MB for `c2010`), the low one only
    /// for what the high one lacks. A character may have none (the binder is not in the archives).
    pub fn character_texture_binders(self, owner: &str) -> Vec<String> {
        match self {
            Game::Sekiro => vec![format!("/chr/{owner}.texbnd.dcx")],
            Game::EldenRing => vec![
                format!("/chr/{owner}_h.texbnd.dcx"),
                format!("/chr/{owner}_l.texbnd.dcx"),
            ],
        }
    }

    /// The texture packages every character of the game binds besides its own, read whole: Sekiro's textures every
    /// character shares (hair, fabric, metal, seams) and its system textures; Elden Ring's textures every character
    /// shares (`aat100_*`, `aat500_*`: skin, scale, leather, fabric, metal, hair, belts), its system textures and its
    /// decal textures (the last two the packages its maps bind too).
    pub fn character_shared_textures(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &["/parts/common_body.tpf.dcx", "/other/systex.tpf.dcx"],
            Game::EldenRing => crate::elden::CHARACTER_TEXTURES,
        }
    }

    /// Texture packages every map of the game binds, read whole (Elden Ring's system and decal textures; Sekiro's are
    /// part of its area texture discovery).
    pub fn common_textures(self) -> &'static [&'static str] {
        match self {
            Game::Sekiro => &[],
            Game::EldenRing => crate::elden::COMMON_TEXTURES,
        }
    }

    /// The package holding the texture `stem`, where the game names packages after their textures (Elden Ring's
    /// assets); `None` otherwise.
    pub fn texture_package(self, stem: &str) -> Option<String> {
        match self {
            Game::Sekiro => None,
            Game::EldenRing => crate::elden::texture_package(stem),
        }
    }

    /// Where the game keeps its params ([`crate::param::Param`]): Sekiro in a binder of its archives, Elden Ring in
    /// `regulation.bin` beside its executable, encrypted.
    pub fn params(self) -> ParamSource {
        match self {
            Game::Sekiro => crate::sekiro::PARAMS,
            Game::EldenRing => crate::elden::PARAMS,
        }
    }

    /// Where the game's NPC param keeps the fields of [`crate::param::NpcStats`].
    pub fn npc_stats(self) -> crate::param::NpcStatLayout {
        match self {
            Game::Sekiro => crate::sekiro::NPC_STATS,
            Game::EldenRing => crate::elden::NPC_STATS,
        }
    }

    /// Where the game's NPC param keeps the 32 `modelDispMask` bits ([`crate::param::NpcParam`]).
    pub fn npc_param(self) -> crate::param::NpcLayout {
        match self {
            Game::Sekiro => crate::sekiro::NPC_PARAM,
            Game::EldenRing => crate::elden::NPC_PARAM,
        }
    }

    /// Whether a map model belongs to the game's distant-backdrop series (Sekiro's `m9xxxxx`).
    pub fn far_model(self, model: &str) -> bool {
        match self {
            Game::Sekiro => model.starts_with("m9"),
            Game::EldenRing => false,
        }
    }
}

/// Where a game keeps its params ([`Game::params`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamSource {
    /// A binder in the game's archives, by path.
    Archived(&'static str),
    /// A file beside the executable (`root` of an [`crate::Install`]), AES-256-CBC encrypted (its first 16 bytes the
    /// initialisation vector), holding a DCX envelope around the param binder. The key is the user's key material: the
    /// file `key_file` of the keys folder ([`crate::Keys::from_folder`]), whose SHA-256 must be `key_sha256`.
    Regulation {
        file: &'static str,
        key_file: &'static str,
        key_sha256: [u8; 32],
    },
}

/// Where a placed object's model is stored ([`Game::object_model`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectModel {
    pub binder: String,
    pub flver: String,
    /// Its collision files, each where the game keeps it.
    pub collision: Vec<CollisionSource>,
}

/// One collision file of an object: the binder holding it (`None`: the model's own binder), its member's name and
/// its resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionSource {
    pub binder: Option<String>,
    pub member: String,
    pub resolution: Resolution,
}

/// One BHD5/BDT pair of an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchivePair {
    /// The number entries of this pair report as their shard.
    pub shard: u8,
    /// The pair's path below the install folder without its `.bhd`/`.bdt` extension (`Data1`, `sd/sd`).
    pub stem: &'static str,
    /// Whether every install holds it; an optional pair (downloadable content) that is absent is reported, not an
    /// error.
    pub required: bool,
}

impl ArchivePair {
    /// A pair every install holds.
    pub const fn required(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: true,
        }
    }
    /// A pair an install may lack.
    pub const fn optional(shard: u8, stem: &'static str) -> Self {
        Self {
            shard,
            stem,
            required: false,
        }
    }
    /// The last component of the stem (`sd` for `sd/sd`): the name its key file takes.
    pub fn name(&self) -> &'static str {
        self.stem.rsplit('/').next().unwrap_or(self.stem)
    }
}

/// Where a game's archive keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Published by the community unpackers and built in, one (modulus, exponent) per pair in the set's order.
    Published(&'static [(&'static [u8], &'static [u8])]),
    /// Not distributed with primase: the caller supplies them.
    Supplied,
}

/// A game's archives: its BHD5/BDT pairs in index order, how its paths hash, how its file records are laid out and
/// where its keys come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveSet {
    pub pairs: &'static [ArchivePair],
    pub hash: PathHash,
    pub record: RecordLayout,
    pub keys: KeySource,
}
