#![allow(clippy::unwrap_used)]
//! Sekiro's binding rules (`resolve`) and the `resolve-fromsoft-shaders` pass, on one layered program built to
//! Sekiro's naming (tests/fixtures/layered.hlsl, compiled with fxc): the constants, texture slots and outputs a
//! layered material's binding holds, worked out from the HLSL, fxc's listing of its buffers and the material.
use primase_core::shader::{
    AddressMode, ConstantRole, FrameParameter, InputRole, LaneStream, NormalEncoding, NormalSpace,
    ObjectParameter, OutputRole, ResourceId, SamplerState, Shader, ShaderBinding, ShaderFormat, Stage,
    TextureSource, ViewParameter,
};
use primase_core::{ColorSpace, Entity, Name, TexFormat, TexRole, Texture, World};
use primase_fromsoft::layer_bake::resolve::{SECOND_COLOUR, Unbound, resolve};
use primase_fromsoft::materials::{Binding, LayeredMaterial, Material};
use primase_fromsoft::mtd::{self, MaterialDefinition, Parameter, Value};
use primase_fromsoft::sekiro::source::{
    MaterialShader, ShaderBundle, ShaderProgram, ShaderPrograms, ShaderStage, TexturePayloads,
};
use primase_image::sampling::Address;
use std::collections::BTreeMap;
use std::sync::Arc;
mod common;

/// The addressing the albedo's FLVER record stores: repeat along u, mirrored repeat along v.
const GRASS_ADDRESS: [Option<AddressMode>; 2] = [Some(AddressMode::Repeat), Some(AddressMode::Mirror)];
const PROGRAM: &[u8] = include_bytes!("fixtures/layered.dxbc");
const VERTEX_PROGRAM: &[u8] = include_bytes!("fixtures/layered_vs.dxbc");

fn texture_kind(kind: &str) -> mtd::Texture {
    mtd::Texture {
        kind: kind.into(),
        uv_number: 0,
        shader_data_index: 0,
        path: String::new(),
        extra: Vec::new(),
    }
}
fn bound(stem: &str, scale: [f32; 2], address: [Option<AddressMode>; 2]) -> Binding {
    Binding {
        texture: stem.into(),
        scale,
        uv_number: 0,
        address,
        available: true,
    }
}
/// M[Multiple] with an albedo declared at slot 8 and a normal at slot 2 (unbound); slot 8 is also named by an
/// undeclared kind, which the declared one beats. SAT parameters: a scale, a colour with an intensity, no value for
/// the third.
fn material() -> LayeredMaterial {
    let float = |name: &str, v: &[f32]| Parameter {
        name: name.into(),
        value: Value::Float(v.to_vec()),
    };
    LayeredMaterial {
        mtd: "layered.mtd".into(),
        definition: MaterialDefinition {
            shader: "N:\\FDP\\data\\Material\\mtd\\M[Multiple].mtd".into(),
            description: String::new(),
            parameters: vec![
                float("M_Multiple__snp_scale", &[0.5, 0.25]),
                float("M_Multiple__snp_tint_color_0", &[0.2, 0.4, 0.6]),
                float("M_Multiple__snp_tint_color_0-Intensity", &[2.]),
                Parameter {
                    name: "GXFT_SATStaticSwitch".into(),
                    value: Value::Int(vec![27]),
                },
            ],
            textures: vec![
                texture_kind("A_Texture2D_8_AlbedoMap"),
                texture_kind("A_Texture2D_2_NormalMap"),
            ],
            block_lengths: Vec::new(),
        },
        bindings: BTreeMap::from([
            (
                "A_Texture2D_8_AlbedoMap".into(),
                bound("grass_a", [2., 3.], GRASS_ADDRESS),
            ),
            (
                "B_Texture2D_8_AlbedoMap".into(),
                bound("other", [9., 9.], [None; 2]),
            ),
        ]),
    }
}

fn bits(values: &[f32]) -> ConstantRole {
    ConstantRole::Material(values.iter().map(|v| v.to_bits()).collect())
}

/// Every constant of the resolved stage as (buffer, name, byte offset, bytes, role), in buffer and offset order.
fn constants(
    stage: &primase_core::shader::StageBinding,
    shader: &primase_dxbc::Shader,
) -> Vec<(String, String, u32, u32, ConstantRole)> {
    let mut out: Vec<_> = stage
        .constants
        .iter()
        .map(|c| {
            (
                shader
                    .interface
                    .resource(c.resource)
                    .unwrap()
                    .provenance
                    .name
                    .clone(),
                c.name.clone().unwrap(),
                c.offset,
                c.size,
                c.role.clone(),
            )
        })
        .collect();
    out.sort_by(|a, b| (&a.0, a.2).cmp(&(&b.0, b.2)));
    out
}

#[test]
fn a_layered_program_is_bound_by_sekiros_rules() {
    let shader = primase_dxbc::Shader::parse(PROGRAM).unwrap();
    let mut world = World::default();
    let (program, grass) = (world.spawn(), world.spawn());
    let mut asked = Vec::new();
    let resolution = resolve(
        &shader,
        program,
        &material(),
        Unbound::Preview,
        &mut |stem, srgb| {
            asked.push((stem.to_owned(), srgb));
            Ok((stem == "grass_a").then_some(grass))
        },
    )
    .unwrap();
    let stage = &resolution.stage;
    assert_eq!((stage.stage, stage.shader), (Stage::Pixel, program));
    // only the declared kind's texture is asked for, as a colour image
    assert_eq!(asked, [("grass_a".to_owned(), true)]);

    use ConstantRole::{Frame, Object, Unknown, View};
    let engine = |name: &str, offset, size, role| ("Engine".to_owned(), name.to_owned(), offset, size, role);
    let material =
        |name: &str, offset, size, role| ("Material".to_owned(), name.to_owned(), offset, size, role);
    let expected = vec![
        engine("FC_DifMapMulCol", 0, 16, Frame(FrameParameter::AlbedoTint)),
        engine("mWorld", 16, 48, Object(ObjectParameter::World)),
        engine("aColor", 64, 16, Object(ObjectParameter::InstanceColour)),
        engine("FC_ColorSlotNo", 80, 4, Frame(FrameParameter::ColourSlot)),
        engine("FC_GlobalTime", 84, 4, Frame(FrameParameter::Time)),
        engine("FC_TimeOfDay", 88, 4, Frame(FrameParameter::TimeOfDay)),
        engine("SC_PreExposure", 92, 4, Frame(FrameParameter::Exposure)),
        engine("SC_RenderCameraPos", 96, 16, View(ViewParameter::CameraPosition)),
        engine(
            "FC_MatrixInvViewProj",
            112,
            64,
            View(ViewParameter::InverseViewProjection),
        ),
        // the tiling of t2 (unbound: 1 by 1) and t8 (the declared binding's scale), a register's four words each
        material("FC_TextureTilingScale", 32, 16, bits(&[1., 1., 0., 0.])),
        material("FC_TextureTilingScale", 128, 16, bits(&[2., 3., 0., 0.])),
        material("M_Multiple__snp_scale", 160, 8, bits(&[0.5, 0.25])),
        // the colour times its intensity, with the fourth lane one: it fills the padding after it
        material(
            "M_Multiple__snp_tint_color_0",
            176,
            16,
            bits(&[0.2 * 2., 0.4 * 2., 0.6 * 2., 1.]),
        ),
        material(
            "M_Multiple__snp_missing",
            192,
            4,
            Unknown {
                why: "missing SAT parameter left at frozen zero".into(),
            },
        ),
    ];
    assert_eq!(constants(stage, &shader), expected);

    // t2: no binding, the preview normal, counted; t8: the texture, the source states no sampler
    let slots: Vec<_> = stage
        .textures
        .iter()
        .map(|t| {
            (
                shader
                    .interface
                    .resource(t.resource)
                    .unwrap()
                    .provenance
                    .name
                    .clone(),
                t.source.clone(),
            )
        })
        .collect();
    assert_eq!(
        slots,
        [
            (
                "Texture2D_2".to_owned(),
                TextureSource::Default {
                    value: [0.5, 0.5, 1., 1.],
                    why: "preview default for missing NormalMap binding".into()
                }
            ),
            (
                "Texture2D_8".to_owned(),
                TextureSource::Texture {
                    texture: grass,
                    // the stored addressing along u and v; the source states none along w, and no filters
                    sampler: SamplerState {
                        wrap: [Some(AddressMode::Repeat), Some(AddressMode::Mirror), None],
                        ..SamplerState::default()
                    }
                }
            ),
        ]
    );
    assert_eq!(
        resolution.policies,
        BTreeMap::from([("preview default for missing NormalMap binding".to_owned(), 1)])
    );

    // the G-buffer: normal, gloss in x, albedo, reflectance, emission
    let outputs: Vec<_> = stage
        .outputs
        .iter()
        .map(|o| (o.label.index, o.lanes, o.role))
        .collect();
    assert_eq!(
        outputs,
        [
            (
                0,
                0b0111,
                OutputRole::Normal {
                    encoding: NormalEncoding::XyzBiased {
                        bias_bits: 0x3f00_2008
                    },
                    space: NormalSpace::World
                }
            ),
            (1, 0b0001, OutputRole::Gloss),
            (2, 0b0111, OutputRole::Albedo),
            (3, 0b0111, OutputRole::Reflectance),
            (4, 0b0111, OutputRole::Emission),
        ]
    );
    // every input comes from the vertex stage
    assert_eq!(stage.inputs.len(), 3);
    assert!(stage.inputs.iter().all(|i| i.role == InputRole::Previous));
}

#[test]
fn leaving_unbound_slots_invents_nothing() {
    let shader = primase_dxbc::Shader::parse(PROGRAM).unwrap();
    let mut world = World::default();
    let program = world.spawn();
    let resolution = resolve(&shader, program, &material(), Unbound::Leave, &mut |_, _| {
        Ok(None)
    })
    .unwrap();
    assert!(resolution.policies.is_empty());
    assert!(
        resolution
            .stage
            .textures
            .iter()
            .all(|t| t.source == TextureSource::Missing)
    );
    // the bound slot keeps its tiling (the binding states it); the unbound slot has none
    let tiling: Vec<_> = constants(&resolution.stage, &shader)
        .into_iter()
        .filter(|c| c.1 == "FC_TextureTilingScale")
        .collect();
    assert_eq!(tiling.len(), 1);
    assert_eq!((tiling[0].2, &tiling[0].4), (128, &bits(&[2., 3., 0., 0.])));
}

fn dds(width: u32, height: u32) -> Arc<[u8]> {
    let t = Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels: vec![vec![200; (width * height * 4) as usize]],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    };
    primase_dds::write_dx10(&t).unwrap().into()
}

#[test]
fn the_pass_binds_every_layered_material_to_shared_shader_and_texture_entities() {
    let mut world = World::default();
    let mut spawn_material = |layered: Option<LayeredMaterial>| {
        let e = world.spawn();
        world.insert(
            e,
            Material {
                mtd: "layered.mtd".into(),
                definition: layered.as_ref().map(|l| l.definition.clone()),
                gx: Vec::new(),
                shader: "M[Multiple]".into(),
                class: "opaque".into(),
                skip: None,
                cutoff: 0.5,
                bindings: BTreeMap::new(),
                layered,
            },
        );
        e
    };
    let a = spawn_material(Some(material()));
    // a second material with the same program and texture, and one with a program the reader does not hold
    let mut other = material();
    other.definition.parameters[0].value = Value::Float(vec![1., 1.]);
    let b = spawn_material(Some(other));
    let mut absent = material();
    absent.definition.parameters[3].value = Value::Int(vec![99]);
    let c = spawn_material(Some(absent));
    let plain = spawn_material(None);
    world.insert_resource(ShaderPrograms(BTreeMap::from([(
        "M[Multiple]_27_Gbuf.ppo".to_owned(),
        Arc::from(PROGRAM),
    )])));
    world.insert_resource(TexturePayloads(BTreeMap::from([(
        "grass_a".to_owned(),
        dds(4, 4),
    )])));
    // the first material's shader has a bundle holding the G-buffer vertex program
    let bundle = world.spawn();
    world.insert(
        bundle,
        ShaderBundle {
            name: "M[Multiple]".into(),
            programs: vec![ShaderProgram {
                name: "M[Multiple]_Gbuf.vpo".into(),
                stage: ShaderStage::Vertex,
                code: Arc::from(VERTEX_PROGRAM),
            }],
            parameters: None,
        },
    );
    world.insert(a, MaterialShader(bundle));
    let run = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}").unwrap();
    let log = &run.steps[0].log;
    assert_eq!(log.counts["layered materials bound"], 2);
    assert_eq!(
        log.counts["layered materials whose program the reader does not hold"],
        1
    );
    assert_eq!(
        (log.counts["shader programs"], log.counts["bound texture files"]),
        (2, 1)
    );
    assert_eq!(
        log.counts["layered materials without a vertex program (no bundle, or no <shader>_Gbuf.vpo in it)"],
        1
    );
    assert_eq!(
        log.approximations["preview default for missing NormalMap binding"],
        2
    );

    // one shader entity: the bytecode as stored, its format and interface; one texture: the DDS as stored in the
    // slot's colour space
    let shaders: Vec<_> = world.iter::<Shader>().collect();
    assert_eq!(shaders.len(), 2);
    let (shader, program) = *shaders
        .iter()
        .find(|(_, s)| s.interface.stage == Stage::Pixel)
        .unwrap();
    let (vertex, vertex_program) = *shaders
        .iter()
        .find(|(_, s)| s.interface.stage == Stage::Vertex)
        .unwrap();
    assert_eq!(&vertex_program.bytecode[..], VERTEX_PROGRAM);
    assert_eq!(world.get::<Name>(vertex).unwrap().0, "M[Multiple]_Gbuf.vpo");
    assert_eq!(
        (program.format, &program.bytecode[..]),
        (ShaderFormat::Dxbc, PROGRAM)
    );
    assert_eq!(world.get::<Name>(shader).unwrap().0, "M[Multiple]_27_Gbuf.ppo");
    assert_eq!(program.interface.stage, Stage::Pixel);
    let textures: Vec<(Entity, &Texture)> = world.iter::<Texture>().collect();
    assert_eq!(textures.len(), 1);
    let (texture, t) = textures[0];
    assert_eq!(world.get::<Name>(texture).unwrap().0, "grass_a.dds@srgb");
    assert_eq!(
        (t.width, t.format, t.color_space, t.role),
        (4, TexFormat::Rgba8Unorm, ColorSpace::Srgb, TexRole::Color)
    );

    // the material with a bundle is bound in both stages, the other in the pixel stage alone
    let both = world.get::<ShaderBinding>(a).unwrap();
    assert_eq!(
        both.stages
            .iter()
            .map(|s| (s.stage, s.shader))
            .collect::<Vec<_>>(),
        [(Stage::Vertex, vertex), (Stage::Pixel, shader)]
    );
    assert_eq!(world.get::<ShaderBinding>(b).unwrap().stages.len(), 1);
    for e in [a, b] {
        let binding = world.get::<ShaderBinding>(e).unwrap();
        assert_eq!(binding.stage(Stage::Pixel).unwrap().shader, shader);
        let mut refs = Vec::new();
        primase_core::Component::references(binding, &mut refs);
        assert!(refs.contains(&shader) && refs.contains(&texture));
    }
    assert!(!world.has::<ShaderBinding>(c) && !world.has::<ShaderBinding>(plain));
    // the second material's own parameter value, not the first's
    let words = |e: Entity| {
        let b = world.get::<ShaderBinding>(e).unwrap();
        b.stage(Stage::Pixel)
            .unwrap()
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some("M_Multiple__snp_scale"))
            .map(|c| c.role.clone())
    };
    assert_eq!(words(a), Some(bits(&[0.5, 0.25])));
    assert_eq!(words(b), Some(bits(&[1., 1.])));
    let _ = ResourceId(0);
}

#[test]
fn the_unbound_textures_option_is_required_and_named() {
    let mut world = World::default();
    let e = common::run(&mut world, "resolve-fromsoft-shaders")
        .unwrap_err()
        .to_string();
    assert!(e.contains("unbound-textures is required"), "{e}");
    let e = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=grey}")
        .unwrap_err()
        .to_string();
    assert!(e.contains("not preview or leave"), "{e}");
}

/// The bake reads the binding: a frozen frame gives the engine's roles their values, the material's words reach the
/// program, and what is left is counted. The fixture writes the sum of its frame constants to the second output's x,
/// so the frozen time and exposure show in the bake's gloss: 0 (colour slot) + 1.5 (time) + 0 (time of day, left at
/// zero) + 1 (unit exposure).
#[test]
fn the_bake_binds_a_frozen_frame_to_the_roles() {
    use primase_fromsoft::layer_bake::{BaseMipBilinearWrap, FrozenFrame, Inputs, Prepared};
    let shader = primase_dxbc::Shader::parse(PROGRAM).unwrap();
    let mut world = World::default();
    let (program, grass) = (world.spawn(), world.spawn());
    let layered = material();
    let stage = resolve(&shader, program, &layered, Unbound::Preview, &mut |stem, _| {
        Ok((stem == "grass_a").then_some(grass))
    })
    .unwrap()
    .stage;
    let view = || -> primase_fromsoft::Result<_> {
        Ok(std::sync::Arc::new(BaseMipBilinearWrap::new(
            2, 2, &[200; 16], true,
        )?))
    };
    let prepared = Prepared::from_binding(
        &shader,
        PROGRAM,
        &stage,
        &layered,
        &FrozenFrame::static_bake(1.5),
        |_, srgb, address| {
            assert!(srgb, "the albedo is the only bound texture");
            assert_eq!(address, [Address::Wrap, Address::Mirror]);
            view()
        },
        |_, _, _| Ok(None),
    )
    .unwrap();
    for (policy, count) in [
        ("global time at the program's sample time", 1),
        ("unit pre-exposure for static material bake", 1),
        ("time of day left at frozen zero", 1),
        ("missing SAT parameter left at frozen zero", 1),
        ("preview default for missing NormalMap binding", 1),
        ("opaque alpha because main albedo is missing", 1),
        // 96 words are read: 44 of the engine's (all set but the time of day) and 52 of the material's (two
        // registers' tiling, the scale, the colour with its padding lane)
        (
            "buffer words left at frozen zero (including padding and unused fields)",
            1 + 38,
        ),
    ] {
        assert_eq!(prepared.policies.get(policy), Some(&count), "{policy}");
    }
    let bake = prepared.bake(2, [1.; 2], Inputs::default()).unwrap();
    assert_eq!(bake.gloss, Some(vec![2.5; 4]));
}

/// The vertex program's binding: its inputs as streams of the lowered geometry, the engine's constants it names as
/// roles with the offsets of the program's own layout, and no output roles.
#[test]
fn a_vertex_program_is_bound_to_streams_and_engine_roles() {
    use ConstantRole::{Object, View};
    use primase_core::Semantic;
    let interface = primase_dxbc::reflection(VERTEX_PROGRAM).unwrap();
    assert_eq!(interface.stage, Stage::Vertex);
    let mut world = World::default();
    let program = world.spawn();
    let stage = primase_fromsoft::layer_bake::resolve::resolve_vertex(&interface, program).unwrap();
    assert_eq!((stage.stage, stage.shader), (Stage::Vertex, program));
    let inputs: Vec<_> = stage
        .inputs
        .iter()
        .map(|i| (format!("{}{}", i.label.name, i.label.index), i.role.clone()))
        .collect();
    let stream = |name: &str, semantic| (name.to_owned(), InputRole::Stream(semantic));
    assert_eq!(
        inputs,
        [
            stream("POSITION0", Semantic::Position),
            stream("NORMAL0", Semantic::Normal),
            stream("TANGENT0", Semantic::Tangent),
            stream("COLOR1", SECOND_COLOUR),
            (
                "TEXCOORD0".to_owned(),
                InputRole::Lanes(vec![
                    LaneStream {
                        semantic: Semantic::TexCoord(0),
                        first: 0,
                        count: 2
                    },
                    LaneStream {
                        semantic: Semantic::TexCoord(1),
                        first: 2,
                        count: 2
                    }
                ])
            ),
            stream("TEXCOORD1", Semantic::Color(1)),
            stream("TEXCOORD2", Semantic::Color(2)),
            stream("TEXCOORD3", Semantic::Color(3)),
            stream("TEXCOORD4", Semantic::Color(4)),
            stream("TEXCOORD5", Semantic::Color(5)),
            stream("TEXCOORD6", Semantic::Color(6)),
            ("SV_InstanceID0".to_owned(), InputRole::System),
        ]
    );
    // constants of the scene and object buffers the table names; the instance data and the shadow colour it does not
    let named = |name: &str| {
        let c = stage
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some(name))
            .unwrap();
        let buffer = interface.resource(c.resource).unwrap().provenance.name.clone();
        (buffer, c.offset, c.size, c.role.clone())
    };
    let scene = |offset, size, role| ("cbSceneParam".to_owned(), offset, size, role);
    assert_eq!(
        named("SC_CameraPos"),
        scene(0, 16, View(ViewParameter::CameraPosition))
    );
    assert_eq!(
        named("FC_ScreenSize"),
        scene(16, 16, View(ViewParameter::ViewportSize))
    );
    assert_eq!(named("FC_MatrixView"), scene(32, 64, View(ViewParameter::View)));
    assert_eq!(
        named("VC_MatrixViewProj"),
        scene(96, 64, View(ViewParameter::ViewProjection))
    );
    assert_eq!(
        named("VC_aObjMatrix"),
        ("cbObjMatrix".to_owned(), 0, 192, Object(ObjectParameter::World))
    );
    // the texture coordinate scale and scroll are the engine's per draw; nothing the material or bundle states gives them
    let unknown: Vec<_> = stage.unknown_constants().map(|(name, _)| name).collect();
    assert_eq!(unknown, ["VC_TexScroll_0", "VC_TexScroll_1"]);
    for (name, offset) in [("VC_TexScroll_0", 0), ("VC_TexScroll_1", 16)] {
        let (buffer, at, size, role) = named(name);
        assert_eq!((buffer.as_str(), at, size), ("cbTexScroll", offset, 16));
        assert!(matches!(role, ConstantRole::Unknown { .. }), "{name}: {role:?}");
    }
    assert_eq!(stage.constants.len(), 7);
    assert!(stage.outputs.is_empty() && stage.textures.is_empty());
}
