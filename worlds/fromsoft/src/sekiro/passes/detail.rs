//! `derive-fromsoft-detail`: each detail-blend material's equation, read from its own program with its own words and
//! proven, as its `pbr.detail` beside the baked `pbr.material`, which stays the fallback.
use std::collections::BTreeMap;

use primase_core::detail::DetailedMaterial;
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{TextureRef, UvTransform, Wrap};
use primase_core::shader::{AddressMode, Shader, ShaderBinding, ShaderFormat, Stage, TextureSource};
use primase_core::{Component, Entity, Mesh, Semantic, Stream, World};

use crate::detail_derive::{derive_equation, slot_transform};
use crate::detail_probe::ProgramProbe;
use crate::materials::{Definition, LayeredMaterial, Material};
use crate::sekiro::components::lowered::UvTiling;

/// `derive-fromsoft-detail`: for each material whose shader is Sekiro's `Character_AMSN_[DetailBlend]` and which a
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

/// The shader family whose programs the detail equation is derived from.
const FAMILY: &str = "Character_AMSN_[DetailBlend]";

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
        let candidates: Vec<(Entity, Material)> = world
            .iter::<Material>()
            .filter(|(_, m)| m.shader == FAMILY)
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut refused: BTreeMap<String, u64> = BTreeMap::new();
        let mut given = 0;
        for (e, material) in candidates {
            match detail(world, e, &material, log) {
                Ok(d) => {
                    world.insert(e, d);
                    given += 1;
                }
                Err(why) => {
                    let reason = why.split(':').next().unwrap_or(&why).to_owned();
                    *refused.entry(reason).or_default() += 1;
                }
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

/// The `pbr.detail` of material `e`, or why it gets none.
fn detail(
    world: &World,
    e: Entity,
    material: &Material,
    log: &mut PassLog,
) -> Result<DetailedMaterial, String> {
    let binding = world.get::<ShaderBinding>(e).ok_or("no shader binding")?;
    let stage = binding.stage(Stage::Pixel).ok_or("no pixel stage")?;
    let shader = world.get::<Shader>(stage.shader).ok_or("no pixel program")?;
    if shader.format != ShaderFormat::Dxbc {
        return Err(format!("a {:?} program: the probe runs DXBC", shader.format));
    }
    let layered = match (&material.layered, &material.definition) {
        (Some(l), _) => l.clone(),
        (None, Some(Definition::Mtd(d))) => LayeredMaterial {
            mtd: material.mtd.clone(),
            definition: d.clone(),
            bindings: material.bindings.clone(),
        },
        _ => return Err("no MTD definition".into()),
    };
    let mut probe =
        ProgramProbe::with_material(&shader.bytecode, &layered).map_err(|e| format!("probe: {e}"))?;
    let equation = derive_equation(&mut probe, proof_seed(&material.mtd))?;
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
        return Err(format!(
            "the applied tiling {applied:?} is not positive and finite"
        ));
    }
    let coordinates = material_coordinates(world, e, texcoord)?;
    // each slot the equation reads, its texture as the binding binds it
    let roles = &equation.roles;
    let mut slots = vec![roles.base_colour, roles.base_normal, roles.base_mask];
    slots.extend(roles.details.iter().flat_map(|d| [d.colour, d.normal]));
    slots.sort_unstable();
    slots.dedup();
    let mut refs: BTreeMap<u32, TextureRef> = BTreeMap::new();
    for slot in slots {
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
            .ok_or_else(|| format!("slot t{slot} is not bound"))?;
        let TextureSource::Texture { texture, sampler } = &bound.source else {
            return Err(format!("slot t{slot} is bound to no texture"));
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
                        "slot t{slot}'s addressing {other:?}: no detail wrap is it"
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
    Ok(equation.detailed_material(|slot| refs[&slot]))
}
