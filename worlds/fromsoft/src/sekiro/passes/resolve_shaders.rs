//! `resolve-fromsoft-shaders`: the layered materials' G-buffer programs as `core.shader` entities and each material's
//! `core.shader_binding`.
use crate::layer_bake::resolve::{
    PALETTE_ROWS_RULE, Unbound, render_state, resolve, resolve_vertex_for_geometry,
};
use crate::materials::Material;
use crate::metaparam::Metaparam;
use crate::sekiro::source::{MaterialShader, Payloads, ShaderBundle, ShaderPrograms, TexturePayloads};
use iridia::ProgramGraph;
use iridia::output_uses::bind_output_uses;
use iridia_exec::PixelShader;
use iridia_exec::ProgramShader;
use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::{MaterialPrograms, ProgramFacts, Shader, ShaderBinding, ShaderFormat, Stage};
use primase_core::{ColorSpace, Component, Entity, Mesh, Name, Semantic, TexRole, Texture, World};
use std::collections::BTreeMap;

/// `resolve-fromsoft-shaders{unbound-textures=preview|leave}`: for each material with a layered definition, bind its
/// G-buffer pixel program (`<shader>_<switch>_Gbuf.ppo`, from `fromsoft.shader_programs`) as Sekiro's rules say
/// ([`crate::layer_bake::resolve`]): one `core.shader` entity per program (the bytecode as stored, its interface read
/// from it), one `core.texture` entity per bound texture file and colour space (the DDS as stored: block data, every
/// mip), and a `core.shader_binding` on the material: the material's own constants as the words the program reads,
/// the engine's per-view, per-frame and per-object values as roles (never values), the sampler state the source
/// states (none), each texture slot and the G-buffer's outputs by their role (a lane the layout leaves Unknown is
/// Coverage where the program's discard tests what it writes there, Opacity where the target's blend weighs by its alpha:
/// [`iridia::output_uses`], counted by program family). A material whose program the reader does not hold is
/// counted and left unbound.
///
/// The option is required: it says what a texture slot with no texture, or whose file the reader does not hold,
/// becomes. `preview` gives it a stated colour for its role (a mid-grey albedo, a flat normal, zero elsewhere), a
/// counted approximation; `leave` binds it `Missing`, so a consumer that needs the slot refuses.
///
/// A material's vertex program is chosen by the data of the primitives that draw it: when every one of them carries joint
/// indices and weights (the geometry lowering keeps them for a skinned model), the shader's skinned permutation
/// (`<shader>_Gbuf_Skin.vpo`); otherwise the unskinned one (`<shader>_Gbuf.vpo`). Run it after the geometry lowering, so
/// the primitives exist.
///
/// A material the game draws in its forward pass (`g_BlendMode` 2, [`crate::materials::LayeredMaterial::is_forward`]) is bound to
/// its `<shader>[_<switch>]_Fwd[_[A]].ppo` and `_Fwd[_Skin].vpo` instead (the plain forward permutation: the shadowed ones,
/// `_FwdSdw` and `_FwdSSdw`, are the engine's per-instance choice), whose render target 0 is the finished colour; what the
/// forward programs read from the engine (the scene depth and SSAO, the irradiance volumes, the IBL cube maps, the volume fog)
/// is bound as engine textures at zero, counted by name, never a material's.
///
/// The binding's render state is [`render_state`]'s: what a capture of the running game settles from the material's MTD
/// (depth test and write, the alpha blend of `g_BlendMode` 2), and nothing it does not.
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
}

/// `resolve-fromsoft-shaders`' declaration ([`primase_core::pass::PassRegistry::add`]): what a texture slot with no
/// texture reads is the program's.
pub const RESOLVE_FROMSOFT_SHADERS: PassSpec = PassSpec {
    name: "resolve-fromsoft-shaders",
    options: &[OptionSpec::choice::<Unbound>(
        "unbound-textures",
        "a texture slot with no texture reads a stated colour for its role (counted), or is left missing",
    )],
    make: |g| {
        Ok(Box::new(ResolveFromSoftShaders::new(
            g.choice("unbound-textures"),
        )))
    },
};

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
        Options::from([("unbound-textures".to_owned(), self.unbound.text().to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Shader::NAME,
            ShaderBinding::NAME,
            MaterialPrograms::NAME,
            ProgramFacts::NAME,
            Texture::NAME,
            Name::NAME,
        ]
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

/// Which record defines a candidate material, for the binding and its counts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Sekiro's layered materials (their own definition and bindings).
    Layered,
    /// The other MTD-defined materials (Sekiro's SAT shaders without a static switch, the fixed-function ones).
    Other,
    /// Material binaries (Elden Ring's).
    Binary,
}
impl Kind {
    const ALL: [Kind; 3] = [Kind::Layered, Kind::Other, Kind::Binary];
    fn word(self) -> &'static str {
        match self {
            Kind::Layered => "layered",
            Kind::Other => "other",
            Kind::Binary => "material binary",
        }
    }
}

/// Counts of one kind of material.
#[derive(Default, Clone, Copy)]
struct Tally {
    bound: u64,
    without_program: u64,
    without_vertex: u64,
    skinned: u64,
    mixed: u64,
    without_skin: u64,
    blended: u64,
    blend_unstated: u64,
    without_depth_write: u64,
    state_unstated: u64,
}

fn run(
    world: &mut World,
    programs: &Payloads,
    payloads: &Payloads,
    unbound: Unbound,
    log: &mut PassLog,
) -> Result<(), String> {
    // every defined material, with the record that defines it: a layered material's own definition and bindings, the
    // others' (an MTD's or a material binary's) as the material states them
    let candidates: Vec<(Entity, Material, Kind)> = world
        .iter::<Material>()
        .filter_map(|(e, m)| {
            let kind = match (&m.layered, &m.definition) {
                (Some(_), _) => Kind::Layered,
                (None, Some(crate::materials::Definition::Mtd(_))) => Kind::Other,
                (None, Some(crate::materials::Definition::Matbin(_))) => Kind::Binary,
                (None, None) => return None,
            };
            Some((e, m.clone(), kind))
        })
        .collect();
    // the programs read once, each its own entity, with its value graph (or why there is none)
    let mut shaders: BTreeMap<String, (Entity, ProgramShader, Result<ProgramGraph, String>)> =
        BTreeMap::new();
    // the textures spawned, by stem and colour space
    let mut textures: BTreeMap<(String, bool), Entity> = BTreeMap::new();
    // the vertex programs, each its own entity with the interface read once, by file name
    let mut vertex_shaders: BTreeMap<String, (Entity, Shader)> = BTreeMap::new();
    // each bundle's parameter definition, taken once per bundle entity
    let mut parameters: BTreeMap<Entity, Option<Metaparam>> = BTreeMap::new();
    let draws = draws(world);
    let mut tally: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut policies: BTreeMap<String, u64> = BTreeMap::new();
    // the output lanes the program's discards or its target's blend show a use for, by role and program family
    let mut uses: BTreeMap<(&'static str, String), u64> = BTreeMap::new();
    let mut program_names = BTreeMap::new();
    for (name, code) in programs {
        program_names
            .entry(crate::names::fold(name))
            .or_insert((name, code));
    }
    for (material_entity, source, kind) in candidates {
        let t = tally.entry(kind.word()).or_default();
        let material = match (&source.layered, source.view()) {
            (Some(l), _) => l.view(),
            (None, Some(view)) => view,
            (None, None) => unreachable!("a candidate has a definition"),
        };
        // the shader's bundle, read where it is: a material takes from it only what is not taken yet (a program not read
        // yet, the bundle's parameters once)
        let bundle_entity = world.get::<MaterialShader>(material_entity).map(|m| m.0);
        let bundle = bundle_entity.and_then(|e| world.get::<ShaderBundle>(e));
        // the game whose program it is: its bundle's (Sekiro's layered programs the reader kept are Sekiro's)
        let game = bundle.map_or(crate::Game::Sekiro, |b| b.game);
        let requested = material.pass_program();
        // the layered programs the reader kept (DXBC), else the one the shader's bundle holds under that name
        let found = program_names
            .get(&crate::names::fold(&requested))
            .map(|&(name, code)| (name.clone(), ShaderFormat::Dxbc, code.clone()))
            .or_else(|| {
                bundle
                    .and_then(|b| b.program(&requested))
                    .map(|p| (p.name.clone(), p.format, p.code.clone()))
            });
        let Some((name, format, bytes)) = found else {
            world.insert(
                material_entity,
                MaterialPrograms(vec![ProgramFacts::unavailable(
                    requested,
                    Stage::Pixel,
                    None,
                    "the reader does not hold the requested pixel program",
                )]),
            );
            t.without_program += 1;
            continue;
        };
        if let (Some(e), Some(b)) = (bundle_entity, bundle) {
            parameters.entry(e).or_insert_with(|| b.parameters.clone());
        }
        // the G-buffer vertex program from the shader's bundle: the skinned permutation when every primitive that draws
        // the material carries joint indices and weights, the unskinned one otherwise (a material drawn both ways has one
        // binding, the unskinned, counted). `@[cl]` is the same program without clip-distance outputs: not needed
        let (unskinned_draws, skinned_draws) = draws.get(&material_entity).copied().unwrap_or((0, 0));
        let skin = skinned_draws > 0 && unskinned_draws == 0;
        if skinned_draws > 0 && unskinned_draws > 0 {
            t.mixed += 1;
        }
        // `<shader>_<pass>.vpo`; the fixed-function shaders spell the G-buffer's `_GBuf`
        let vertex = bundle
            .and_then(|b| {
                [material.pass(), "GBuf"].iter().find_map(|g| {
                    b.program(&format!("{}_{g}{}.vpo", b.name, if skin { "_Skin" } else { "" }))
                })
            })
            .map(|p| (p.name.clone(), p.format, p.code.clone()));
        if skin {
            t.skinned += 1;
            if vertex.is_none() && bundle.is_some() {
                t.without_skin += 1;
            }
        }
        if !shaders.contains_key(&name) {
            let lifted = match crate::passes::program_reads::lift(format, &bytes) {
                Ok(lifted) => lifted,
                Err(error) => {
                    world.insert(
                        material_entity,
                        MaterialPrograms(vec![ProgramFacts::unavailable(
                            &name,
                            Stage::Pixel,
                            Some(iridia::source_hash(&bytes)),
                            error.to_string(),
                        )]),
                    );
                    return Err(format!("{name}: {error}"));
                }
            };
            let facts = lifted.facts(&name);
            let parsed = match ProgramShader::new(lifted) {
                Ok(parsed) => parsed,
                Err(error) => {
                    // This is an executor refusal, not a failed lift: retain the successful source facts.
                    world.insert(material_entity, MaterialPrograms(vec![facts]));
                    return Err(format!("{name}: {error}"));
                }
            };
            let entity = world.spawn();
            world.insert(entity, Name(name.clone()));
            world.insert(entity, facts);
            world.insert(
                entity,
                Shader {
                    format,
                    bytecode: bytes.to_vec(),
                    interface: PixelShader::interface(&parsed).clone(),
                },
            );
            let graph = parsed.program().graph();
            shaders.insert(name.clone(), (entity, parsed, graph));
        }
        let (shader_entity, parsed, graph) = &shaders[&name];
        let metaparam = bundle_entity
            .and_then(|e| parameters.get(&e))
            .and_then(Option::as_ref);
        let mut refused = None;
        let resolution = resolve(
            parsed,
            *shader_entity,
            &material,
            game,
            metaparam,
            unbound,
            &mut |stem: &str, srgb: bool| {
                let key = (stem.to_owned(), srgb);
                if let Some(&e) = textures.get(&key) {
                    return Ok(Some(e));
                }
                // a texture the DDS reader refuses is one this read does not have: the slot is resolved as an unbound
                // one is, by the unbound-textures policy, and the refusal counted by its reason
                let t = match texture(payloads, stem, srgb) {
                    Ok(Some(t)) => t,
                    Ok(None) => return Ok(None),
                    Err(e) => {
                        refused = Some(e.to_string());
                        return Ok(None);
                    }
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
            },
        )
        .map_err(|e| format!("{name}: {e}"))?;
        if let Some(why) = refused {
            log.omit(
                &format!("layer textures the DDS reader refuses, resolved as unbound ({why})"),
                1,
            );
        }
        for (policy, count) in resolution.policies {
            *policies.entry(policy).or_default() += count;
        }
        let mut stages = Vec::new();
        match vertex {
            Some((vertex_name, format, code)) => {
                if !vertex_shaders.contains_key(&vertex_name) {
                    // the vertex program's interface: a DXBC program's reflection, a DXIL one's as the lifter reads it
                    let interface = match format {
                        ShaderFormat::Dxbc => {
                            primase_dxbc::reflection(&code).map_err(|e| format!("{vertex_name}: {e}"))?
                        }
                        _ => {
                            primase_dxil::lift(&code)
                                .map_err(|e| format!("{vertex_name}: {e}"))?
                                .interface
                        }
                    };
                    let entity = world.spawn();
                    world.insert(entity, Name(vertex_name.clone()));
                    let shader = Shader {
                        format,
                        bytecode: code.to_vec(),
                        interface,
                    };
                    world.insert(entity, shader.clone());
                    vertex_shaders.insert(vertex_name.clone(), (entity, shader));
                }
                let (entity, shader) = &vertex_shaders[&vertex_name];
                let vertex = resolve_vertex_for_geometry(shader, *entity, world, material_entity)
                    .map_err(|e| format!("{vertex_name}: {e}"))?;
                if let Some(why) = vertex.undecided {
                    // not a finding: the matrix is taken as camera-relative, as most programs of the corpus are
                    *policies
                        .entry(format!(
                            "vertex program's view-projection undecided by the probe ({why}): taken as camera-relative"
                        ))
                        .or_default() += 1;
                }
                if vertex.palette_rows {
                    *policies.entry(PALETTE_ROWS_RULE.to_owned()).or_default() += 1;
                }
                if vertex.palette_bound {
                    *policies
                        .entry(crate::layer_bake::resolve::PALETTE_BOUND_POLICY.to_owned())
                        .or_default() += 1;
                }
                stages.push(vertex.stage);
            }
            None => t.without_vertex += 1,
        }
        stages.push(resolution.stage);
        // the render state: what Sekiro's capture settles from an MTD ([`render_state`]); a material binary's is not
        // established (Elden Ring is not run), so none is stated, counted
        let state = match (&source.layered, &source.definition) {
            (Some(l), _) => render_state(l),
            (None, Some(crate::materials::Definition::Mtd(d))) => {
                render_state(&crate::materials::LayeredMaterial {
                    mtd: source.mtd.clone(),
                    definition: d.clone(),
                    bindings: source.bindings.clone(),
                })
            }
            _ => {
                t.state_unstated += 1;
                Default::default()
            }
        };
        if kind != Kind::Binary {
            match material.blend_mode().unwrap_or(0) {
                0 => {}
                2 => t.blended += 1,
                _ => t.blend_unstated += 1,
            }
        }
        if state.depth.is_some_and(|d| !d.write) {
            t.without_depth_write += 1;
        }
        // the lanes left Unknown that the program's discards or the target's blend show a use for
        if let Some(pixel) = stages.iter_mut().find(|s| s.stage == Stage::Pixel) {
            let bound = bind_output_uses(
                &mut pixel.outputs,
                PixelShader::interface(parsed),
                graph.as_ref().ok(),
                &state,
            );
            let family = format!(
                "{}{}",
                material.pass(),
                if material.alpha_reference() > 0 {
                    "_[A]"
                } else {
                    ""
                }
            );
            for (role, lanes) in [("Coverage", bound.coverage), ("Opacity", bound.opacity)] {
                if lanes > 0 {
                    *uses.entry((role, family.clone())).or_default() += u64::from(lanes);
                }
            }
        }
        // Reuse the very lift used to resolve this material, including the material-specific output roles.
        let facts = world
            .get::<ProgramFacts>(*shader_entity)
            .ok_or("resolved pixel program has no facts")?
            .clone()
            .bound(
                *shader_entity,
                &stages
                    .iter()
                    .find(|stage| stage.stage == Stage::Pixel)
                    .ok_or("resolved material has no pixel stage")?
                    .outputs,
            );
        world.insert(material_entity, MaterialPrograms(vec![facts]));
        world.insert(material_entity, ShaderBinding { stages, state });
        t.bound += 1;
    }
    for kind in Kind::ALL {
        let which = kind.word();
        let t = tally.get(which).copied().unwrap_or_default();
        // the layered keys are counted as ever, the others only where there are some
        if kind != Kind::Layered && t.bound + t.without_program == 0 {
            continue;
        }
        log.count(&format!("{which} materials bound"), t.bound);
        if t.without_program > 0 {
            log.count(
                &format!("{which} materials whose program the reader or the shader's bundle does not hold"),
                t.without_program,
            );
        }
        if t.without_vertex > 0 {
            log.count(
                &format!(
                    "{which} materials without a vertex program (no bundle, or no <shader>_Gbuf.vpo in it)"
                ),
                t.without_vertex,
            );
        }
        if t.skinned > 0 {
            log.count(
                &format!("{which} materials drawn only by skinned primitives (the skinned vertex program)"),
                t.skinned,
            );
        }
        if t.mixed > 0 {
            log.count(
                &format!(
                    "{which} materials drawn by skinned and unskinned primitives (bound to the unskinned vertex program)"
                ),
                t.mixed,
            );
        }
        if t.blended > 0 {
            log.count(
                &format!(
                    "{which} materials alpha blended (g_BlendMode 2: SrcAlpha/InvSrcAlpha in the state)"
                ),
                t.blended,
            );
        }
        if t.blend_unstated > 0 {
            log.count(
                &format!("{which} materials of a blend mode other than 0 and 2 (no blend stated: no draw of one was matched)"),
                t.blend_unstated,
            );
        }
        if t.without_depth_write > 0 {
            log.count(
                &format!("{which} materials that disable depth write (g_GX_bDisableDepthWrite)"),
                t.without_depth_write,
            );
        }
        if t.state_unstated > 0 {
            log.count(
                &format!("{which} materials with no render state stated (the game's is not established)"),
                t.state_unstated,
            );
        }
        if t.without_skin > 0 {
            log.count(
                &format!(
                    "skinned {which} materials whose bundle has no <shader>_Gbuf_Skin.vpo (no vertex program bound)"
                ),
                t.without_skin,
            );
        }
    }
    for ((role, family), lanes) in uses {
        log.count(
            &format!("pixel output lanes made {role} by the program's discard or the target's blend, over the materials bound ({family} programs)"),
            lanes,
        );
    }
    log.count("shader programs", (shaders.len() + vertex_shaders.len()) as u64);
    log.count("bound texture files", textures.len() as u64);
    for (policy, count) in policies {
        log.approximate(&policy, count);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "owned fixture programs")]
mod program_facts_tests {
    #[test]
    fn sekiro_and_elden_fixture_lifts_keep_source_counts_hash_and_register_inputs() {
        use primase_core::shader::{InstructionUnit, ShaderFormat, Stage};
        for (name, format, bytes, unit) in [
            (
                "Sekiro alpha reference fixture",
                ShaderFormat::Dxbc,
                include_bytes!("../../../tests/fixtures/alpha_ref_tested.dxbc").as_slice(),
                InstructionUnit::DxbcInstructions,
            ),
            (
                "Elden alpha reference fixture",
                ShaderFormat::Dxil,
                include_bytes!("../../../tests/fixtures/alpha_ref_tested.dxil").as_slice(),
                InstructionUnit::DxilInstructions,
            ),
        ] {
            let lifted = crate::passes::program_reads::lift(format, bytes).unwrap();
            let facts = lifted.facts(name);
            assert_eq!(facts.stage, Stage::Pixel);
            assert_eq!(facts.hash, Some(iridia::source_hash(bytes)));
            assert_eq!(facts.instruction_count.unwrap().unit, unit);
            assert!(facts.instruction_count.unwrap().count > 0);
            assert_eq!(
                facts.inputs,
                lifted
                    .interface
                    .inputs
                    .iter()
                    .filter(|input| input.used)
                    .cloned()
                    .collect::<Vec<_>>()
            );
            assert!(!facts.inputs.is_empty());
            assert!(facts.inputs.iter().all(|input| input.provenance.d3d.is_some()));
        }
    }
}
