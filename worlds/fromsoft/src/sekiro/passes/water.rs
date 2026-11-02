//! `derive-fromsoft-water`: each material whose game program draws water, measured from that program
//! ([`crate::water_derive::measure`]) as its `core.water_body`, and its class water.
use std::collections::BTreeMap;

use iridia_interface::{AddressMode, Stage};
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{TextureRef, UvTransform, Wrap};
use primase_core::shader::{Shader, ShaderBinding, TextureSource};
use primase_core::{Component, Entity, WaterBody, WaveLayer, World};

use crate::materials::Material;
use crate::sekiro::components::lowered::UvTiling;
use crate::water_derive::{Measured, NO_EXTINCTION, measure};
use crate::water_probe::{WaterProbe, reads_behind};

/// `derive-fromsoft-water`: for each material a `core.shader_binding` binds (run it after `resolve-fromsoft-shaders`
/// and before `lower-fromsoft-materials`) whose pixel program reads the frame's scene colour and its depth, the program
/// is run on the probe's scenes and what it draws measured. Where what is behind its surface fades exponentially with
/// the depth of water in front of it and the rest of the measurement holds, the material is water: its class becomes
/// `water` (the lowering's `core.material_usage`) and it gets a `core.water_body` of the measured extinction, scatter,
/// ior, shore fade and waves. What the program does that the body does not carry is counted by reason; so is a program
/// not measured, and a material whose definition states water interaction (`g_GXFT_WaterInteraction`) and whose program
/// does not fade what is behind it with depth (it keeps the class its definition states, without a body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeriveFromSoftWater;

pub const DERIVE_FROMSOFT_WATER: PassSpec = PassSpec {
    name: "derive-fromsoft-water",
    options: &[],
    make: |_| Ok(Box::new(DeriveFromSoftWater)),
};

/// Why a material whose definition states water interaction gets no body where its program does not read the scene
/// behind its surface: Sekiro's tessellated water of `g_BlendMode` 0 is drawn into the G-buffer, its program taking the
/// depth of water under it from the frame's depth only to fade itself in over the shore, so what is behind it never
/// shows through it.
pub const NOT_BEHIND: &str = "its definition states water interaction and its program does not read the scene behind its \
    surface (drawn into the G-buffer): it keeps its stated class, without a body";

/// What every water body leaves out of its program, counted once per body: the inputs the frame binds as none (the
/// water simulation's ripples, the screen-space reflection), which the probe runs as bound, and the program's own
/// refraction, which offsets the screen coordinate it reads what is behind at by the normal rather than bending by an
/// index of refraction.
pub const SCREEN_REFRACTION: &str = "refraction drawn as a screen offset by the normal in the program; the body states an ior a consumer bends by";

/// What a lowering counts where a wave's texture coordinate is taken as the mesh's first set: the probe gives every
/// coordinate input the same coordinate, so the vertex program's map from the mesh's sets to it is not measured.
pub const WAVE_COORDINATES: &str = "wave tiling measured from the program's coordinate input, taken as the mesh's first set through the material's tiling";

impl Pass for DeriveFromSoftWater {
    fn name(&self) -> &'static str {
        "derive-fromsoft-water"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME, ShaderBinding::NAME, Shader::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[WaterBody::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a water program's refraction, ripples and reflections beyond what core.water_body states"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let candidates: Vec<Entity> = world.iter::<Material>().map(|(e, _)| e).collect();
        let mut refused: BTreeMap<String, u64> = BTreeMap::new();
        let mut given = 0;
        for e in candidates {
            let Some(stage) = world.get::<ShaderBinding>(e).and_then(|b| b.stage(Stage::Pixel)) else {
                continue;
            };
            let stated_water = world.get::<Material>(e).is_some_and(|m| m.class == "water");
            if !reads_behind(stage) {
                if stated_water {
                    *refused.entry(NOT_BEHIND.to_owned()).or_default() += 1;
                }
                continue;
            }
            let Some(probe) = probe_of(world, e) else { continue };
            let probe = match probe {
                Ok(p) => p,
                Err(why) => {
                    *refused
                        .entry(format!("its program cannot be probed: {why}"))
                        .or_default() += 1;
                    continue;
                }
            };
            match measure(&probe) {
                Ok(m) => {
                    let body = body(world, e, &m, log)?;
                    for reason in &m.not_carried {
                        log.approximate_on(e, reason, 1);
                    }
                    // what the binding states (once) and what the runs stated, each reason counted at its most
                    let mut stated = probe.policies.clone();
                    for (reason, n) in &m.stated {
                        let most = stated.entry(reason.clone()).or_default();
                        *most = (*most).max(*n);
                    }
                    for (reason, n) in &stated {
                        log.approximate_on(e, &format!("water program run with {reason}"), *n);
                    }
                    log.approximate_on(e, SCREEN_REFRACTION, 1);
                    if let Some(material) = world.get_mut::<Material>(e) {
                        material.class = "water".into();
                    }
                    world.insert(e, body);
                    given += 1;
                }
                Err(n) if n.reason == NO_EXTINCTION && stated_water => {
                    *refused
                        .entry(format!(
                            "its definition states water interaction and {NO_EXTINCTION}: it keeps its stated class, without a body"
                        ))
                        .or_default() += 1;
                }
                Err(n) => *refused.entry(n.reason).or_default() += 1,
            }
        }
        log.count(
            "materials whose program draws water, given core.water_body (measured from their program)",
            given,
        );
        for (reason, n) in refused {
            log.count(
                &format!("materials whose program reads the scene behind it, given no water body: {reason}"),
                n,
            );
        }
        Ok(())
    }
}

/// The probe of material `e`'s pixel program as its binding binds it, its inputs written by the binding's vertex program;
/// `None` where it has no bound pixel program.
fn probe_of(world: &World, e: Entity) -> Option<Result<WaterProbe, String>> {
    let binding = world.get::<ShaderBinding>(e)?;
    let stage = binding.stage(Stage::Pixel)?;
    let shader = world.get::<Shader>(stage.shader)?;
    let vertex = binding
        .stage(Stage::Vertex)
        .and_then(|v| world.get::<Shader>(v.shader).map(|s| (s, v.shader)));
    Some(match vertex {
        Some((vertex, entity)) => WaterProbe::new(shader, stage, vertex, entity),
        None => Err("no bound vertex program to give the pixel program its inputs".to_owned()),
    })
}

/// The water body of material `e` from what its program measured: each wave its binding's texture at the tiling the
/// program samples it with, relative to a tiling the geometry pass applied to the first coordinate set.
fn body(world: &World, e: Entity, m: &Measured, log: &mut PassLog) -> Result<WaterBody, String> {
    let stage = world
        .get::<ShaderBinding>(e)
        .and_then(|b| b.stage(Stage::Pixel))
        .ok_or("no pixel stage")?;
    let shader = world.get::<Shader>(stage.shader).ok_or("no pixel program")?;
    let applied = match world.get::<UvTiling>(e) {
        Some(t) => t.0,
        None => [1.; 2],
    };
    let mut waves = Vec::new();
    for wave in &m.waves {
        let bound = stage
            .textures
            .iter()
            .find(|t| {
                shader
                    .interface
                    .resource(t.resource)
                    .and_then(|r| r.provenance.d3d)
                    .is_some_and(|d| d.register == wave.register)
            })
            .ok_or_else(|| format!("wave texture t{} is not bound", wave.register))?;
        let TextureSource::Texture { texture, sampler } = &bound.source else {
            return Err(format!(
                "wave texture t{} is not a material texture",
                wave.register
            ));
        };
        let wrap = std::array::from_fn(|axis| match sampler.wrap[axis] {
            Some(AddressMode::Mirror) => Wrap::Mirror,
            Some(AddressMode::Clamp) => Wrap::Clamp,
            _ => Wrap::Repeat,
        });
        log.approximate_on(e, WAVE_COORDINATES, 1);
        waves.push(WaveLayer {
            normal: TextureRef {
                texture: *texture,
                texcoord: 0,
                wrap,
                transform: Some(UvTransform {
                    scale: std::array::from_fn(|k| wave.transform.scale[k] / applied[k]),
                    ..wave.transform
                }),
                filters: sampler.filters,
            },
            velocity: wave.velocity,
            strength: wave.strength,
        });
    }
    Ok(WaterBody {
        extinction: m.extinction,
        scatter: m.scatter,
        ior: WaterBody::ior_of_f0(m.f0.max(1e-6)),
        waves,
        shore_fade: m.shore_fade,
    })
}
