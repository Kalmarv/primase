//! The models effects' mesh particles draw (`fromsoft.effect_model`), each made the `core.mesh` its particles draw on its
//! own entity: posed by its bones as `fromsoft-apply-rigid-skinning` poses a placed model, and each mesh's level-0 face
//! set decoded as `lower-fromsoft-geometry` decodes one (positions, normals and UV0 in the world frame, with the further
//! UV sets and the vertex colours a material binary's programs read, [`source_streams`]). Each FLVER material is a
//! `pbr.material` of its own, resolved with its definition (an MTD or a material binary) as a placed model's is
//! ([`crate::materials::resolve_definition`]): its base texture the preview's ([`crate::passes::preview`]: the FLVER's
//! own path, else the definition's), its base colour the definition's diffuse colour ([`base_color`]), lit unless the
//! definition turns lighting off, opaque, alpha-tested or blended by its class. A `GXFlver_Col` program reads only the
//! alpha of its vertex colour ([`GXFLVER_COLOUR`]). What the definitions' programs do beyond that is not drawn, counted.
use super::super::geometry::source_streams;
use crate::components::materials::{Definition, ParameterValue};
use crate::materials::resolve_definition;
use crate::passes::preview::preview;
use crate::passes::rigid::lower_model;
use crate::sekiro::components::effect::{EffectModel, EffectResource, ResourceKind, model_texture};
use crate::sekiro::source::{LayerLayout, SECOND_COLOUR};
use fromsoft_formats::flver::Model;
use primase_core::particles::Blend;
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef};
use primase_core::{Entity, Mesh, Name, Primitive, Semantic, Stream, Topology, World};
use std::collections::{BTreeMap, BTreeSet};

/// What a material whose definition the install does not hold approximates: drawn unlit and blended, its colour texture
/// times the vertex colour.
pub const UNREAD_DEFINITION: &str = "effect model materials whose definition is not held, drawn unlit and blended as their colour texture times the vertex colour";
/// What a material whose definition names an additive blend approximates besides: core's materials have no additive blend
/// (the particle's own blend is the one drawn).
pub const ADDITIVE_BLENDED: &str =
    "effect model materials whose definition names an additive blend, alpha-blended as a material";
/// What every material with a definition approximates: its program's arithmetic beyond the base texture, the diffuse
/// colour, the vertex colour it reads and its lighting switch (its further textures, colours and switches) is not drawn.
pub const PROGRAM_NOT_DRAWN: &str = "effect model materials drawn as their base texture, diffuse colour and lighting switch (their program's further terms not drawn)";

/// The program of the effect models' plain materials (`S[A]_add_GXFlver` and the like). Its pixel program reads only the
/// alpha of the vertex colour the FLVER indexes 1 (`COLOR1.w`) and multiplies the texture's colour by the particle's
/// colour and the material's `g_DiffuseColor` times `g_DiffuseColorPower` (the draw's `FC_DynamicDiffuseColor`): traced
/// on a captured Elden Ring frame (effect 523022's Model 83050, whose `COLOR1` stores (1, 0, 1, a)). Its lowered colour
/// stream is that alpha with white.
pub const GXFLVER_COLOUR: &str = "GXFlver_Col";

/// How a lowered model's materials say its particles draw it: what a particle whose action names no blend of its own (a
/// rich model's: its blend word is unnamed) is drawn with. `blend` is the one every material's definition states by its
/// class (opaque as [`Blend::Replace`], additive as [`Blend::Additive`], blended as [`Blend::Alpha`]), none where a
/// definition is not held, one is alpha-tested or they differ; `lit` where every material is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub blend: Option<Blend>,
    pub lit: bool,
}

/// Every effect model of `world` made the `core.mesh` of its entity, its materials `pbr.material` entities of their own;
/// `resources` are the effect resource entities by kind and number, where a material's textures are found. Returns the
/// models lowered with their looks; one that is already a mesh is an error (the pass ran twice), one that draws nothing or
/// does not pose is left without a mesh, counted.
pub(super) fn lower(
    world: &mut World,
    resources: &BTreeMap<(ResourceKind, i32), Entity>,
    log: &mut PassLog,
) -> Result<BTreeMap<Entity, Look>, String> {
    let models: Vec<(Entity, i32, EffectModel)> = world
        .iter::<EffectModel>()
        .filter_map(|(e, m)| {
            let r = world.get::<EffectResource>(e)?;
            (r.kind == ResourceKind::Model).then(|| (e, r.id, m.clone()))
        })
        .collect();
    let mut lowered = BTreeMap::new();
    for (e, id, model) in models {
        if world.has::<Mesh>(e) {
            return Err(format!("effect model {id} ({e:?}) already has core.mesh"));
        }
        let posed = match lower_model(&model.model, log) {
            Ok(posed) => posed,
            Err(_) => {
                log.omit("effect models that do not pose rigidly, left out", 1);
                continue;
            }
        };
        let (primitives, sides) =
            primitives(&posed, log).map_err(|why| format!("effect model {id}: {why}"))?;
        if primitives.is_empty() {
            log.omit("effect models with no level-0 triangles, left out", 1);
            continue;
        }
        // a material entity for each FLVER material the primitives draw, and the materials whose program reads the vertex
        // colour as GXFlver_Col does
        let mut materials = BTreeMap::new();
        let mut alpha_only = BTreeSet::new();
        let mut blends = Vec::new();
        let mut lit = true;
        for (&index, &double_sided) in &sides {
            let m = posed
                .materials
                .get(index)
                .ok_or_else(|| format!("effect model {id}: material {index} past the model's"))?;
            let entity = world.spawn();
            let found = Found {
                definition: model.definition(m),
                game: model.game,
                resources,
            };
            let (material, program, blend) = material(m, &found, double_sided, entity, log);
            if program == GXFLVER_COLOUR {
                alpha_only.insert(index);
            }
            blends.push(blend);
            lit &= material.shading == Shading::Lit;
            let definition = m.mtd.rsplit(['\\', '/']).next().unwrap_or(&m.mtd);
            world.insert(
                entity,
                Name(format!("sfx model {id} material {} ({definition})", m.name)),
            );
            world.insert(entity, material);
            materials.insert(index, entity);
        }
        let primitives = primitives
            .into_iter()
            .map(|(index, mut p)| {
                p.material = materials.get(&index).copied();
                if alpha_only.contains(&index) {
                    alpha_of_second_colour(&mut p, log);
                }
                p
            })
            .collect();
        world.insert(e, Mesh::new(primitives));
        log.count("effect models lowered", 1);
        let blend = blends
            .iter()
            .all(|b| *b == blends[0])
            .then_some(blends[0])
            .flatten();
        lowered.insert(e, Look { blend, lit });
    }
    Ok(lowered)
}

/// The primitives of a model, each with the FLVER material index it draws (its `material` unset), and for each
/// material drawn whether any of its face sets draws back faces.
type Primitives = (Vec<(usize, Primitive)>, BTreeMap<usize, bool>);

/// The primitives of `model`'s level-0 face sets.
fn primitives(model: &Model, log: &mut PassLog) -> Result<Primitives, String> {
    let mut out = Vec::new();
    let mut sides: BTreeMap<usize, bool> = BTreeMap::new();
    for (index, mesh) in model.meshes.iter().enumerate() {
        let Some(face_set) = mesh.faces.iter().position(|f| f.is_lod0()) else {
            log.omit("effect model meshes with no level-0 face set", 1);
            continue;
        };
        let others = mesh.faces.len() - 1;
        if others > 0 {
            log.approximate(
                "effect model face sets other than the first of level 0 not lowered",
                others as u64,
            );
        }
        let Some(part) = crate::geometry::decode(model, index, face_set).map_err(|e| e.to_string())? else {
            continue;
        };
        part.mesh.validate().map_err(|e| e.to_string())?;
        let layout = LayerLayout::of(model, index).map_err(|e| e.to_string())?;
        let mut streams = BTreeMap::from([
            (Semantic::Position, Stream::F32x3(part.mesh.positions)),
            (Semantic::Normal, Stream::F32x3(part.mesh.normals)),
            (Semantic::TexCoord(0), Stream::F32x2(part.mesh.uv)),
        ]);
        streams.extend(source_streams(&mesh.vertices, &layout, &part.source_vertices));
        *sides.entry(mesh.material).or_default() |= !mesh.faces[face_set].cull;
        out.push((
            mesh.material,
            Primitive {
                topology: Topology::Triangles,
                streams,
                indices: Some(part.mesh.indices),
                material: None,
            },
        ));
    }
    Ok((out, sides))
}

/// What a material is resolved with: its definition where the install holds it, its game, and the effect resources its
/// textures are found among (by the number their stems name).
struct Found<'a> {
    definition: Option<&'a Definition>,
    game: fromsoft_formats::Game,
    resources: &'a BTreeMap<(ResourceKind, i32), Entity>,
}

/// A FLVER material `m` as a `pbr.material` (its counts on `entity`), with the name of its definition's program (empty
/// without one) and the blend its class states ([`Look`]).
fn material(
    m: &fromsoft_formats::flver::Material,
    found: &Found<'_>,
    double_sided: bool,
    entity: Entity,
    log: &mut PassLog,
) -> (PbrMaterial, String, Option<Blend>) {
    let texture = |stem: &str| {
        let (kind, number) = model_texture(stem)?;
        found
            .resources
            .get(&(ResourceKind::of_texture(kind), number))
            .copied()
    };
    let resolved = resolve_definition(m, found.definition, found.game, |stem| texture(stem).is_some());
    let view = preview(&resolved);
    let base_color_texture = view.albedo.as_deref().and_then(texture).map(TextureRef::new);
    if base_color_texture.is_some() && view.uv_scale != [1.0, 1.0] {
        log.approximate_on(entity, "effect model colour textures' UV scale not applied", 1);
    }
    if base_color_texture.is_none() {
        log.approximate_on(
            entity,
            "effect model materials with no colour texture held, drawn in their particles' colour alone",
            1,
        );
    }
    for (named, what) in [
        (&view.normal, "effect model normal textures not drawn"),
        (&view.metallic, "effect model metallic textures not drawn"),
        (&view.emissive, "effect model emissive textures not drawn"),
    ] {
        if named.is_some() {
            log.approximate_on(entity, what, 1);
        }
    }
    let Some(d) = found.definition else {
        log.approximate_on(entity, UNREAD_DEFINITION, 1);
        let material = PbrMaterial {
            base_color_texture,
            metallic: 0.0,
            alpha_mode: AlphaMode::Blend,
            double_sided,
            shading: Shading::Unlit,
            ..PbrMaterial::default()
        };
        return (material, String::new(), None);
    };
    log.approximate_on(entity, PROGRAM_NOT_DRAWN, 1);
    let unlit = matches!(
        d.as_ref().parameter("GXFT_InvalidLighting"),
        Some(ParameterValue::Bool(true))
    );
    let (alpha_mode, blend) = match resolved.class.as_str() {
        "opaque" => (AlphaMode::Opaque, Some(Blend::Replace)),
        "cutout" | "leaves" => (
            AlphaMode::Mask {
                cutoff: resolved.cutoff,
            },
            None,
        ),
        "additive" => {
            log.approximate_on(entity, ADDITIVE_BLENDED, 1);
            (AlphaMode::Blend, Some(Blend::Additive))
        }
        _ => (AlphaMode::Blend, Some(Blend::Alpha)),
    };
    let material = PbrMaterial {
        base_color: base_color(d),
        base_color_texture,
        metallic: 0.0,
        alpha_mode,
        double_sided,
        shading: if unlit { Shading::Unlit } else { Shading::Lit },
        ..PbrMaterial::default()
    };
    (material, d.as_ref().shader_name().to_owned(), blend)
}

/// The colour a definition multiplies its base texture by: `g_DiffuseColor` times `g_DiffuseColorPower` (a
/// `GXFlver_Col` material's: the draw's `FC_DynamicDiffuseColor`, 20 for `S[A]_add_GXFlver`'s (1, 1, 1) and power 20 in
/// every captured frame) times `g_DiffuseMapColor` (an `S[AMSN]_Glow` material's: the draw's `FC_DifMapMulCol`), each 1
/// where the definition states none; alpha 1.
pub fn base_color(d: &Definition) -> [f32; 4] {
    let colour = |name: &str| match d.as_ref().parameter(name) {
        Some(ParameterValue::Float(v)) if v.len() >= 3 => [v[0], v[1], v[2]],
        _ => [1.0; 3],
    };
    let power = match d.as_ref().parameter("g_DiffuseColorPower") {
        Some(ParameterValue::Float(v)) => v.first().copied().unwrap_or(1.0),
        _ => 1.0,
    };
    let (diffuse, map) = (colour("g_DiffuseColor"), colour("g_DiffuseMapColor"));
    let [r, g, b] = [0, 1, 2].map(|i| diffuse[i] * power * map[i]);
    [r, g, b, 1.0]
}

/// `p`'s colour as a `GXFlver_Col` program reads it: white with the alpha of the colour the FLVER indexes 1 (the second
/// colour), in place of the colour indexed 0; a primitive that stores no second colour keeps what it has, counted.
fn alpha_of_second_colour(p: &mut Primitive, log: &mut PassLog) {
    match p.streams.get(&SECOND_COLOUR) {
        Some(Stream::F32x4(colours)) => {
            let alpha = colours.iter().map(|c| [1.0, 1.0, 1.0, c[3]]).collect();
            p.streams.insert(Semantic::Color(0), Stream::F32x4(alpha));
        }
        _ => log.approximate(
            "effect model primitives of a GXFlver_Col material that store no second vertex colour, kept as stored",
            1,
        ),
    }
}
