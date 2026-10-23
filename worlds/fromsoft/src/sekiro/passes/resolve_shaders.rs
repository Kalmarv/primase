//! `resolve-fromsoft-shaders`: the layered materials' G-buffer programs as `core.shader` entities and each material's
//! `core.shader_binding`.
use crate::layer_bake::resolve::{Unbound, resolve};
use crate::materials::Material;
use crate::sekiro::source::{Payloads, ShaderPrograms, TexturePayloads};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{Shader, ShaderBinding, ShaderFormat};
use primase_core::{ColorSpace, Component, Entity, Name, TexRole, Texture, World};
use std::collections::BTreeMap;

/// `resolve-fromsoft-shaders{unbound-textures=preview|leave}`: for each material with a layered definition, bind its
/// G-buffer pixel program (`<shader>_<switch>_Gbuf.ppo`, from `fromsoft.shader_programs`) as Sekiro's rules say
/// ([`crate::layer_bake::resolve`]): one `core.shader` entity per program (the bytecode as stored, its interface read
/// from it), one `core.texture` entity per bound texture file and colour space (the DDS as stored: block data, every
/// mip), and a `core.shader_binding` on the material: the material's own constants as the words the program reads,
/// the engine's per-view, per-frame and per-object values as roles (never values), the sampler state the source
/// states (none), each texture slot and the G-buffer's outputs by their role. A material whose program the reader
/// does not hold is counted and left unbound.
///
/// The option is required: it says what a texture slot with no texture, or whose file the reader does not hold,
/// becomes. `preview` gives it a stated colour for its role (a mid-grey albedo, a flat normal, zero elsewhere), a
/// counted approximation; `leave` binds it `Missing`, so a consumer that needs the slot refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveFromSoftShaders {
    unbound: Unbound,
}

impl ResolveFromSoftShaders {
    pub fn new(unbound: Unbound) -> Self {
        ResolveFromSoftShaders { unbound }
    }
    pub fn unbound(&self) -> Unbound {
        self.unbound
    }

    /// From its options (`unbound-textures` is required), through [`ResolveFromSoftShaders::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut unbound = None;
        for (k, v) in o {
            unbound = match (k.as_str(), v.as_str()) {
                ("unbound-textures", "preview") => Some(Unbound::Preview),
                ("unbound-textures", "leave") => Some(Unbound::Leave),
                ("unbound-textures", v) => {
                    return Err(format!("unbound-textures={v} is not preview or leave"));
                }
                _ => return Err(format!("unknown option {k}")),
            };
        }
        Ok(Self::new(unbound.ok_or(
            "unbound-textures is required: what a texture slot with no texture reads is the program's choice",
        )?))
    }
}

/// The bound texture's component, a DDS read as stored with the colour space the slot's role gives it.
fn texture(payloads: &Payloads, stem: &str, srgb: bool) -> crate::Result<Option<Texture>> {
    let Some(bytes) = payloads.get(stem) else {
        return Ok(None);
    };
    let mut t = primase_dds::read(bytes).map_err(|e| crate::invalid("layer texture DDS", 0, e))?;
    t.color_space = if srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    t.role = if srgb { TexRole::Color } else { TexRole::Other };
    Ok(Some(t))
}

impl Pass for ResolveFromSoftShaders {
    fn name(&self) -> &'static str {
        "resolve-fromsoft-shaders"
    }
    fn options(&self) -> Options {
        Options::from([(
            "unbound-textures".to_owned(),
            match self.unbound {
                Unbound::Preview => "preview",
                Unbound::Leave => "leave",
            }
            .to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Shader::NAME, ShaderBinding::NAME, Texture::NAME, Name::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a texture slot with no texture gets a preview colour for its role (unbound-textures=preview)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let none = BTreeMap::new();
        world.resource_scope::<ShaderPrograms, _>(|world, programs| {
            world.resource_scope::<TexturePayloads, _>(|world, payloads| {
                run(
                    world,
                    programs.map_or(&none, |p| &p.0),
                    payloads.map_or(&none, |p| &p.0),
                    self.unbound,
                    log,
                )
            })
        })
    }
}

fn run(
    world: &mut World,
    programs: &Payloads,
    payloads: &Payloads,
    unbound: Unbound,
    log: &mut PassLog,
) -> Result<(), String> {
    let layered: Vec<(Entity, crate::materials::LayeredMaterial)> = world
        .iter::<Material>()
        .filter_map(|(e, m)| m.layered.clone().map(|l| (e, l)))
        .collect();
    // the programs read once, each its own entity
    let mut shaders: BTreeMap<String, (Entity, primase_dxbc::Shader)> = BTreeMap::new();
    // the textures spawned, by stem and colour space
    let mut textures: BTreeMap<(String, bool), Entity> = BTreeMap::new();
    let (mut bound, mut without_program) = (0u64, 0u64);
    let mut policies: BTreeMap<String, u64> = BTreeMap::new();
    for (material_entity, material) in layered {
        let name = material.program_name().map_err(|e| e.to_string())?;
        let Some(bytes) = programs.get(&name) else {
            without_program += 1;
            continue;
        };
        if !shaders.contains_key(&name) {
            let parsed = primase_dxbc::Shader::parse(bytes).map_err(|e| format!("{name}: {e}"))?;
            let entity = world.spawn();
            world.insert(entity, Name(name.clone()));
            world.insert(
                entity,
                Shader {
                    format: ShaderFormat::Dxbc,
                    bytecode: bytes.to_vec(),
                    interface: parsed.interface.clone(),
                },
            );
            shaders.insert(name.clone(), (entity, parsed));
        }
        let (shader_entity, parsed) = &shaders[&name];
        let resolution = resolve(parsed, *shader_entity, &material, unbound, &mut |stem, srgb| {
            let key = (stem.to_owned(), srgb);
            if let Some(&e) = textures.get(&key) {
                return Ok(Some(e));
            }
            let Some(t) = texture(payloads, stem, srgb)? else {
                return Ok(None);
            };
            let e = world.spawn();
            world.insert(
                e,
                Name(if srgb {
                    format!("{stem}.dds@srgb")
                } else {
                    format!("{stem}.dds")
                }),
            );
            world.insert(e, t);
            textures.insert(key, e);
            Ok(Some(e))
        })
        .map_err(|e| format!("{name}: {e}"))?;
        for (policy, count) in resolution.policies {
            *policies.entry(policy).or_default() += count;
        }
        world.insert(
            material_entity,
            ShaderBinding {
                stages: vec![resolution.stage],
                state: Default::default(),
            },
        );
        bound += 1;
    }
    log.count("layered materials bound", bound);
    if without_program > 0 {
        log.count(
            "layered materials whose program the reader does not hold",
            without_program,
        );
    }
    log.count("shader programs", shaders.len() as u64);
    log.count("bound texture files", textures.len() as u64);
    for (policy, count) in policies {
        log.approximate(&policy, count);
    }
    Ok(())
}
