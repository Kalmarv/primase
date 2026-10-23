//! `resolve-fromsoft-shaders`: the layered materials' G-buffer programs as `core.shader` entities and each material's
//! `core.shader_binding`.
use crate::layer_bake::resolve::{Unbound, resolve, resolve_vertex};
use crate::materials::Material;
use crate::sekiro::source::{MaterialShader, Payloads, ShaderBundle, ShaderPrograms, TexturePayloads};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{Shader, ShaderBinding, ShaderFormat};
use primase_core::{ColorSpace, Component, Entity, Mesh, Name, Semantic, TexRole, Texture, World};
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
///
/// A material's vertex program is chosen by the data of the primitives that draw it: when every one of them carries joint
/// indices and weights (the geometry lowering keeps them for a skinned model), the shader's skinned permutation
/// (`<shader>_Gbuf_Skin.vpo`); otherwise the unskinned one (`<shader>_Gbuf.vpo`). Run it after the geometry lowering, so
/// the primitives exist.
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

/// How the primitives of a world draw each material: `(unskinned, skinned)`, a primitive being skinned when it carries
/// joint indices and weights (`Joints(0)` and `Weights(0)`): the mesh's own data, not a guess from a name.
fn draws(world: &World) -> BTreeMap<Entity, (u64, u64)> {
    let mut draws = BTreeMap::<Entity, (u64, u64)>::new();
    for (_, mesh) in world.iter::<Mesh>() {
        for primitive in mesh.primitives() {
            let Some(material) = primitive.material else {
                continue;
            };
            let skinned = primitive.streams.contains_key(&Semantic::Joints(0))
                && primitive.streams.contains_key(&Semantic::Weights(0));
            let entry = draws.entry(material).or_default();
            if skinned {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
    }
    draws
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
    // the vertex programs, each its own entity, by file name
    let mut vertex_shaders: BTreeMap<String, Entity> = BTreeMap::new();
    let draws = draws(world);
    let (mut bound, mut without_program, mut without_vertex) = (0u64, 0u64, 0u64);
    let (mut skinned_materials, mut mixed_materials, mut without_skin) = (0u64, 0u64, 0u64);
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
        // the G-buffer vertex program from the shader's bundle: the skinned permutation when every primitive that draws
        // the material carries joint indices and weights, the unskinned one otherwise (a material drawn both ways has
        // one binding, the unskinned, counted). `@[cl]` is the same program without clip-distance outputs: not needed
        let (unskinned_draws, skinned_draws) = draws.get(&material_entity).copied().unwrap_or((0, 0));
        let skin = skinned_draws > 0 && unskinned_draws == 0;
        if skinned_draws > 0 && unskinned_draws > 0 {
            mixed_materials += 1;
        }
        let bundle = world
            .get::<MaterialShader>(material_entity)
            .and_then(|m| world.get::<ShaderBundle>(m.0));
        let vertex = bundle
            .and_then(|b| b.program(&format!("{}_Gbuf{}.vpo", b.name, if skin { "_Skin" } else { "" })))
            .map(|p| (p.name.clone(), p.code.clone()));
        if skin {
            skinned_materials += 1;
            if vertex.is_none() && bundle.is_some() {
                without_skin += 1;
            }
        }
        let mut stages = Vec::new();
        match vertex {
            Some((vertex_name, code)) => {
                let interface = primase_dxbc::reflection(&code).map_err(|e| format!("{vertex_name}: {e}"))?;
                let entity = *vertex_shaders.entry(vertex_name.clone()).or_insert_with(|| {
                    let entity = world.spawn();
                    world.insert(entity, Name(vertex_name.clone()));
                    world.insert(
                        entity,
                        Shader {
                            format: ShaderFormat::Dxbc,
                            bytecode: code.to_vec(),
                            interface: interface.clone(),
                        },
                    );
                    entity
                });
                stages.push(resolve_vertex(&interface, entity).map_err(|e| format!("{vertex_name}: {e}"))?);
            }
            None => without_vertex += 1,
        }
        stages.push(resolution.stage);
        world.insert(
            material_entity,
            ShaderBinding {
                stages,
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
    if without_vertex > 0 {
        log.count(
            "layered materials without a vertex program (no bundle, or no <shader>_Gbuf.vpo in it)",
            without_vertex,
        );
    }
    if skinned_materials > 0 {
        log.count(
            "layered materials drawn only by skinned primitives (the skinned vertex program)",
            skinned_materials,
        );
    }
    if mixed_materials > 0 {
        log.count(
            "layered materials drawn by skinned and unskinned primitives (bound to the unskinned vertex program)",
            mixed_materials,
        );
    }
    if without_skin > 0 {
        log.count(
            "skinned layered materials whose bundle has no <shader>_Gbuf_Skin.vpo (no vertex program bound)",
            without_skin,
        );
    }
    log.count("shader programs", (shaders.len() + vertex_shaders.len()) as u64);
    log.count("bound texture files", textures.len() as u64);
    for (policy, count) in policies {
        log.approximate(&policy, count);
    }
    Ok(())
}
