//! `derive-fromsoft-detail`: each detail-blend material's equation, read from its own program with its own words and
//! proven, as its `pbr.detail` beside the baked `pbr.material`, which stays the fallback. Both games' families: Sekiro's
//! `Character_AMSN_[DetailBlend]` (DXBC programs holding an F0) and Elden Ring's `C[DetailBlend]` and `C[DetailBlend][Rich]`
//! (DXIL programs holding a metallic factor); the other Elden Ring families are refused by name ([`family`]).
use crate::game::Shading;
use std::collections::BTreeMap;

use iridia_interface::{AddressMode, Stage};
use primase_core::detail::DetailedMaterial;
use primase_core::detail::SurfaceLowering;
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{TextureRef, UvTransform, Wrap};
use primase_core::{Component, Entity, Mesh, Semantic, Stream, World};
use primase_core::{shader::Shader, shader::ShaderBinding, shader::TextureSource};

use crate::detail_derive::{derive_equation, slot_transform};
use crate::detail_probe::ProgramProbe;
use crate::materials::{Definition, LayeredMaterial, Material};
use crate::sekiro::components::lowered::UvTiling;
use fromsoft_formats::Game;
use fromsoft_formats::shader::Profile;

/// `derive-fromsoft-detail`: for each material whose shader is Sekiro's `Character_AMSN_[DetailBlend]` or Elden Ring's
/// `C[DetailBlend]` (plain, `[Rich]`, and their `_cloth` variants, whose pixel programs are the same) and which a
/// `core.shader_binding` binds (run it after `resolve-fromsoft-shaders`), its pixel program probed with the material's
/// own words ([`ProgramProbe::with_material`]) and its equation derived and proven ([`derive_equation`]: the random
/// and transition sets, every role within the bound). Where it proves, the material gets a `pbr.detail`
/// ([`DetailedMaterial`]): the equation, each texture slot the binding's texture at the tiling the program samples it
/// with ([`slot_transform`], relative to a tiling the geometry pass applied), the sampler's filters, and what it holds constant. A material whose program does not
/// prove, or whose slots are not all bound to a texture, gets none, counted by reason. Nothing else changes: the baked
/// `pbr.material` is the fallback a consumer without the detail draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeriveFromSoftDetail;

pub const DERIVE_FROMSOFT_DETAIL: PassSpec = PassSpec {
    name: "derive-fromsoft-detail",
    options: &[],
    make: |_| Ok(Box::new(DeriveFromSoftDetail)),
};

/// The Sekiro shader family whose programs the detail equation is derived from.
const FAMILY: &str = "Character_AMSN_[DetailBlend]";

/// The Elden Ring shader family: `C[DetailBlend]` and its variants (category `C`, first brackets `DetailBlend`).
const ELDEN_FAMILY: &str = "C[DetailBlend]";

/// Why a detail-blend variant none of the others names is refused.
const NO_TERM: &str = "its family is none the model has a term for";

/// The count of a metallic-factor material: its PBR model states no dielectric reflectance, so the viewer's stands (glTF's
/// 0.04). The source's lighting reads `F0 = lerp(0.04, albedo, metallic)` ([`fromsoft_formats::Game::gbuffer`]), which agrees, but no
/// measurement has settled it.
pub const GLTF_DIELECTRIC: &str = "detail-blend materials whose dielectric reflectance is the viewer's (glTF's 0.04): the source's lighting reads F0 = lerp(0.04, albedo, metallic), which no measurement has settled";

/// Which game's family a shader is of, and whether the model has a term for it: `Ok(game)` for a family derived,
/// `Err(reason)` for a detail-blend family refused by name (the same text for every material of it); `None` for any other
/// shader. Elden Ring's `[S2]` family has an overlay layer drawn by its own alpha and a back face, and its `_SSS` family a
/// subsurface lane, which the model has no term for; the others (`_FurBlur`, `Glow`, `VA_Morph`) are none of the
/// programs read.
pub fn family(shader: &str) -> Option<Result<Game, &'static str>> {
    if shader == FAMILY {
        return Some(Ok(Game::Sekiro));
    }
    let Some(profile) = Profile::parse(shader) else {
        // outside the shader grammar (a bracket left open, a character after the last bracket): a name that begins as the
        // family does is one of its variants none of which is derived
        return shader.starts_with(ELDEN_FAMILY).then_some(Err(NO_TERM));
    };
    if profile.category != "C" || profile.base_type != "DetailBlend" {
        return None;
    }
    let tags: Vec<&str> = profile.tags.iter().map(String::as_str).collect();
    let suffixes: Vec<&str> = profile.suffixes.iter().map(String::as_str).collect();
    Some(match (tags.as_slice(), suffixes.as_slice()) {
        ([] | ["Rich"], [] | ["cloth"]) => Ok(Game::EldenRing),
        (["S2", ..], _) => Err(
            "its [S2] family has an overlay layer drawn by its own alpha and a back face, which the model has no term for",
        ),
        ([], [suffix, ..]) if suffix.starts_with("SSS") => {
            Err("its _SSS family writes a subsurface lane, which the model has no term for")
        }
        _ => Err(NO_TERM),
    })
}

/// The count of a slot whose texture the source states no addressing for and whose coordinates (the material's mesh
/// coordinates through the slot's tiling) stay within 0 to 1: it repeats, the addressing deciding only the filtering at
/// the edges.
pub const UNSTATED_WRAP_UNIT: &str =
    "wrap not stated; the slot's coordinates stay within 0 to 1, so it decides only edge filtering";

/// The count of a slot whose texture the source states no addressing for and whose coordinates reach past 0 to 1: it
/// repeats, with the share of the coordinates past 0 to 1, where clamping would hold the texture constant.
fn unstated_wrap_beyond(percent: u32) -> String {
    format!(
        "wrap not stated; Clamp would hold the texture constant beyond 0 to 1 over {percent}% of the coordinates"
    )
}

/// The coordinates of set `set` of every primitive drawing material `e`, or why there are none to measure.
fn material_coordinates(world: &World, e: Entity, set: u8) -> Result<Vec<[f32; 2]>, String> {
    let mut out = Vec::new();
    let mut drawn = false;
    for (_, mesh) in world.iter::<Mesh>() {
        for p in mesh.primitives().iter().filter(|p| p.material == Some(e)) {
            drawn = true;
            match p.streams.get(&Semantic::TexCoord(set)) {
                Some(Stream::F32x2(v)) => out.extend(v.iter().copied()),
                Some(Stream::F32x3(v)) => out.extend(v.iter().map(|x| [x[0], x[1]])),
                Some(Stream::F32x4(v)) => out.extend(v.iter().map(|x| [x[0], x[1]])),
                Some(_) => return Err(format!("coordinate set {set} is not floats")),
                None => return Err(format!("a primitive drawing it has no coordinate set {set}")),
            }
        }
    }
    if !drawn {
        return Err("no primitive draws it: its coordinates are not measured".into());
    }
    Ok(out)
}

/// The proof's seed for a material: its MTD's name, hashed (FNV-1a), so a material's proof is the same each run.
pub fn proof_seed(mtd: &str) -> u32 {
    mtd.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    }) | 1
}

impl Pass for DeriveFromSoftDetail {
    fn name(&self) -> &'static str {
        "derive-fromsoft-detail"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME, ShaderBinding::NAME, Shader::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[DetailedMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a detail texture whose addressing the source does not state repeats"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let candidates: Vec<(Entity, Material, Result<Game, &'static str>)> = world
            .iter::<Material>()
            .filter_map(|(e, m)| family(&m.shader).map(|f| (e, m.clone(), f)))
            .collect();
        let mut refused: BTreeMap<String, u64> = BTreeMap::new();
        let mut given = 0;
        for (e, material, game) in candidates {
            let game = match game {
                Ok(game) => game,
                Err(reason) => {
                    *refused.entry(reason.to_owned()).or_default() += 1;
                    continue;
                }
            };
            match detail(world, e, &material, game, log) {
                Ok(d) => {
                    world.insert(e, d);
                    given += 1;
                }
                Err(reason) => *refused.entry(reason).or_default() += 1,
            }
        }
        log.count(
            "detail-blend materials given pbr.detail (their program's equation proven on their own words)",
            given,
        );
        for (reason, n) in refused {
            log.count(
                &format!("detail-blend materials given no pbr.detail: {reason}"),
                n,
            );
        }
        Ok(())
    }
}

/// The `pbr.detail` of material `e`, or why it gets none: a complete reason, the same text for every material refused
/// alike (no material's own values in it).
fn detail(
    world: &World,
    e: Entity,
    material: &Material,
    game: Game,
    log: &mut PassLog,
) -> Result<DetailedMaterial, String> {
    let binding = world.get::<ShaderBinding>(e).ok_or("no shader binding")?;
    let stage = binding.stage(Stage::Pixel).ok_or("no pixel stage")?;
    let shader = world.get::<Shader>(stage.shader).ok_or("no pixel program")?;
    if shader.format != game.shader_format() {
        return Err(format!(
            "a {:?} program: the probe runs {:?} for this game",
            shader.format,
            game.shader_format()
        ));
    }
    let mut probe = match game {
        Game::Sekiro => {
            let layered = match (&material.layered, &material.definition) {
                (Some(l), _) => l.clone(),
                (None, Some(Definition::Mtd(d))) => LayeredMaterial {
                    mtd: material.mtd.clone(),
                    definition: d.clone(),
                    bindings: material.bindings.clone(),
                },
                _ => return Err("no MTD definition".into()),
            };
            ProgramProbe::with_material(&shader.bytecode, &layered)
        }
        Game::EldenRing => {
            let view = material.view().ok_or("no material definition")?;
            ProgramProbe::with_view(game, &shader.bytecode, &view)
        }
    }
    .map_err(|e| format!("its program cannot be probed: {e}"))?;
    let equation = derive_equation(&mut probe, proof_seed(&material.mtd)).map_err(|e| e.reason)?;
    let texcoord = match probe.coordinate_sets().as_slice() {
        [set] => u8::try_from(*set).map_err(|_| "a coordinate set past 255")?,
        sets => {
            return Err(format!(
                "coordinate sets {sets:?}: the equation's textures take one"
            ));
        }
    };
    // the program tiles the authored coordinates: where the geometry pass multiplied set 0 by the material's tiling
    // (`fromsoft.uv_tiling`), each slot's transform is taken relative to it, as the material lowering's are
    let applied = match world.get::<UvTiling>(e) {
        Some(t) if texcoord == 0 => t.0,
        _ => [1.; 2],
    };
    if !applied.iter().all(|v| v.is_finite() && *v > 0.) {
        return Err("the tiling the geometry pass applied is not positive and finite".into());
    }
    let coordinates = material_coordinates(world, e, texcoord)?;
    // each slot the equation reads (the derivation gives a slot a role only where its texels move an output), named
    // by its role, its texture as the binding binds it
    let roles = &equation.roles;
    let mut slots: BTreeMap<u32, String> = BTreeMap::new();
    for (slot, role) in [
        (roles.base_colour, "base colour".to_owned()),
        (roles.base_normal, "base normal".to_owned()),
        (roles.base_mask, "base mask".to_owned()),
    ]
    .into_iter()
    .chain(roles.details.iter().enumerate().flat_map(|(j, d)| {
        [
            (d.colour, format!("detail {j} colour")),
            (d.normal, format!("detail {j} normal")),
        ]
    })) {
        slots.entry(slot).or_insert(role);
    }
    let mut refs: BTreeMap<u32, TextureRef> = BTreeMap::new();
    for (slot, role) in slots {
        let bound = stage
            .textures
            .iter()
            .find(|t| {
                shader
                    .interface
                    .resource(t.resource)
                    .and_then(|r| r.provenance.d3d)
                    .is_some_and(|d| d.register == slot)
            })
            .ok_or_else(|| format!("slot t{slot}, the {role} the equation reads, is not bound"))?;
        let (texture, sampler) = match &bound.source {
            TextureSource::Texture { texture, sampler } => (texture, sampler),
            TextureSource::Default { why, .. } => {
                return Err(format!(
                    "slot t{slot}, the {role} the equation reads, has no texture: the binding reads a constant ({why})"
                ));
            }
            TextureSource::Frame(frame) => {
                return Err(format!(
                    "slot t{slot}, the {role} the equation reads, is the frame's {frame:?}, not a material texture"
                ));
            }
            TextureSource::Missing => {
                return Err(format!(
                    "slot t{slot}, the {role} the equation reads, is missing and the source states no default"
                ));
            }
            TextureSource::Target { view } => {
                return Err(format!(
                    "slot t{slot}, the {role} the equation reads, is render graph target {}, not a material texture",
                    view.target
                ));
            }
        };
        let measured = slot_transform(&probe, slot)?;
        let transform = UvTransform {
            scale: std::array::from_fn(|k| measured.scale[k] / applied[k]),
            ..measured
        };
        if sampler.wrap[..2].iter().any(Option::is_none) {
            // the coordinates the program samples the slot at: the mesh's through its tiling
            let beyond = coordinates
                .iter()
                .filter(|uv| {
                    (0..2).any(|k| {
                        let x = uv[k] * transform.scale[k] + transform.offset[k];
                        !(0. ..=1.).contains(&x)
                    })
                })
                .count();
            if beyond == 0 {
                log.approximate_on(e, UNSTATED_WRAP_UNIT, 1);
            } else {
                let percent = (100 * beyond).div_ceil(coordinates.len().max(1)) as u32;
                log.approximate_on(e, &unstated_wrap_beyond(percent), 1);
            }
        }
        let mut wrap = [Wrap::Repeat; 2];
        for (axis, address) in sampler.wrap[..2].iter().enumerate() {
            wrap[axis] = match address {
                None => Wrap::Repeat,
                Some(AddressMode::Repeat) => Wrap::Repeat,
                Some(AddressMode::Mirror) => Wrap::Mirror,
                Some(AddressMode::Clamp) => Wrap::Clamp,
                Some(other) => {
                    return Err(format!(
                        "slot t{slot}, the {role} the equation reads, is addressed {other:?}, which no detail wrap is"
                    ));
                }
            };
        }
        refs.insert(
            slot,
            TextureRef {
                texture: *texture,
                texcoord,
                wrap,
                transform: Some(transform),
                filters: sampler.filters,
            },
        );
    }
    if equation.constants.lowering == SurfaceLowering::MetallicGloss {
        log.approximate_on(e, GLTF_DIELECTRIC, 1);
    }
    Ok(equation.detailed_material(|slot| refs[&slot]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sekiro's family and Elden Ring's plain and `[Rich]` ones (and their cloth variants) are derived; its `[S2]`, `_SSS` and
    /// other detail-blend families are refused by name; any other shader is none of them.
    #[test]
    fn a_shader_is_of_a_derived_family_or_refused_by_name() {
        assert_eq!(family("Character_AMSN_[DetailBlend]"), Some(Ok(Game::Sekiro)));
        for shader in [
            "C[DetailBlend]",
            "C[DetailBlend][Rich]",
            "C[DetailBlend]_cloth",
            "C[DetailBlend][Rich]_cloth",
        ] {
            assert_eq!(family(shader), Some(Ok(Game::EldenRing)), "{shader}");
        }
        for (shader, expected) in [
            ("C[DetailBlend][S2]", "[S2] family"),
            ("C[DetailBlend][S2]_cloth", "[S2] family"),
            ("C[DetailBlend]_SSS", "_SSS family"),
            ("C[DetailBlend]_SSS_cloth", "_SSS family"),
            ("C[DetailBlend]_FurBlur", "has a term for"),
            ("C[DetailBlend][1_0_0_0][S2]_Glow[Int]", "has a term for"),
        ] {
            let Some(Err(reason)) = family(shader) else {
                panic!("{shader} is not refused by name");
            };
            assert!(reason.contains(expected), "{shader}: {reason}");
        }
        assert_eq!(family("C[AMSN]"), None);
        assert_eq!(family("Character_AMSN"), None);
        // another category with the same brackets is none of them, and a name outside the shader grammar that begins as the
        // family does is refused as any other variant is
        assert_eq!(family("CS[DetailBlend]"), None);
        assert_eq!(family("M[DetailBlend]"), None);
        for shader in ["C[DetailBlend]rope", "C[DetailBlend][Rich", "C[DetailBlend]_"] {
            let Some(Err(reason)) = family(shader) else {
                panic!("{shader} is not refused by name");
            };
            assert!(reason.contains("has a term for"), "{shader}: {reason}");
        }
        // the variants are told apart by their tags and suffixes as the grammar splits them
        for (shader, expected) in [
            ("C[DetailBlend][Rich]_SSS", "has a term for"),
            ("C[DetailBlend][S2]_SSS", "[S2] family"),
            ("C[DetailBlend]_SSS_Glow", "_SSS family"),
            ("C[DetailBlend]_cloth_Glow", "has a term for"),
            ("C[DetailBlend][Rich][S2]", "has a term for"),
        ] {
            let Some(Err(reason)) = family(shader) else {
                panic!("{shader} is not refused by name");
            };
            assert!(reason.contains(expected), "{shader}: {reason}");
        }
    }
}
