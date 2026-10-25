use super::settings::{PixelCap, Seconds};
use super::*;
use crate::materials::{self, LayeredMaterial, Material};
use crate::sekiro::source::{FaceCull, LayerInputs, PrototypeSource};
use primase_core::pass::{Pass, PassLog, PassRegistry};
use primase_core::{
    DrawOrder, Entity, Instance, LodForm, Lods, Mesh, Primitive, Semantic, Stream, Topology, Transform, World,
};
use std::collections::BTreeMap;

/// The water colour the tests state (the earlier skate exporter's).
const WATER: primase_image::Srgba8 = primase_image::Srgba8([48, 98, 122, 190]);

/// The distant backdrop's placements are dropped by name, so the conservation check (enforced) sees every
/// placement it removes declared; its prototypes, which hold no core content yet, declare nothing.
#[test]
fn far_placements_are_declared_drops() {
    use primase_core::ledger::{Conservation, Measure};
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    let mut world = World::new();
    let far = world.spawn();
    world.insert(
        far,
        PrototypeSource {
            id: "m9000000".into(),
            far: true,
        },
    );
    let near = world.spawn();
    world.insert(
        near,
        PrototypeSource {
            id: "m0000000".into(),
            far: false,
        },
    );
    for prototype in [far, far, near] {
        let p = world.spawn();
        world.insert(
            p,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(p, Transform::IDENTITY);
    }
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(DropFromSoftFar)],
            },
            &mut report,
        )
        .unwrap();
    assert_eq!(report.steps[0].log.declared(Measure::Placements), 2);
    assert_eq!(world.iter::<Instance>().count(), 1);
    assert!(!world.is_alive(far));
}

/// A layered source material (no texture bound), `skip` its skip class.
fn layered(skip: Option<materials::SkipClass>) -> Material {
    let mut m = materials::resolve(
        &crate::flver::Material {
            name: "fixture".into(),
            mtd: "fixture.mtd".into(),
            gx: vec![],
            textures: vec![],
        },
        None,
        |_| false,
    );
    m.skip = skip;
    m.layered = Some(LayeredMaterial {
        mtd: m.mtd.clone(),
        definition: crate::mtd::MaterialDefinition {
            shader: "M[Multiple].spx".into(),
            description: String::new(),
            parameters: vec![],
            textures: vec![],
            block_lengths: vec![],
        },
        bindings: BTreeMap::new(),
    });
    m
}

/// A triangle drawn with `material`, carrying layer input member 1.
fn primitive(material: Entity) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
            ),
            (Semantic::Color(1), Stream::U8x4(vec![[255, 0, 0, 0]; 3])),
        ]),
        indices: Some(vec![0, 1, 2]),
        material: Some(material),
    }
}

fn mesh_with_inputs(world: &mut World, material: Entity) -> Entity {
    let e = world.spawn();
    world.insert(e, Mesh::new(vec![primitive(material)]));
    world.insert(
        e,
        LayerInputs {
            primitives: vec![vec![Semantic::Color(1)]],
        },
    );
    e
}

#[test]
fn the_bake_refuses_two_placed_prototypes_of_one_source_id() {
    // the source id orders the bake's inputs; two prototypes sharing one would leave that order to their entities
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, layered(None));
    for _ in 0..2 {
        let prototype = mesh_with_inputs(&mut world, m);
        world.insert(
            prototype,
            PrototypeSource {
                id: "obj/o000001".into(),
                far: false,
            },
        );
        let placement = world.spawn();
        world.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(placement, Transform::IDENTITY);
    }
    let before = world.clone();
    let bake = BakeFromSoftLayers::new(PixelCap::new(64).unwrap(), Seconds::new(0.).unwrap());
    let err = bake.run(&mut world, &mut PassLog::default()).unwrap_err();
    assert!(
        err.contains("two placed prototypes with source id obj/o000001"),
        "{err}"
    );
    assert_eq!(
        world.iter::<LayerInputs>().count(),
        before.iter::<LayerInputs>().count()
    );
}

#[test]
fn the_bake_refuses_level_of_detail_forms_before_changing_anything_and_leaves_unplaced_inputs() {
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, layered(None));
    let prototype = mesh_with_inputs(&mut world, m);
    world.insert(
        prototype,
        PrototypeSource {
            id: "obj/o000001".into(),
            far: false,
        },
    );
    let form = mesh_with_inputs(&mut world, m);
    world.insert(
        prototype,
        Lods {
            detail: 0,
            forms: vec![LodForm { detail: 1, form }],
            default: None,
        },
    );
    let placement = world.spawn();
    world.insert(
        placement,
        Instance {
            prototype,
            variant: 0,
        },
    );
    world.insert(placement, Transform::IDENTITY);
    let before = world.clone();
    let bake = BakeFromSoftLayers::new(PixelCap::new(64).unwrap(), Seconds::new(0.).unwrap());
    let err = bake.run(&mut world, &mut PassLog::default()).unwrap_err();
    assert!(err.contains("select-lod"), "{err}");
    assert_eq!(
        world.iter::<LayerInputs>().count(),
        before.iter::<LayerInputs>().count()
    );
    assert!(
        world.get::<Mesh>(form).unwrap().primitives()[0]
            .streams
            .contains_key(&Semantic::Color(1))
    );

    // an unplaced prototype's inputs are not the bake's: they stay, streams and all
    let mut world = World::new();
    let m = world.spawn();
    world.insert(m, layered(None));
    let unplaced = mesh_with_inputs(&mut world, m);
    world.insert(
        unplaced,
        PrototypeSource {
            id: "obj/o000002".into(),
            far: false,
        },
    );
    let mut log = PassLog::default();
    bake.run(&mut world, &mut log).unwrap();
    assert_eq!(log.counts["unplaced meshes whose layer inputs stay"], 1);
    assert_eq!(log.counts["layer input streams consumed"], 0);
    assert!(world.has::<LayerInputs>(unplaced));
    assert!(
        world.get::<Mesh>(unplaced).unwrap().primitives()[0]
            .streams
            .contains_key(&Semantic::Color(1))
    );
}

#[test]
fn a_preview_drops_only_the_inputs_of_the_materials_it_previewed() {
    let mut world = World::new();
    let previewed = world.spawn();
    world.insert(previewed, layered(None));
    let skipped = world.spawn();
    world.insert(skipped, layered(Some(materials::SkipClass::Sky)));
    let e = world.spawn();
    let mut first = primitive(previewed);
    first.streams.insert(
        Semantic::TexCoord(1),
        Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
    );
    world.insert(e, Mesh::new(vec![first, primitive(skipped)]));
    world.insert(
        e,
        FaceCull {
            primitives: vec![true; 2],
        },
    );
    world.insert(
        e,
        LayerInputs {
            primitives: vec![
                vec![Semantic::Color(1), Semantic::TexCoord(1)],
                vec![Semantic::Color(1)],
            ],
        },
    );
    let mut log = PassLog::default();
    LowerFromSoftMaterials::new(Some(super::pbr::Layers::Preview), WATER)
        .run(&mut world, &mut log)
        .unwrap();
    assert_eq!(
        log.approximations["layer input streams dropped with layered materials lowered from previews"],
        2
    );
    let mesh = world.get::<Mesh>(e).unwrap();
    assert!(!mesh.primitives()[0].streams.contains_key(&Semantic::Color(1)));
    assert!(!mesh.primitives()[0].streams.contains_key(&Semantic::TexCoord(1)));
    // the skipped class was not lowered: its inputs are still the source's
    assert!(mesh.primitives()[1].streams.contains_key(&Semantic::Color(1)));
    assert_eq!(
        world.get::<LayerInputs>(e).unwrap().primitives,
        [vec![], vec![Semantic::Color(1)]]
    );
}

#[test]
fn a_lowered_material_carries_what_its_source_class_states_about_its_surfaces() {
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    use primase_core::{DrawLayer, MaterialUsage};
    // the reader's class from each shader family, as resolve gives it
    let source = |shader: &str, parameters: Vec<crate::mtd::Parameter>| {
        materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                gx: vec![],
                textures: vec![],
            },
            Some(&crate::mtd::MaterialDefinition {
                shader: shader.into(),
                description: String::new(),
                parameters,
                textures: vec![],
                block_lengths: vec![],
            }),
            |_| false,
        )
    };
    let int = |name: &str, v: i32| crate::mtd::Parameter {
        name: name.into(),
        value: crate::mtd::Value::Int(vec![v]),
    };
    let mut world = World::new();
    let mut drawn = Vec::new();
    for (shader, parameters, class) in [
        ("GXFlver_Tessellation_Water.spx", vec![], "water"),
        ("M[Translucency]_Leaves.spx", vec![], "leaves"),
        ("GXFlver_MeshDecal.spx", vec![], "decal"),
        ("M[ARSN].spx", vec![int("g_BlendMode", 2)], "alpha"),
        ("M[ARSN].spx", vec![int("g_AlphaRef", 128)], "cutout"),
        ("M[ARSN].spx", vec![], "opaque"),
    ] {
        let m = source(shader, parameters);
        assert_eq!(m.class, class, "{shader}");
        let e = world.spawn();
        world.insert(e, m);
        drawn.push(e);
    }
    let mesh = world.spawn();
    world.insert(mesh, Mesh::new(drawn.iter().map(|&m| primitive(m)).collect()));
    world.insert(
        mesh,
        FaceCull {
            primitives: vec![true; drawn.len()],
        },
    );
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(LowerFromSoftMaterials::new(None, WATER))],
            },
            &mut report,
        )
        .unwrap();
    let [water, leaves, decal, alpha, cutout, opaque] = drawn[..] else {
        unreachable!()
    };
    assert_eq!(world.get::<MaterialUsage>(water), Some(&MaterialUsage::Water));
    assert_eq!(world.get::<MaterialUsage>(leaves), Some(&MaterialUsage::Foliage));
    assert_eq!(
        world.get::<DrawOrder>(decal),
        Some(&DrawOrder::conventional(
            DrawLayer::Decal,
            super::pbr::DECAL_QUEUE
        ))
    );
    assert!(!world.get::<DrawOrder>(decal).unwrap().depth_write);
    // the class states nothing more: no usage on a decal, no draw order on water or leaves (their alpha mode's)
    for e in [decal, alpha, cutout, opaque] {
        assert!(world.get::<MaterialUsage>(e).is_none());
    }
    for e in [water, leaves, alpha, cutout, opaque] {
        assert!(world.get::<DrawOrder>(e).is_none());
    }
    assert_eq!(DrawOrder::of(&world, leaves).layer, DrawLayer::Cutout);
    assert_eq!(DrawOrder::of(&world, water).layer, DrawLayer::Transparent);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["materials of class water given their usage"], 1);
    assert_eq!(log.counts["materials of class leaves given their usage"], 1);
    assert_eq!(log.counts["materials of class decal given their draw order"], 1);
}

/// Sekiro's water is a family by name, not only the tessellated sea: the river and waterfall shaders the layer bake's
/// scroll policy names (`M[ARSN]_Water_3normaltet`, `M[Flow]_Water`) are water too, and a shader whose name only
/// resembles one is not.
#[test]
fn sekiros_rivers_and_waterfalls_are_water_by_their_shader_family() {
    let class = |shader: &str| {
        materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                gx: vec![],
                textures: vec![],
            },
            Some(&crate::mtd::MaterialDefinition {
                shader: shader.into(),
                description: String::new(),
                parameters: vec![],
                textures: vec![],
                block_lengths: vec![],
            }),
            |_| false,
        )
        .class
    };
    for shader in [
        "GXFlver_Tessellation_Water.spx",
        "M[Interaction]_Tessellation_Water.spx",
        "M[ARSN]_Water_3normaltet.spx",
        "M[Flow]_Water.spx",
    ] {
        assert_eq!(class(shader), "water", "{shader}");
    }
    for shader in ["M[ARSN].spx", "M[AMSN].spx", "GXFlver_MeshDecal.spx"] {
        assert_ne!(class(shader), "water", "{shader}");
    }
}

/// `g_BlendMode` 4 is a light volume in Sekiro's table and, in Elden Ring's, a surface drawn added to what is behind it
/// (c2200's emissive body and wing overlays); lowered, it glows with its own colour and is blended, counted.
#[test]
fn an_additive_blend_is_a_light_volume_in_sekiro_and_an_additive_surface_in_elden_ring() {
    use crate::Game;
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    use primase_core::pbr::AlphaMode;
    let resolve = |game: Game, shader: &str, blend: i32| {
        materials::resolve_definition(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                gx: vec![],
                textures: vec![],
            },
            Some(&materials::Definition::Mtd(crate::mtd::MaterialDefinition {
                shader: shader.into(),
                description: String::new(),
                parameters: vec![crate::mtd::Parameter {
                    name: "g_BlendMode".into(),
                    value: crate::mtd::Value::Int(vec![blend]),
                }],
                textures: vec![],
                block_lengths: vec![],
            })),
            game,
            |_| false,
        )
    };
    let sekiro = resolve(Game::Sekiro, "M[ARSN].spx", 4);
    assert_eq!(sekiro.skip, Some(materials::SkipClass::LightVolume));
    let elden = resolve(Game::EldenRing, "C[c2200]_Body.spx", 4);
    assert_eq!((elden.skip, elden.class.as_str()), (None, "additive"));
    // the light shafts of its table stay volumes, and a blend mode other than 4 is as before
    let shaft = resolve(Game::EldenRing, "M[LightShaft].spx", 4);
    assert_eq!(shaft.skip, Some(materials::SkipClass::LightVolume));
    assert_eq!(resolve(Game::EldenRing, "C[c2200]_Body.spx", 2).class, "alpha");

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, elden);
    let mesh = world.spawn();
    world.insert(mesh, Mesh::new(vec![primitive(e)]));
    world.insert(
        mesh,
        FaceCull {
            primitives: vec![true],
        },
    );
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(LowerFromSoftMaterials::new(None, WATER))],
            },
            &mut report,
        )
        .unwrap();
    let pbr = world.get::<primase_core::pbr::PbrMaterial>(e).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Blend);
    // no texture was bound in this fixture: nothing to glow with
    assert_eq!(pbr.emissive, [0.; 3]);
    assert!(
        report.steps[0].log.approximations["additive blend (g_BlendMode 4) lowered as the base colour's emission, alpha blended"]
            == 1
    );
}

#[test]
fn a_lowered_material_is_as_sided_as_the_face_sets_drawing_it_and_split_when_they_differ() {
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    use primase_core::pbr::PbrMaterial;
    use primase_core::{Name, Semantic};
    let mut world = World::new();
    let material = |world: &mut World, name: &str| {
        let m = world.spawn();
        // named as the reader names it, by its MTD, which the lowering keeps without a preview base map
        let mut source = layered(None);
        source.layered = None;
        source.mtd = name.into();
        world.insert(m, source);
        world.insert(m, primase_core::Name(name.into()));
        m
    };
    let culled = material(&mut world, "culled");
    let open = material(&mut world, "open");
    let both = material(&mut world, "both");
    // two meshes: the first draws `both` culled and `open`, the second `culled` and `both` not culled
    let first = world.spawn();
    world.insert(first, Mesh::new(vec![primitive(both), primitive(open)]));
    world.insert(
        first,
        FaceCull {
            primitives: vec![true, false],
        },
    );
    let second = world.spawn();
    world.insert(second, Mesh::new(vec![primitive(culled), primitive(both)]));
    world.insert(
        second,
        FaceCull {
            primitives: vec![true, false],
        },
    );
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(LowerFromSoftMaterials::new(None, WATER))],
            },
            &mut report,
        )
        .unwrap();
    let sided = |e: Entity| world.get::<PbrMaterial>(e).unwrap().double_sided;
    assert!(!sided(culled));
    assert!(sided(open));
    assert!(!sided(both));
    // the primitive drawing `both` without culling draws its two-sided copy, which keeps its source
    let copy = world.get::<Mesh>(second).unwrap().primitives()[1]
        .material
        .unwrap();
    assert_ne!(copy, both);
    assert!(sided(copy));
    assert_eq!(world.get::<Name>(copy).unwrap().0, "both_two_sided");
    assert_eq!(
        serde_json::to_value(world.get::<Material>(copy).unwrap()).unwrap(),
        serde_json::to_value(world.get::<Material>(both).unwrap()).unwrap()
    );
    assert_eq!(
        world.get::<Mesh>(first).unwrap().primitives()[0].material,
        Some(both)
    );
    assert!(
        world.get::<Mesh>(second).unwrap().primitives()[1]
            .streams
            .contains_key(&Semantic::Position)
    );
    let log = &report.steps[0].log;
    assert_eq!(log.counts["materials split by face-set cull"], 1);
    assert_eq!(
        log.counts["materials single-sided: their face sets cull back faces"],
        1
    );
    assert_eq!(log.counts["materials two-sided: their face sets do not cull"], 1);

    // a mesh drawing a lowered material without its face sets' cull is refused: sidedness is never a default
    let mut world = World::new();
    let m = material(&mut world, "unknown");
    let mesh = world.spawn();
    world.insert(mesh, Mesh::new(vec![primitive(m)]));
    let err = LowerFromSoftMaterials::new(None, WATER)
        .run(&mut world, &mut PassLog::default())
        .unwrap_err();
    assert!(err.contains("without fromsoft.face_cull"), "{err}");
}

#[test]
fn fromsoft_textures_lose_their_transforms_and_other_materials_keep_theirs() {
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
    let mut world = World::new();
    let t = world.spawn();
    let scaled = |s: f32| {
        let mut r = TextureRef::new(t);
        r.transform = Some(UvTransform {
            scale: [s, s],
            ..UvTransform::IDENTITY
        });
        r
    };
    let material = PbrMaterial {
        base_color_texture: Some(TextureRef::new(t)),
        normal_texture: Some(scaled(4.)),
        metallic_roughness_texture: Some(scaled(1.)),
        ..PbrMaterial::default()
    };
    let fromsoft = world.spawn();
    world.insert(fromsoft, material.clone());
    world.insert(fromsoft, layered(None));
    let other = world.spawn();
    world.insert(other, material.clone());
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(DropFromSoftTextureTransforms::new())],
            },
            &mut report,
        )
        .unwrap();
    assert!(
        world
            .get::<PbrMaterial>(fromsoft)
            .unwrap()
            .textures()
            .all(|r| r.transform.is_none())
    );
    // another source's material is not this pass's
    assert_eq!(world.get::<PbrMaterial>(other), Some(&material));
    let log = &report.steps[0].log;
    // the identity transform is no approximation
    assert_eq!(log.approximations.len(), 1, "{:?}", log.approximations);
    assert_eq!(
        log.approximations["normal textures sampled at the albedo's tiling, their own transform left out"],
        1
    );
}

/// A read region that places no model with collision files has no `fromsoft.object_collision`: the object collision
/// lowering has nothing to do there, and its step is recorded as skipped, not failed.
#[test]
fn object_collision_is_skipped_where_no_placed_model_has_collision() {
    use primase_core::pass::{Options, PassManager, Pipeline, RunReport};
    let options: Options = [
        ("resolution", "high-else-low"),
        ("convex", "rounded:0.01"),
        ("leave-out", ""),
        ("leave-out-motion", ""),
        ("convex-surface", "default"),
        ("reach", "none"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let pass = super::object_collision::LOWER_FROMSOFT_OBJECT_COLLISION
        .instance(&options)
        .unwrap();
    let mut world = World::new();
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run_part(&mut world, &Pipeline { passes: vec![pass] }, &mut report)
        .unwrap();
    assert_eq!(report.steps[0].skipped, ["fromsoft.object_collision"]);
}

/// A world with no FLVER model (an area of collision alone, or whose only models were the distant backdrop
/// `drop-fromsoft-far` left out) has no static mesh to pose: the rigid skinning is recorded skipped, not failed.
#[test]
fn rigid_skinning_is_skipped_where_the_world_holds_no_model() {
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    let pass = crate::rigid::FROMSOFT_APPLY_RIGID_SKINNING
        .instance(&Default::default())
        .unwrap();
    let mut world = World::new();
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run_part(&mut world, &Pipeline { passes: vec![pass] }, &mut report)
        .unwrap();
    assert_eq!(report.steps[0].skipped, ["fromsoft.flver"]);
}

/// A material whose program the reader does not hold has no `core.shader_binding`, and a read none of whose materials
/// has one (a weapon part) has nothing to bake: the bake runs, counts each material it found unbound and changes none.
#[test]
fn the_material_bake_runs_where_no_material_has_a_binding() {
    use primase_core::pass::{PassManager, Pipeline, RunReport};
    use primase_core::pbr::PbrMaterial;
    let pass = super::bake_materials::BAKE_FROMSOFT_MATERIALS
        .instance(
            &[("max-size", "4"), ("sample-time", "0")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        )
        .unwrap();
    let mut world = World::new();
    let material = world.spawn();
    let mut source = layered(None);
    source.layered = None;
    world.insert(material, source);
    world.insert(material, PbrMaterial::default());
    let mesh = world.spawn();
    world.insert(mesh, Mesh::new(vec![primitive(material)]));
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run_part(&mut world, &Pipeline { passes: vec![pass] }, &mut report)
        .expect("a bake with no binding anywhere is not refused");
    assert!(report.steps[0].skipped.is_empty(), "it ran");
    assert!(world.get::<PbrMaterial>(material).is_some());
}

/// The crate registers exactly these passes by their declarations: those with required options are made only with
/// them, and every one says what it takes.
#[test]
fn the_fromsoft_passes_register_by_name() {
    let mut reg = PassRegistry::new();
    super::register(&mut reg);
    let names = [
        "bake-fromsoft-layers",
        "bake-fromsoft-materials",
        "derive-fromsoft-detail",
        "drop-fromsoft-far",
        "drop-fromsoft-texture-transforms",
        "fromsoft-apply-rigid-skinning",
        "lower-fromsoft-collision",
        "lower-fromsoft-environment",
        "lower-fromsoft-extras",
        "lower-fromsoft-geometry",
        "lower-fromsoft-lights",
        "lower-fromsoft-materials",
        "lower-fromsoft-object-collision",
        "lower-fromsoft-part-collision",
        "raise-fromsoft-display-masks",
        "resolve-fromsoft-shaders",
        "select-fromsoft-outfit",
    ];
    assert_eq!(format!("{reg:?}"), format!("{names:?}"));
    let free = [
        "derive-fromsoft-detail",
        "drop-fromsoft-far",
        "drop-fromsoft-texture-transforms",
        "fromsoft-apply-rigid-skinning",
        "lower-fromsoft-extras",
        "raise-fromsoft-display-masks",
    ];
    for name in names {
        assert_eq!(reg.requires_options(name), !free.contains(&name), "{name}");
        assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
    }
}

/// A two-bone model (a root and an arm raised 1 m and turned a quarter about X) with one static mesh whose vertices
/// name a bone by NORMAL.w, in a layout whose normal member (`kind`) does or does not carry a W, and whose node is
/// `node`.
fn static_arm(normal_kind: u32, palette: Vec<u32>, normal_w: i32, node: i32) -> crate::flver::Model {
    use crate::flver::{Bone, FaceSet, Member, Mesh as FlverMesh, Model, Vertex};
    let member = |semantic, kind, offset| Member {
        unknown: 0,
        stored: true,
        offset,
        kind,
        semantic,
        index: 0,
    };
    let vertex = |position| Vertex {
        position,
        normal: [0., 1., 0.],
        normal_w,
        ..Default::default()
    };
    Model {
        dummies: vec![],
        version: 0x2001a,
        speedtree: false,
        layouts: vec![vec![member(0, 2, 0), member(3, normal_kind, 12)]],
        materials: vec![crate::flver::Material {
            name: "fixture".into(),
            mtd: "fixture.mtd".into(),
            gx: vec![],
            textures: vec![],
        }],
        bones: vec![
            Bone {
                name: "root".into(),
                translation: [0.; 3],
                rotation: [0.; 3],
                scale: [1.; 3],
                parent: -1,
            },
            Bone {
                name: "arm".into(),
                translation: [0., 1., 0.],
                rotation: [std::f32::consts::FRAC_PI_2, 0., 0.],
                scale: [1.; 3],
                parent: 0,
            },
        ],
        meshes: vec![FlverMesh {
            dynamic: false,
            material: 0,
            node,
            palette,
            layouts: vec![0],
            vertices: vec![vertex([0., 0., 0.]), vertex([1., 0., 0.]), vertex([0., 0., 1.])],
            faces: vec![FaceSet {
                flags: 0,
                strip: false,
                cull: true,
                indices: vec![0, 1, 2],
            }],
        }],
    }
}

/// `model` as one prototype of a world, with its material and, when `bound`, its skeleton (as the character reader
/// makes it).
fn arm_world(model: crate::flver::Model, bound: bool) -> (World, Entity) {
    use crate::components::models::{SkeletonOf, SourceModel};
    let mut world = World::new();
    let material = world.spawn();
    world.insert(material, materials::resolve(&model.materials[0], None, |_| false));
    let prototype = world.spawn();
    world.insert(
        prototype,
        PrototypeSource {
            id: "c9999".into(),
            far: false,
        },
    );
    world.insert(prototype, crate::sekiro::source::MaterialSlots(vec![material]));
    if bound {
        let skeleton = world.spawn();
        world.insert(skeleton, crate::pose::skeleton(&model));
        world.insert(prototype, SkeletonOf(skeleton));
    }
    world.insert(prototype, SourceModel(model));
    (world, prototype)
}

fn lower_arm(pose: Pose) -> LowerFromSoftGeometry {
    LowerFromSoftGeometry {
        pose,
        uv_tiling: Tiling::Keep,
        winding: Winding::Stored,
        degenerates: Degenerates::Keep,
        leave_out: Default::default(),
        blocker_cards: Cards::Draw,
        speedtree_frame: SpeedTreeFrame::Program,
    }
}

fn positions(world: &World, prototype: Entity) -> Vec<[f32; 3]> {
    match &world.get::<Mesh>(prototype).expect("lowered").primitives()[0].streams[&Semantic::Position] {
        Stream::F32x3(p) => p.clone(),
        other => panic!("positions {other:?}"),
    }
}

/// A static mesh of a model bound to a skeleton (Sekiro's c9001, wp_a_0711) lowers with its skin: posed into the
/// bind pose where `fromsoft-apply-rigid-skinning` and `pose=rigid` put it, every vertex following the bone its
/// NORMAL.w names at weight 1, so the skin moves it with that bone.
#[test]
fn a_static_mesh_of_a_bound_model_follows_its_bone() {
    let (mut stored, prototype) = arm_world(static_arm(0x13, vec![], 1, -1), true);
    let mut log = PassLog::default();
    lower_arm(Pose::Stored).run(&mut stored, &mut log).unwrap();
    let primitive = &stored.get::<Mesh>(prototype).unwrap().primitives()[0];
    assert_eq!(
        primitive.streams[&Semantic::Joints(0)],
        Stream::U8x4(vec![[1, 0, 0, 0]; 3])
    );
    assert_eq!(
        primitive.streams[&Semantic::Weights(0)],
        Stream::F32x4(vec![[1., 0., 0., 0.]; 3])
    );
    assert_eq!(
        log.counts["static meshes of a bound model bound rigidly to the bone their vertices' NORMAL.w names"],
        1
    );

    let (mut rigid, rigid_prototype) = arm_world(static_arm(0x13, vec![], 1, -1), false);
    let mut log = PassLog::default();
    crate::passes::rigid::ApplyRigidSkinning
        .run(&mut rigid, &mut log)
        .unwrap();
    lower_arm(Pose::Rigid).run(&mut rigid, &mut log).unwrap();
    assert_eq!(positions(&stored, prototype), positions(&rigid, rigid_prototype));
    // the arm's rest moved the vertices: they are not as stored
    assert_ne!(positions(&stored, prototype)[1], [1., 0., 0.]);
}

/// A static mesh whose layout stores no normal W (Float3) follows its node: its NORMAL.w (read as 0) is not its bone.
/// Bound to a skeleton it moves with the arm, its node, as the W-carrying mesh naming the arm does; posed rigidly it
/// stands where that one does.
#[test]
fn a_static_mesh_without_a_normal_w_follows_its_node() {
    let (mut named, named_prototype) = arm_world(static_arm(0x13, vec![], 1, -1), true);
    lower_arm(Pose::Stored)
        .run(&mut named, &mut PassLog::default())
        .unwrap();
    let (mut node, prototype) = arm_world(static_arm(2, vec![], 0, 1), true);
    let mut log = PassLog::default();
    lower_arm(Pose::Stored).run(&mut node, &mut log).unwrap();
    let primitive = &node.get::<Mesh>(prototype).unwrap().primitives()[0];
    assert_eq!(
        primitive.streams[&Semantic::Joints(0)],
        Stream::U8x4(vec![[1, 0, 0, 0]; 3])
    );
    assert_eq!(positions(&node, prototype), positions(&named, named_prototype));
    assert_eq!(
        log.counts["static meshes of a bound model bound rigidly to their node (no normal W)"],
        1
    );

    let (mut rigid, rigid_prototype) = arm_world(static_arm(2, vec![], 0, 1), false);
    let mut log = PassLog::default();
    crate::passes::rigid::ApplyRigidSkinning
        .run(&mut rigid, &mut log)
        .unwrap();
    assert_eq!(log.counts["static meshes following their node (no normal W)"], 1);
    lower_arm(Pose::Rigid).run(&mut rigid, &mut log).unwrap();
    assert_eq!(
        positions(&rigid, rigid_prototype),
        positions(&named, named_prototype)
    );
}

/// What is not established stays refused, by name: a static mesh with a palette, one with no normal W whose node names
/// no bone (a skin needs a joint) and a NORMAL.w that names no bone.
#[test]
fn a_static_mesh_of_a_bound_model_is_refused_where_its_bone_is_not_stated() {
    let no_node =
        "mesh 0 is static, stores no normal W and its NodeIndex -1 names no bone of the 2: a skin needs one";
    for (model, says) in [
        (static_arm(0x13, vec![1], 0, -1), "static with a bone palette"),
        (static_arm(2, vec![], 0, -1), no_node),
        (
            static_arm(0x13, vec![], 7, -1),
            "NORMAL.w 7 names no bone of the 2",
        ),
    ] {
        let (mut world, _) = arm_world(model, true);
        let e = lower_arm(Pose::Stored)
            .run(&mut world, &mut PassLog::default())
            .unwrap_err();
        assert!(e.contains(says), "{e}");
    }
}

/// `fromsoft-apply-rigid-skinning` leaves a static mesh with no normal W whose node names no bone as stored (Smithbox's
/// observed rule; an Elden Ring area has one), as an approximation on the model, rather than posing it by bone 0.
#[test]
fn rigid_skinning_leaves_a_static_mesh_whose_node_names_no_bone_as_stored() {
    let model = static_arm(2, vec![], 0, 5);
    let stored: Vec<[f32; 3]> = model.meshes[0].vertices.iter().map(|v| v.position).collect();
    let (mut world, prototype) = arm_world(model, false);
    let mut log = PassLog::default();
    crate::passes::rigid::ApplyRigidSkinning
        .run(&mut world, &mut log)
        .unwrap();
    let rigid = &world
        .get::<crate::components::models::RigidModel>(prototype)
        .unwrap()
        .0;
    let posed: Vec<[f32; 3]> = rigid.meshes[0].vertices.iter().map(|v| v.position).collect();
    assert_eq!(posed, stored);
    let reason = crate::passes::rigid::UNBOUND_NODE;
    assert_eq!(log.approximations.get(reason), Some(&1));
    assert_eq!(
        log.approximated_on.get(reason),
        Some(&std::collections::BTreeSet::from([prototype]))
    );
}
