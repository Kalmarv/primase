//! Opt-in static bake assembly. The reader's source convention remains intact;
//! consumer normal conversion still happens after this step.
use super::{
    layers::{AreaBake, BakeReport},
    normals, pbr,
    reflectance::Reflectance,
};
use crate::{
    Result, invalid,
    layer_bake::{Inputs, SECOND_UV_APPROXIMATION, grouping},
};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, World,
};
use std::collections::{BTreeMap, BTreeSet};

fn texture(world: &mut World, name: String, value: Texture) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Name(name));
    world.insert(entity, value);
    entity
}
fn rgba(size: u32, pixels: Vec<u8>, normal: bool) -> Texture {
    Texture {
        width: size,
        height: size,
        format: TexFormat::Rgba8Unorm,
        levels: vec![pixels],
        color_space: if normal {
            ColorSpace::Linear
        } else {
            ColorSpace::Srgb
        },
        role: if normal { TexRole::Normal } else { TexRole::Color },
        normal: normal.then_some(normals::SOURCE),
    }
}

// Neutral ORM B is a multiplier. The traced dielectric output uses factor zero;
// unsupported output has no metallic evidence and is the counted dielectric default.
fn channel_material(
    world: &mut World,
    name: &str,
    alpha_mode: AlphaMode,
    image: crate::layer_bake::Bake,
    reflectance: Reflectance,
    log: &mut primase_core::pass::PassLog,
) -> Result<PbrMaterial> {
    let size = image.size;
    let base = texture(world, format!("{name}_a"), rgba(size, image.albedo, false));
    let normal = texture(world, format!("{name}_n"), rgba(size, image.normal, true));
    let mut material = PbrMaterial {
        base_color_texture: Some(TextureRef::new(base)),
        normal_texture: Some(TextureRef::new(normal)),
        alpha_mode,
        // sidedness is each variant's: the face-set cull of the parts it draws
        double_sided: false,
        ..Default::default()
    };
    if let Some(gloss) = &image.gloss {
        let orm = texture(
            world,
            format!("{name}_orm"),
            pbr::orm_from_gloss(size, size, gloss)?,
        );
        material.metallic_roughness_texture = Some(TextureRef::new(orm));
        material.roughness = 1.;
        pbr::note_gloss_packing(log);
    }
    if let Some(emission) = &image.emissive {
        let (pixels, strength, zeroed) = pbr::emissive_from_linear(size, size, emission)?;
        let emission = texture(world, format!("{name}_e"), pixels);
        material.emissive_texture = Some(TextureRef::new(emission));
        material.emissive = [1.; 3];
        material.emissive_strength = strength;
        log.approximate("emission quantized to sRGB UNORM8", 1);
        log.count("nonzero emission texels quantized entirely black", zeroed);
    }
    material.metallic = match reflectance {
        Reflectance::Dielectric => 0.,
        Reflectance::Unsupported => pbr::UNKNOWN_METALLIC,
    };
    material.specular = reflectance.specular();
    Ok(material)
}

fn source_variant(
    world: &mut World,
    original: Entity,
    template: &PbrMaterial,
    cull: bool,
    name: String,
) -> Result<Entity> {
    let source = world
        .get::<crate::materials::Material>(original)
        .ok_or_else(|| invalid("area bake", 0, "missing source provenance"))?
        .clone();
    let mut material = template.clone();
    material.alpha_mode = match source.class.as_str() {
        "cutout" | "leaves" => AlphaMode::Mask {
            cutoff: source.cutoff,
        },
        "alpha" | "decal" | "water" => AlphaMode::Blend,
        _ => AlphaMode::Opaque,
    };
    material.double_sided = !cull;
    let entity = world.spawn();
    world.insert(entity, Name(name));
    if let Some(layered) = &source.layered {
        world.insert(entity, layered.clone());
    }
    world.insert(entity, source);
    world.insert(entity, material);
    Ok(entity)
}

/// Bake every group at its reference size capped at `max_size`, its programs sampled at `sample_time` seconds, and
/// replace the grouped parts by their levels' parts ([`World::rebuild_primitives`], every per-primitive component
/// following). Returns the report and the primitives (prototype, index after the rebuild) the baked parts became.
/// `baked_text` is what the bake itself approximates, as the pass's declaration puts it for this run: recorded once on
/// every material the bake makes ([`primase_core::pass::PassLog::approximate_on`]).
pub(super) fn assemble(
    bake: &AreaBake,
    prototypes: &BTreeMap<String, Entity>,
    max_size: u32,
    sample_time: f32,
    baked_text: &str,
    world: &mut World,
) -> Result<(BakeReport, BTreeSet<(Entity, usize)>)> {
    let mut report = BakeReport::default();
    // each baked part, with its vertex map when its vertices were compacted
    let mut replacements = BTreeMap::<(Entity, usize), Vec<(Primitive, Option<Vec<u32>>)>>::new();
    let mut replaced_materials = BTreeSet::new();
    let mut variants = BTreeMap::new();
    // Each group's reasons and the materials it made, applied once the bake has removed the materials nothing kept:
    // an attributed material has to be alive after the pass (`PassLog::approximated_on`).
    let mut records: Vec<(Vec<Entity>, primase_core::pass::PassLog)> = Vec::new();
    let threads = primase_core::parallel::Threads::of(world);
    for (group_index, group) in bake.groups.iter().enumerate() {
        // What this group's bake approximates, and the materials it makes: merged into the report at the group's end,
        // each reason on the materials it concerns.
        let mut group_log = primase_core::pass::PassLog::default();
        let mut made: Vec<Entity> = Vec::new();
        let prepared = bake.prepare(world, group_index, sample_time)?;
        let size = prepared.bake_size(max_size)?;
        let mut templates = Vec::new();
        for (level, center) in group.centers.iter().enumerate() {
            let channels = prepared.bake_with_reflectance(
                threads,
                size,
                group.period,
                Inputs {
                    weights_snow: center[..4].try_into().expect("four weights"),
                    tint_color: [center[4], center[5], center[6], 1.],
                    detail: center[7],
                },
            )?;
            let reflectance = Reflectance::classify(&group.material, size, channels.reflectance.as_deref());
            let image = channels.image;
            if level == 0 {
                report.materials += 1;
                report.gloss_materials += usize::from(image.gloss.is_some());
                report.missing_gloss_materials += usize::from(image.gloss.is_none());
                report.emissive_materials += usize::from(image.emissive.is_some());
                report.missing_emissive_materials += usize::from(image.emissive.is_none());
                for (policy, count) in &image.policies {
                    if policy != "constant layer weights, tint and detail" {
                        group_log.approximate(policy, *count);
                    }
                }
            }
            // This policy counts output texels, unlike source-group policies.
            if let Some(&texels) = image.policies.get("constant layer weights, tint and detail") {
                group_log.approximate("constant layer weights, tint and detail", texels);
            }
            report.variants += 1;
            report.nonunit_uv_variants += usize::from(group.period != [1.; 2]);
            let name = format!("bake_{group_index:04}_{level}");
            let alpha_mode = crate::materials::alpha_mode(
                &group.material,
                world.get::<primase_core::shader::ShaderBinding>(group.entity),
                false,
                &mut group_log,
            );
            let material =
                channel_material(world, &name, alpha_mode, image, reflectance, &mut group_log)?;
            templates.push((material, reflectance));
        }
        for source in &group.parts {
            let entity = *prototypes
                .get(&source.prototype)
                .ok_or_else(|| invalid("area bake", source.part, "missing prototype"))?;
            let primitive = world
                .get::<Mesh>(entity)
                .and_then(|m| m.primitives().get(source.part))
                .ok_or_else(|| invalid("area bake", source.part, "missing source part"))?
                .clone();
            let original = primitive
                .material
                .ok_or_else(|| invalid("area bake", source.part, "missing source material"))?;
            let identity = world
                .get::<crate::materials::Material>(original)
                .ok_or_else(|| invalid("area bake", source.part, "missing source provenance"))?
                .identity()?;
            let cull = *world
                .get::<crate::sekiro::source::FaceCull>(entity)
                .and_then(|c| c.primitives.get(source.part))
                .ok_or_else(|| {
                    invalid(
                        "area bake",
                        source.part,
                        "layered part without fromsoft.face_cull",
                    )
                })?;
            // the tiling the geometry pass multiplied into these coordinates, if it did
            let scale = world.get::<super::UvTiling>(original).map_or([1.; 2], |t| t.0);
            if prepared.reads_second_uv {
                let (meshes, vertices) = second_uv_differs(&primitive, scale);
                let by = |what: &str| {
                    format!(
                        "{SECOND_UV_APPROXIMATION}, {}: {what}",
                        group.material.mtd.rsplit(['/', '\\']).next().unwrap_or("")
                    )
                };
                if meshes > 0 {
                    group_log.approximate(&by("parts whose stored UV1 differs from UV0"), meshes);
                    group_log.approximate(&by("vertices whose stored UV1 differs from UV0"), vertices);
                }
            }
            replaced_materials.insert(original);
            // Only instantiate levels used by this source part. Equal bake inputs
            // share textures, never the full source material/GX identity: a variant is one source material's (its
            // identity record's) at one level and one face-set cull, named for that record's digest (and `_two_sided`
            // for parts that do not cull), so its name follows the source and not the order entities were spawned in.
            let mut materials = vec![original; templates.len()];
            for &level in &source.assignments {
                if level >= templates.len() {
                    return Err(invalid("area bake", source.part, "invalid level"));
                }
                let key = (identity.clone(), group_index, level, cull);
                let material = if let Some(&e) = variants.get(&key) {
                    e
                } else {
                    let digest = crate::materials::identity_digest(&identity);
                    let sided = if cull { "" } else { "_two_sided" };
                    let e = source_variant(
                        world,
                        original,
                        &templates[level].0,
                        cull,
                        format!("bake_{group_index:04}_{level}_source_{digest}{sided}"),
                    )?;
                    templates[level].1.record_material(&mut group_log);
                    variants.insert(key, e);
                    made.push(e);
                    e
                };
                materials[level] = material;
            }
            let parts = split(&primitive, &source.assignments, &materials, scale, group.period)?;
            if replacements.insert((entity, source.part), parts).is_some() {
                return Err(invalid("area bake", source.part, "source part grouped twice"));
            }
        }
        records.push((made, group_log));
    }
    report.source_materials = replaced_materials.len();
    let candidates = world.closure(replaced_materials);
    let mut baked = BTreeSet::new();
    for &entity in prototypes.values() {
        // the baked parts replace their source part; every per-primitive component follows, then every per-vertex
        // one each compacted part's vertices
        let mut gathers = Vec::new();
        world
            .rebuild_primitives(entity, |original| {
                let mut primitives = Vec::new();
                let mut from = Vec::new();
                for (index, primitive) in original.primitives().iter().enumerate() {
                    match replacements.remove(&(entity, index)) {
                        Some(parts) => {
                            baked.extend(
                                (primitives.len()..primitives.len() + parts.len()).map(|i| (entity, i)),
                            );
                            from.extend(std::iter::repeat_n(index, parts.len()));
                            for (part, gather) in parts {
                                if let Some(g) = gather {
                                    gathers.push((primitives.len(), g));
                                }
                                primitives.push(part);
                            }
                        }
                        None => {
                            from.push(index);
                            primitives.push(primitive.clone());
                        }
                    }
                }
                Ok((primitives, from))
            })
            .map_err(|e| invalid("area bake", 0, e))?;
        for (primitive, gather) in gathers {
            world
                .remap_vertices(entity, primitive, &gather)
                .map_err(|e| invalid("area bake", primitive, e))?;
        }
    }
    let retained = world.closure(world.entities().filter(|e| !candidates.contains(e)));
    for entity in candidates.difference(&retained) {
        world.despawn(*entity);
    }
    // What each group's bake approximated belongs to every material it made that the bake kept: a bake is reported on
    // the material, not as a provenance record. The bake itself is one record per
    // material, its text the pass's; the group's other reasons keep their counts and gain the materials they concern.
    for (made, mut group_log) in records {
        let made: Vec<Entity> = made.into_iter().filter(|e| world.is_alive(*e)).collect();
        let reasons: Vec<String> = group_log.approximations.keys().cloned().collect();
        for &e in &made {
            group_log.approximate_on(e, baked_text, 1);
        }
        for &e in &made {
            for reason in &reasons {
                group_log.approximated_on.entry(reason.clone()).or_default().insert(e);
            }
        }
        report.policies.absorb(group_log);
    }
    Ok((report, baked))
}

fn split(
    source: &Primitive,
    assignments: &[usize],
    materials: &[Entity],
    scale: [f32; 2],
    period: [f32; 2],
) -> Result<Vec<(Primitive, Option<Vec<u32>>)>> {
    let denominator: [f32; 2] = std::array::from_fn(|i| scale[i] * period[i]);
    if denominator.iter().any(|x| !x.is_finite() || *x == 0.) {
        return Err(invalid("area bake", 0, "invalid bake UV scale"));
    }
    let split = grouping::split(source, assignments, materials.len())?;
    // Preserve authored ordering whenever this primitive uses only one level,
    // even if other primitives sharing its material use additional levels: its vertices then stay as they are.
    let whole = split.len() == 1;
    split
        .into_iter()
        .map(|(level, part, gather)| {
            let (mut part, gather) = if whole {
                (source.clone(), None)
            } else {
                (part, Some(gather))
            };
            let Some(Stream::F32x2(uv)) = part.streams.get_mut(&Semantic::TexCoord(0)) else {
                return Err(invalid("area bake", 0, "missing main UV stream"));
            };
            for uv in uv {
                for i in 0..2 {
                    uv[i] /= denominator[i];
                }
            }
            part.material = Some(materials[level]);
            Ok((part, gather))
        })
        .collect()
}

/// Whether a part's stored second UV set (`TexCoord(1)`) differs from its first, un-tiled by `scale` (the factor the
/// geometry pass multiplied into `TexCoord(0)`): `(1, vertices that differ)` or `(0, 0)`. A part with no stored second
/// set counts nothing: what the engine feeds its program is not known.
fn second_uv_differs(part: &Primitive, scale: [f32; 2]) -> (u64, u64) {
    let (Some(Stream::F32x2(first)), Some(Stream::F32x2(second))) = (
        part.streams.get(&Semantic::TexCoord(0)),
        part.streams.get(&Semantic::TexCoord(1)),
    ) else {
        return (0, 0);
    };
    let differs = first
        .iter()
        .zip(second)
        .filter(|(a, b)| (0..2).any(|i| (b[i] - a[i] / scale[i]).abs() > 1e-6 * b[i].abs().max(1.)))
        .count() as u64;
    (u64::from(differs > 0), differs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_jar_reflectance_and_unchanged_channels() -> std::result::Result<(), Box<dyn std::error::Error>> {
        use crate::layer_bake::{Prepared, wrapped_texture};
        use primase_texcodec::TexDecoder;
        use sha2::{Digest, Sha256};
        use std::sync::Arc;
        // the oracle's folder: reflectance.json and the program and textures it names, by paths relative to it
        let Some(folder) = primase_testdata::oracle(
            primase_testdata::OracleSet::Scripts,
            primase_testdata::Game::Sekiro,
            "reflectance",
        ) else {
            return Ok(());
        };
        let oracle: serde_json::Value =
            serde_json::from_slice(&primase_testdata::read_input(folder.join("reflectance.json"))?)?;
        let read = |path: &str, expected: &str| -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
            let bytes = primase_testdata::read_input(folder.join(path))?;
            assert_eq!(
                Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                expected
            );
            Ok(bytes)
        };
        let shader = read(
            oracle["shader"].as_str().unwrap(),
            oracle["shader_sha256"].as_str().unwrap(),
        )?;
        assert_eq!(
            oracle["shader_sha256"],
            "bdb79b8f5c974834ea03232810d9edd28d54f3edc23c7573f5484fdebf010daf"
        );
        let offset = oracle["literal_offset"].as_u64().unwrap() as usize;
        let literal = u32::from_le_bytes(shader[offset..offset + 4].try_into()?);
        assert_eq!(literal, oracle["literal_bits"].as_u64().unwrap() as u32);
        assert_eq!(literal, 0x3e31_5cac);
        // Reconstruct through the existing source resolver; these source types
        // intentionally have no public Deserialize contract.
        let src = &oracle["source"];
        let def = &src["definition"];
        let definition = crate::mtd::MaterialDefinition {
            shader: def["shader"].as_str().unwrap().into(),
            description: def["description"].as_str().unwrap().into(),
            parameters: def["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    let value = &p["value"];
                    crate::mtd::Parameter {
                        name: p["name"].as_str().unwrap().into(),
                        value: if let Some(b) = value["Bool"].as_bool() {
                            crate::mtd::Value::Bool(b)
                        } else if let Some(v) = value.get("Int") {
                            crate::mtd::Value::Int(serde_json::from_value(v.clone()).unwrap())
                        } else {
                            crate::mtd::Value::Float(serde_json::from_value(value["Float"].clone()).unwrap())
                        },
                    }
                })
                .collect(),
            textures: def["textures"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| crate::mtd::Texture {
                    kind: t["kind"].as_str().unwrap().into(),
                    path: t["path"].as_str().unwrap().into(),
                    uv_number: t["uv_number"].as_i64().unwrap() as i32,
                    shader_data_index: t["shader_data_index"].as_i64().unwrap() as i32,
                    extra: serde_json::from_value(t["extra"].clone()).unwrap(),
                })
                .collect(),
            block_lengths: def["block_lengths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| crate::mtd::BlockLength {
                    start: b["start"].as_u64().unwrap() as usize,
                    kind: b["kind"].as_u64().unwrap() as u32,
                    declared: b["declared"].as_u64().unwrap() as usize,
                    consumed: b["consumed"].as_u64().unwrap() as usize,
                })
                .collect(),
        };
        let source = crate::materials::resolve(
            &crate::flver::Material {
                name: "jar".into(),
                mtd: format!("{}.mtd", src["mtd"].as_str().unwrap()),
                gx: vec![],
                textures: src["bindings"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(kind, b)| crate::flver::Texture {
                        kind: kind.clone(),
                        path: format!("{}.dds", b["texture"].as_str().unwrap()),
                        scale: serde_json::from_value(b["scale"].clone()).unwrap(),
                        ..Default::default()
                    })
                    .collect(),
            },
            Some(&definition),
            |_| true,
        );
        // the oracle predates Binding::available (the area's catalog holding the texture): the catalog above holds
        // every texture, so each binding is available; and Binding::address (the addressing the FLVER texture record
        // stores): the records built above state none, so each is unstated on both axes
        let mut expected = src["layered"].clone();
        for binding in expected["bindings"].as_object_mut().unwrap().values_mut() {
            binding["available"] = serde_json::Value::Bool(true);
            binding["address"] = serde_json::json!([null, null]);
        }
        assert_eq!(serde_json::to_value(&source.layered)?, expected);
        assert_eq!(source.mtd, "M[m11_01]_OBJ_Pot1");
        let mut images = BTreeMap::new();
        for (name, entry) in oracle["images"].as_object().unwrap() {
            let bytes = read(entry["path"].as_str().unwrap(), entry["sha256"].as_str().unwrap())?;
            let texture = primase_dds::read(&bytes)?;
            let rgba = primase_texcodec::Decoder::default().decode_rgba8(
                texture.format,
                texture.width,
                texture.height,
                &texture.levels[0],
            )?;
            images.insert(name.clone(), (texture.width, texture.height, rgba));
        }
        let prepared = Prepared::new(&shader, source.layered.as_ref().unwrap(), 0., |name, srgb| {
            let (w, h, rgba) = images.get(name).expect("complete jar source texture closure");
            Ok(Some(Arc::new(wrapped_texture(*w, *h, rgba, srgb)?)))
        })?;
        for inputs in [
            Inputs::default(),
            Inputs {
                weights_snow: [0.25, 0.5, 0.75, 0.25],
                tint_color: [0.8, 0.7, 0.6, 1.],
                detail: 1.,
            },
            Inputs {
                weights_snow: [1.; 4],
                detail: 0.,
                ..Inputs::default()
            },
        ] {
            let public = prepared.bake(32, prepared.period, inputs)?;
            let captured = prepared.bake_with_reflectance(
                primase_core::parallel::Threads::ONE,
                32,
                prepared.period,
                inputs,
            )?;
            for pixel in captured.reflectance.as_ref().unwrap() {
                assert_eq!(pixel.mask, 7);
                assert_eq!(pixel.bits, [literal; 3]);
            }
            assert_eq!(public.albedo, captured.image.albedo);
            assert_eq!(public.normal, captured.image.normal);
            assert_eq!(public.gloss, captured.image.gloss);
            assert_eq!(public.emissive, captured.image.emissive);
            assert_eq!(public.policies, captured.image.policies);
            assert_eq!(public.instructions, captured.image.instructions);
            assert_eq!(public.samples, captured.image.samples);
            let classification = Reflectance::classify(&source, 32, captured.reflectance.as_deref());
            assert_eq!(classification, Reflectance::Dielectric);
            let mut before = World::default();
            let mut after = World::default();
            let mut log = primase_core::pass::PassLog::default();
            let mode = crate::materials::alpha_mode(&source, None, false, &mut Default::default());
            let old = channel_material(
                &mut before,
                "jar",
                mode,
                public,
                Reflectance::Unsupported,
                &mut Default::default(),
            )?;
            let new = channel_material(&mut after, "jar", mode, captured.image, classification, &mut log)?;
            // Both are dielectric; tracing adds the program's F0 (0.03) as the specular factor, and no other value changes
            assert_eq!((old.metallic, new.metallic), (0., 0.));
            assert_eq!(old.specular, None);
            assert_eq!(new.specular, Reflectance::Dielectric.specular());
            assert_eq!(
                old,
                PbrMaterial {
                    specular: None,
                    ..new.clone()
                },
                "traced reflectance changes only the specular factor"
            );
            let pixels = |world: &World| {
                world
                    .iter::<Texture>()
                    .map(|(_, t)| (t.format, t.color_space, t.role, t.levels.clone()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(pixels(&before), pixels(&after));
            classification.record_material(&mut log);
            assert!(!log.approximations.contains_key("source dielectric F0 0.03 mapped to target F0 0.04"));
            assert!(!log.approximations.contains_key(pbr::UNKNOWN_METALLIC_LOSS));
        }
        println!(
            "jar M[Multiple]_1: 3 x 1024 executed RGB samples equal independent literal 0x{literal:08x}; all baked channels/policies and lowered images exact; traced dielectric, metallic 0"
        );
        Ok(())
    }
    #[test]
    fn equal_bake_inputs_share_images_but_keep_referenced_source_gx() {
        let mut world = World::default();
        let mut source = crate::materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                textures: vec![],
                gx: vec![],
            },
            None,
            |_| false,
        );
        source.layered = Some(crate::materials::LayeredMaterial {
            mtd: source.mtd.clone(),
            definition: crate::mtd::MaterialDefinition {
                shader: "M[Multiple]".into(),
                description: String::new(),
                parameters: vec![],
                textures: vec![],
                block_lengths: vec![],
            },
            bindings: BTreeMap::new(),
        });
        let original_a = world.spawn();
        source.gx = vec![crate::flver::GxItem {
            id: *b"GXMD",
            version: 123,
            data: vec![1, 2, 3, 4],
        }];
        world.insert(original_a, source.clone());
        let original_b = world.spawn();
        let mut other = source.clone();
        other.gx[0].data = vec![4, 3, 2, 1];
        world.insert(original_b, other.clone());
        assert_eq!(
            super::super::layers::bake_identity(&source).unwrap(),
            super::super::layers::bake_identity(&other).unwrap()
        );
        assert_ne!(
            serde_json::to_value(&source).unwrap(),
            serde_json::to_value(&other).unwrap()
        );
        // Prepared consumes these identical definition/binding values; GX must
        // neither alter the clustering pool nor cause another image execution.
        assert_eq!(
            serde_json::to_value(&source.layered).unwrap(),
            serde_json::to_value(&other.layered).unwrap()
        );
        // a base map tiled otherwise
        let mut changed_uv = other.clone();
        changed_uv.bindings.insert(
            "Texture2D_8_AlbedoMap".into(),
            crate::materials::Binding {
                texture: "base".into(),
                scale: [2., 1.],
                uv_number: 1,
                address: [None; 2],
                available: true,
            },
        );
        assert_ne!(
            super::super::layers::bake_identity(&source).unwrap(),
            super::super::layers::bake_identity(&changed_uv).unwrap()
        );
        let mut changed_alpha = other.clone();
        changed_alpha.cutoff = 0.9;
        assert_ne!(
            super::super::layers::bake_identity(&source).unwrap(),
            super::super::layers::bake_identity(&changed_alpha).unwrap()
        );
        let image = crate::layer_bake::Bake {
            size: 1,
            albedo: vec![10, 20, 30, 40],
            normal: vec![64, 128, 192, 255],
            gloss: Some(vec![0.25]),
            emissive: Some(vec![[4., 1., 0.]]),
            instructions: 0,
            samples: 0,
            policies: BTreeMap::new(),
        };
        let template = channel_material(
            &mut world,
            "bake_shared",
            crate::materials::alpha_mode(&source, None, false, &mut Default::default()),
            image,
            Reflectance::Unsupported,
            &mut Default::default(),
        )
        .unwrap();
        let pixels_before: Vec<_> = world
            .iter::<Texture>()
            .map(|(e, t)| (e, t.levels.clone()))
            .collect();
        // a's parts cull back faces, b's do not: the template's channels, each its own sidedness
        let a = source_variant(&mut world, original_a, &template, true, "a".into()).unwrap();
        let b = source_variant(&mut world, original_b, &template, false, "b".into()).unwrap();
        assert_ne!(a, b);
        let mesh = world.spawn();
        world.insert(
            mesh,
            Mesh::new(
                [a, b]
                    .into_iter()
                    .map(|material| Primitive {
                        topology: primase_core::Topology::Triangles,
                        material: Some(material),
                        streams: BTreeMap::new(),
                        indices: Some(vec![]),
                    })
                    .collect(),
            ),
        );
        for ((primitive, expected), two_sided) in world
            .get::<Mesh>(mesh)
            .unwrap()
            .primitives()
            .iter()
            .zip([source, other])
            .zip([false, true])
        {
            let entity = primitive.material.unwrap();
            assert_eq!(
                serde_json::to_value(world.get::<crate::materials::Material>(entity).unwrap()).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            let mut sided = template.clone();
            sided.double_sided = two_sided;
            assert_eq!(world.get::<PbrMaterial>(entity).unwrap(), &sided);
        }
        assert_eq!(
            pixels_before,
            world
                .iter::<Texture>()
                .map(|(e, t)| (e, t.levels.clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn assembled_channels_preserve_conventions_factors_and_absence() {
        let mut source = crate::materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                textures: vec![],
                gx: vec![],
            },
            None,
            |_| false,
        );
        source.class = "cutout".into();
        source.cutoff = 0.4;
        for emission in [None, Some([0.; 3]), Some([4., 1., 0.])] {
            let mut world = World::default();
            let mut log = primase_core::pass::PassLog::default();
            let image = crate::layer_bake::Bake {
                size: 1,
                albedo: vec![10, 20, 30, 40],
                normal: vec![64, 128, 192, 255],
                gloss: emission.map(|_| vec![0.25]),
                emissive: emission.map(|rgb| vec![rgb]),
                instructions: 0,
                samples: 0,
                policies: BTreeMap::new(),
            };
            let material = channel_material(
                &mut world,
                "bake_fixture",
                crate::materials::alpha_mode(&source, None, false, &mut Default::default()),
                image,
                Reflectance::Unsupported,
                &mut log,
            )
            .unwrap();
            Reflectance::Unsupported.record_material(&mut log);
            assert_eq!(material.metallic, 0.);
            assert_eq!(material.roughness, 1.);
            assert_eq!(material.alpha_mode, AlphaMode::Mask { cutoff: 0.4 });
            // the template states no sidedness: each variant takes its parts' face-set cull
            assert!(!material.double_sided);
            assert_eq!(log.approximations[pbr::UNKNOWN_METALLIC_LOSS], 1);
            assert!(material.occlusion_texture.is_none());
            let base = world
                .get::<Texture>(material.base_color_texture.unwrap().texture)
                .unwrap();
            assert_eq!(
                (base.role, base.color_space, base.normal),
                (TexRole::Color, ColorSpace::Srgb, None)
            );
            assert_eq!(base.levels[0], [10, 20, 30, 40]);
            let normal = world
                .get::<Texture>(material.normal_texture.unwrap().texture)
                .unwrap();
            assert_eq!(
                (normal.role, normal.color_space, normal.normal),
                (TexRole::Normal, ColorSpace::Linear, Some(normals::SOURCE))
            );
            assert_eq!(normal.levels[0], [64, 128, 192, 255]);
            match emission {
                None => {
                    assert!(material.metallic_roughness_texture.is_none());
                    assert!(material.emissive_texture.is_none());
                    assert_eq!(material.emissive, [0.; 3]);
                    assert_eq!(material.emissive_strength, 1.);
                }
                Some(rgb) => {
                    let orm = world
                        .get::<Texture>(material.metallic_roughness_texture.unwrap().texture)
                        .unwrap();
                    assert_eq!(
                        (orm.role, orm.color_space, orm.normal),
                        (TexRole::Mask, ColorSpace::Linear, None)
                    );
                    assert_eq!(orm.levels[0], [255, 134, 255, 255]);
                    let light = world
                        .get::<Texture>(material.emissive_texture.unwrap().texture)
                        .unwrap();
                    assert_eq!(
                        (light.role, light.color_space, light.normal),
                        (TexRole::Color, ColorSpace::Srgb, None)
                    );
                    assert_eq!(material.emissive, [1.; 3]);
                    assert_eq!(material.emissive_strength, rgb[0].max(1.));
                    assert_eq!(
                        light.levels[0],
                        if rgb[0] == 0. {
                            vec![0, 0, 0, 255]
                        } else {
                            vec![255, 137, 0, 255]
                        }
                    );
                }
            }
            for (entity, texture) in world.iter::<Texture>() {
                assert_eq!((texture.width, texture.height), (1, 1));
                assert!(world.get::<Name>(entity).unwrap().0.starts_with("bake_"));
            }
        }
    }

    #[test]
    fn a_stored_second_uv_set_that_differs_is_counted_against_the_untiled_first() {
        let part = |first: Vec<[f32; 2]>, second: Option<Vec<[f32; 2]>>| {
            let mut streams = BTreeMap::from([(Semantic::TexCoord(0), Stream::F32x2(first))]);
            streams.extend(second.map(|s| (Semantic::TexCoord(1), Stream::F32x2(s))));
            Primitive {
                topology: primase_core::Topology::Triangles,
                streams,
                indices: None,
                material: None,
            }
        };
        // tiled by (2, 4) by the geometry pass: the stored first set is uv0 * scale
        let tiled = vec![[1., 2.], [3., 8.], [5., 4.]];
        let same = vec![[0.5, 0.5], [1.5, 2.], [2.5, 1.]];
        assert_eq!(
            second_uv_differs(&part(tiled.clone(), Some(same.clone())), [2., 4.]),
            (0, 0)
        );
        let mut moved = same.clone();
        moved[2][1] += 0.01;
        moved[0][0] = 7.;
        assert_eq!(
            second_uv_differs(&part(tiled.clone(), Some(moved)), [2., 4.]),
            (1, 2)
        );
        // the first set taken as stored when the geometry pass did not tile
        assert_eq!(
            second_uv_differs(&part(tiled.clone(), Some(tiled.clone())), [1., 1.]),
            (0, 0)
        );
        // no stored second set: nothing is counted, and nothing is made up
        assert_eq!(second_uv_differs(&part(tiled, None), [2., 4.]), (0, 0));
    }

    #[test]
    fn split_preserves_single_level_order_and_remaps_every_stream() {
        let mut world = World::default();
        let materials = [world.spawn(), world.spawn()];
        let source = Primitive {
            topology: primase_core::Topology::Triangles,
            streams: BTreeMap::from([
                (Semantic::Position, Stream::F32x3(vec![[0.; 3]; 5])),
                (Semantic::TexCoord(0), Stream::F32x2(vec![[8., 12.]; 5])),
                (
                    Semantic::Color(1),
                    Stream::F32x4((0..5).map(|i| [i as f32; 4]).collect()),
                ),
            ]),
            indices: Some(vec![2, 0, 1, 1, 3, 2]),
            material: None,
        };
        let one = split(&source, &[0, 0], &materials[..1], [2., 3.], [2., 2.]).unwrap();
        // one level: the source's vertices as they are, nothing for a per-vertex component to follow
        assert_eq!(one[0].1, None);
        let one: Vec<Primitive> = one.into_iter().map(|(p, _)| p).collect();
        assert_eq!(one[0].indices, source.indices);
        assert_eq!(
            one[0].streams[&Semantic::Color(1)],
            source.streams[&Semantic::Color(1)]
        );
        assert_eq!(
            one[0].streams[&Semantic::TexCoord(0)],
            Stream::F32x2(vec![[2., 2.]; 5])
        );
        // Other primitives can use level zero while this one uses only level one.
        // Preserve its unused vertex and authored order, not just its triangles.
        let nonzero: Vec<Primitive> = split(&source, &[1, 1], &materials, [2., 3.], [2., 2.])
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(nonzero.len(), 1);
        assert_eq!(nonzero[0].material, Some(materials[1]));
        assert_eq!(nonzero[0].indices, source.indices);
        assert_eq!(nonzero[0].streams, one[0].streams);
        let two = split(&source, &[1, 0], &materials, [2., 3.], [2., 2.]).unwrap();
        // two levels: each part's vertices compacted, their source vertices stated
        assert_eq!(two[0].1, Some(vec![1, 3, 2]));
        assert_eq!(two[1].1, Some(vec![2, 0, 1]));
        let two: Vec<Primitive> = two.into_iter().map(|(p, _)| p).collect();
        assert_eq!(two.len(), 2);
        for (level, expected) in [[1., 3., 2.], [2., 0., 1.]].iter().enumerate() {
            assert_eq!(two[level].material, Some(materials[level]));
            assert_eq!(two[level].indices, Some(vec![0, 1, 2]));
            assert_eq!(
                two[level].streams[&Semantic::Color(1)],
                Stream::F32x4(expected.iter().map(|i| [*i; 4]).collect())
            );
        }
        assert!(split(&source, &[0], &materials, [1.; 2], [1.; 2]).is_err());
        assert!(split(&source, &[0, 0], &materials, [0., 1.], [1.; 2]).is_err());
    }
}
