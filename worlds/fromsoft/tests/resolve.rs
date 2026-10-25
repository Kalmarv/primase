#![allow(clippy::unwrap_used)]
//! Sekiro's binding rules (`resolve`) and the `resolve-fromsoft-shaders` pass, on one layered program built to
//! Sekiro's naming (tests/fixtures/layered.hlsl, compiled with fxc): the constants, texture slots and outputs a
//! layered material's binding holds, worked out from the HLSL, fxc's listing of its buffers and the material.
use primase_core::shader::{
    AddressMode, BlendFactor, BlendOp, Compare, ConstantRole, FrameParameter, FrameTexture, InputRole,
    LaneStream, MatrixLines, NormalEncoding, NormalSpace, ObjectParameter, OutputRole, ResourceId,
    SamplerState, ScalarEncoding, Shader, ShaderBinding, ShaderFormat, Stage, TextureSource, ViewParameter,
};
use primase_core::{
    BindingId, ColorSpace, Deformation, Entity, Influences, Joint, JointId, Mesh, Name, Primitive, Rest,
    Semantic, Skeleton, Skin, Slot, Stream, TexFormat, TexRole, Texture, Topology, Transform, WeightRule,
    World,
};
use primase_fromsoft::layer_bake::resolve::{
    SECOND_COLOUR, UNBOUND_MATERIAL_FIELDS, Unbound, render_state, resolve,
};
use primase_fromsoft::materials::{Binding, LayeredMaterial, Material};
use primase_fromsoft::mtd::{self, MaterialDefinition, Parameter, Value};
use primase_fromsoft::sekiro::source::{
    MaterialShader, ShaderBundle, ShaderProgram, ShaderPrograms, ShaderStage, TexturePayloads,
};
use primase_image::sampling::Address;
use primase_shader_exec::PixelShader;
use primase_shader_exec::ProgramShader;
use std::collections::BTreeMap;
use std::sync::Arc;
mod common;

/// The addressing the albedo's FLVER record stores: repeat along u, mirrored repeat along v.
const GRASS_ADDRESS: [Option<AddressMode>; 2] = [Some(AddressMode::Repeat), Some(AddressMode::Mirror)];
const PROGRAM: &[u8] = include_bytes!("fixtures/layered.dxbc");
/// layered.dxbc with a `SAT_MtdParam` array after its named parameters (tests/fixtures/layered_sat.hlsl).
const SAT_PROGRAM: &[u8] = include_bytes!("fixtures/layered_sat.dxbc");
const VERTEX_PROGRAM: &[u8] = include_bytes!("fixtures/layered_vs.dxbc");
const SKIN_PROGRAM: &[u8] = include_bytes!("fixtures/layered_skin_vs.dxbc");
const SKIN_DXIL_PROGRAM: &[u8] = include_bytes!("fixtures/layered_skin_vs.dxil");
/// layered_vs.dxbc with the camera taken from the world position before the matrix (tests/fixtures/layered_vs.hlsl, RELATIVE).
const RELATIVE_PROGRAM: &[u8] = include_bytes!("fixtures/layered_rel_vs.dxbc");

/// layered_vs.dxbc with a constant added to the clip position (tests/fixtures/layered_vs.hlsl, OFFSET): the identity matrix no
/// longer gives the position back.
const OFFSET_PROGRAM: &[u8] = include_bytes!("fixtures/layered_off_vs.dxbc");

/// layered_vs.dxbc with the cloth permutation's shape (tests/fixtures/layered_vs.hlsl, CLOTH).
const CLOTH_PROGRAM: &[u8] = include_bytes!("fixtures/layered_cloth_vs.dxbc");
const LINING_PROGRAM: &[u8] = include_bytes!("fixtures/layered_lining_vs.dxbc");

/// Sekiro's cloth programs' shape, each palette row chosen by the flag (tests/fixtures/layered_cloth_rows_vs.hlsl).
const CLOTH_ROWS_PROGRAM: &[u8] = include_bytes!("fixtures/layered_cloth_rows_vs.dxbc");

/// Elden Ring's LOD billboards' shape, the palette read for the joint's translation alone
/// (tests/fixtures/layered_billboard_vs.hlsl).
const BILLBOARD_PROGRAM: &[u8] = include_bytes!("fixtures/layered_billboard_vs.dxbc");

/// A DXBC vertex program as the pass holds it.
fn dxbc_shader(bytes: &[u8], interface: primase_core::shader::Interface) -> Shader {
    Shader {
        format: ShaderFormat::Dxbc,
        bytecode: bytes.to_vec(),
        interface,
    }
}

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
                float("g_DiffuseMapColor", &[0.8, 0.6, 0.4]),
                Parameter {
                    name: "g_AlphaRef".into(),
                    value: Value::Int(vec![0, 0]),
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

/// A program's container lifted and ready to run.
fn executor(bytes: &[u8]) -> ProgramShader {
    ProgramShader::new(primase_dxbc::lift(bytes).unwrap()).unwrap()
}

/// Every constant of the resolved stage as (buffer, name, byte offset, bytes, role), in buffer and offset order.
fn constants(
    stage: &primase_core::shader::StageBinding,
    shader: &ProgramShader,
) -> Vec<(String, String, u32, u32, ConstantRole)> {
    let mut out: Vec<_> = stage
        .constants
        .iter()
        .map(|c| {
            (
                PixelShader::interface(shader)
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

/// A program that carries a `SAT_MtdParam` array beside its named `<shader>_snp_*` parameters (Sekiro's programs do)
/// binds the named ones as the same program without the array does: reading parameters from the array at the
/// metaparam's start words is for programs that name none (Elden Ring's).
#[test]
fn a_program_with_a_sat_array_keeps_its_named_parameters() {
    let named = |bytes: &[u8]| {
        let shader = executor(bytes);
        let program = World::default().spawn();
        let resolution = resolve(
            &shader,
            program,
            &material().view(),
            primase_fromsoft::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap();
        constants(&resolution.stage, &shader)
            .into_iter()
            .filter(|c| c.1.starts_with("M_Multiple__snp_"))
            .collect::<Vec<_>>()
    };
    let plain = named(PROGRAM);
    assert_eq!(plain.len(), 3);
    assert_eq!(named(SAT_PROGRAM), plain);
}

/// Constants as [`constants`] lists them.
type Bound = Vec<(String, String, u32, u32, ConstantRole)>;

/// `material()` with its definition's shader `shader` (the MTD's path to it), resolved against `bytes`: the `_snp_`
/// constants bound, and the counts.
fn snp_bound(
    bytes: &[u8],
    shader_path: &str,
    keep: impl Fn(&Parameter) -> bool,
) -> (Bound, BTreeMap<String, u64>) {
    let shader = executor(bytes);
    let mut m = material();
    m.definition.shader = shader_path.into();
    m.definition.parameters.retain(keep);
    let resolution = resolve(
        &shader,
        World::default().spawn(),
        &m.view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let snp = constants(&resolution.stage, &shader)
        .into_iter()
        .filter(|c| c.1.contains("_snp_"))
        .collect();
    (snp, resolution.policies)
}

/// A `_cloth` shader's material names its parameters with the base family's prefix, as its program names its fields
/// (Sekiro's `P_BD_M_9040_tops1_Maffler_cloth.mtd`, shader `Character_AMSN_[DetailBlend]_cloth`, and that bundle's
/// `_cloth_1_Gbuf.ppo` both say `Character_AMSN__DetailBlend__snp_0_color_0`): the fields are bound by the parameters'
/// names, as the base shader's material binds them, not after the material's shader name.
#[test]
fn a_cloth_shaders_material_binds_its_programs_fields_by_the_parameters_names() {
    let (plain, _) = snp_bound(PROGRAM, "Material/mtd/M[Multiple].mtd", |_| true);
    let (cloth, policies) = snp_bound(PROGRAM, "Material/mtd/M[Multiple]_cloth.mtd", |_| true);
    assert_eq!(plain.len(), 3);
    assert_eq!(cloth, plain);
    assert!(cloth.contains(&(
        "Material".to_owned(),
        "M_Multiple__snp_scale".to_owned(),
        160,
        8,
        bits(&[0.5, 0.25])
    )));
    assert!(!policies.contains_key(UNBOUND_MATERIAL_FIELDS));
}

/// A program with material fields that none of the material's parameters reach (by name, or through a `SAT_MtdParam`
/// array) keeps its zeros there, and says so: one count.
#[test]
fn a_program_whose_material_fields_bind_nothing_is_counted() {
    let path = "Material/mtd/M[Multiple].mtd";
    for program in [PROGRAM, SAT_PROGRAM] {
        let (bound, policies) = snp_bound(program, path, |p| !p.name.contains("_snp_"));
        assert_eq!(bound, []);
        assert_eq!(policies.get(UNBOUND_MATERIAL_FIELDS), Some(&1));
    }
}

#[test]
fn a_layered_program_is_bound_by_sekiros_rules() {
    let shader = executor(PROGRAM);
    let mut world = World::default();
    let (program, grass) = (world.spawn(), world.spawn());
    let mut asked = Vec::new();
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        None,
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
        // the material's g_DiffuseMapColor and a zero
        engine("FC_DifMapMulCol", 0, 16, bits(&[0.8, 0.6, 0.4, 0.])),
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
        engine("envData", 176, 4, Object(ObjectParameter::EnvironmentData)),
        // the tiling of t2 (unbound: 1 by 1) and t8 (the binding's FLVER scale), a register's four words each, centred
        material("FC_TextureTilingScale", 32, 16, bits(&[1., 1., 0., 0.])),
        material("FC_TextureTilingScale", 128, 16, bits(&[2., 3., -0.5, -1.])),
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
                why: "missing material field left at frozen zero".into(),
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
                PixelShader::interface(&shader)
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

    // the G-buffer: normal, gloss in x (the smoothness over 0.95, squared), albedo, reflectance (its square root),
    // emission; every other lane Unknown with what the lighting programs show of it
    let unknown: Vec<_> = stage
        .outputs
        .iter()
        .filter_map(|o| match &o.role {
            OutputRole::Unknown { why } => Some((o.label.index, o.lanes, why.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        unknown
            .iter()
            .map(|(n, lanes, _)| (*n, *lanes))
            .collect::<Vec<_>>(),
        [
            (0, 0b1000),
            (1, 0b0010),
            (1, 0b1100),
            (2, 0b1000),
            (3, 0b1000),
            (4, 0b1000)
        ]
    );
    assert!(unknown[4].2.contains("the light group"));
    let outputs: Vec<_> = stage
        .outputs
        .iter()
        .filter(|o| !matches!(o.role, OutputRole::Unknown { .. }))
        .map(|o| (o.label.index, o.lanes, o.role.clone()))
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
            (
                1,
                0b0001,
                OutputRole::Gloss {
                    encoding: ScalarEncoding::square_of(0.95)
                }
            ),
            (2, 0b0111, OutputRole::Albedo),
            (
                3,
                0b0111,
                OutputRole::Reflectance {
                    encoding: ScalarEncoding::square_root_of(1.0)
                }
            ),
            (
                4,
                0b0111,
                OutputRole::Emission {
                    encoding: ScalarEncoding::LINEAR
                }
            ),
        ]
    );
    // every input comes from the vertex stage
    assert_eq!(stage.inputs.len(), 3);
    assert!(stage.inputs.iter().all(|i| i.role == InputRole::Previous));
}

#[test]
fn leaving_unbound_slots_invents_nothing() {
    let shader = executor(PROGRAM);
    let mut world = World::default();
    let program = world.spawn();
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Leave,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    assert!(resolution.policies.is_empty());
    assert!(
        resolution
            .stage
            .textures
            .iter()
            .all(|t| t.source == TextureSource::Missing)
    );
    // the tiling is the engine's row whatever is bound: the bound slot's FLVER scale, the unbound slot's 1
    let tiling: Vec<_> = constants(&resolution.stage, &shader)
        .into_iter()
        .filter(|c| c.1 == "FC_TextureTilingScale")
        .map(|c| (c.2, c.4))
        .collect();
    assert_eq!(
        tiling,
        [(32, bits(&[1., 1., 0., 0.])), (128, bits(&[2., 3., -0.5, -1.]))]
    );
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
                definition: layered
                    .as_ref()
                    .map(|l| primase_fromsoft::materials::Definition::Mtd(l.definition.clone())),
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
            game: primase_fromsoft::Game::Sekiro,
            name: "M[Multiple]".into(),
            programs: vec![ShaderProgram {
                name: "M[Multiple]_Gbuf.vpo".into(),
                stage: ShaderStage::Vertex,
                format: primase_core::shader::ShaderFormat::Dxbc,
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
        log.counts["layered materials whose program the reader or the shader's bundle does not hold"],
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
    // The same selected pixel program supplies the card's facts, without an inspection-time lift.
    for entity in [a,b] {
        let facts = &world.get::<primase_core::shader::MaterialPrograms>(entity).unwrap().0[0];
        assert_eq!(facts.shader,Some(shader));
        assert_eq!(facts.hash,Some(primase_shader::source_hash(PROGRAM)));
        assert_eq!(facts.stage,Stage::Pixel);
        assert!(facts.instruction_count.unwrap().count > 0);
        assert_eq!(facts.instruction_count.unwrap().unit,primase_core::shader::InstructionUnit::DxbcInstructions);
        assert!(facts.inputs.iter().all(|input|input.used && input.provenance.d3d.is_some()));
        assert_eq!(facts.outputs,world.get::<ShaderBinding>(entity).unwrap().stage(Stage::Pixel).unwrap().outputs);
        assert!(facts.surface.base.is_some());
    }
    assert!(world.get::<primase_core::shader::MaterialPrograms>(c).unwrap().0[0].not_lifted.is_some());
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
    assert!(
        e.contains("unbound-textures=grey is none of preview or leave"),
        "{e}"
    );
}

/// The bake reads the binding: a frozen frame gives the engine's roles their values, the material's words reach the
/// program, and what is left is counted. The fixture writes the sum of its frame constants to the second output's x,
/// so the frozen time and exposure show in the bake's gloss: 0 (colour slot) + 1.5 (time) + 0 (time of day, left at
/// zero) + 1 (unit exposure).
#[test]
fn the_bake_binds_a_frozen_frame_to_the_roles() {
    use primase_fromsoft::layer_bake::{FrozenFrame, Inputs, Prepared, Programs, wrapped_texture};
    let shader = executor(PROGRAM);
    let mut world = World::default();
    let (program, grass) = (world.spawn(), world.spawn());
    let layered = material();
    let stage = resolve(
        &shader,
        program,
        &layered.view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |stem, _| Ok((stem == "grass_a").then_some(grass)),
    )
    .unwrap()
    .stage;
    let view = || -> primase_fromsoft::Result<_> {
        Ok(std::sync::Arc::new(wrapped_texture(2, 2, &[200; 16], true)?))
    };
    let prepared = Prepared::from_binding(
        Programs {
            shader: &shader,
            bytecode: PROGRAM,
            stage: &stage,
            vertex: None,
        },
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
        ("missing material field left at frozen zero", 1),
        ("preview default for missing NormalMap binding", 1),
        ("opaque alpha because main albedo is missing", 1),
        (
            "instance record frozen to its zeros: environment data, matrix base and clip plane (the environment data reaches no baked lane)",
            1,
        ),
        // the engine's buffer is 12 rows: 45 words set but the time of day, and the 3 padding words after the
        // environment data; 52 of the material's are read (two registers' tiling, the scale, the colour with its padding lane)
        (
            "buffer words left at frozen zero (including padding and unused fields)",
            1 + 3 + 38,
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
    let stage = primase_fromsoft::layer_bake::resolve::resolve_vertex(
        &dxbc_shader(VERTEX_PROGRAM, interface.clone()),
        program,
    )
    .unwrap()
    .stage;
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
                        count: 2,
                        component: 0
                    },
                    LaneStream {
                        semantic: Semantic::TexCoord(1),
                        first: 2,
                        count: 2,
                        component: 0
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
    // the screen size is two pairs: the size in pixels, then its reciprocals
    let screen: Vec<_> = stage
        .constants
        .iter()
        .filter(|c| c.name.as_deref() == Some("FC_ScreenSize"))
        .map(|c| (c.offset, c.size, c.role.clone()))
        .collect();
    assert_eq!(
        screen,
        [
            (16, 8, View(ViewParameter::ViewportSize)),
            (24, 8, View(ViewParameter::InverseViewportSize)),
        ]
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
    // the texture coordinate scale and scroll of each of the four sets: the engine's per-model value
    for set in 0..4u8 {
        let (buffer, at, size, role) = named(&format!("VC_TexScroll_{set}"));
        assert_eq!(
            (buffer.as_str(), at, size, role),
            (
                "cbTexScroll",
                u32::from(set) * 16,
                16,
                Object(ObjectParameter::TextureScroll(set))
            )
        );
    }
    assert_eq!(stage.unknown_constants().count(), 0);
    // the instance record's fields the program reads, by their own names in the first element
    assert_eq!(
        named("matricesData"),
        (
            "cbInstanceData".to_owned(),
            64,
            16,
            Object(ObjectParameter::MatrixBase)
        )
    );
    assert_eq!(
        named("userClipPlane"),
        (
            "cbInstanceData".to_owned(),
            48,
            16,
            Object(ObjectParameter::ClipPlane)
        )
    );
    // eleven constants of the table, the screen size as two bindings
    assert_eq!(stage.constants.len(), 12);
    assert!(stage.outputs.is_empty() && stage.textures.is_empty());
}

/// The role of the view-projection matrix is what the program shows by running it: a program that takes the camera
/// position from the world position first has the camera-relative matrix, one that multiplies the world position as it is
/// has the plain one.
#[test]
fn the_view_projection_is_camera_relative_where_the_program_subtracts_the_camera() {
    let role_of = |bytes: &[u8]| {
        let interface = primase_dxbc::reflection(bytes).unwrap();
        let entity = World::default().spawn();
        let stage =
            primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(bytes, interface), entity)
                .unwrap()
                .stage;
        stage
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some("VC_MatrixViewProj"))
            .unwrap()
            .role
            .clone()
    };
    assert_eq!(
        role_of(RELATIVE_PROGRAM),
        ConstantRole::View(ViewParameter::CameraRelativeViewProjection)
    );
    assert_eq!(
        role_of(VERTEX_PROGRAM),
        ConstantRole::View(ViewParameter::ViewProjection)
    );
    // the skinned program multiplies the blended world position as it is
    assert_eq!(
        role_of(SKIN_PROGRAM),
        ConstantRole::View(ViewParameter::ViewProjection)
    );
}

/// The cloth permutation's inputs and switch: BINORMAL0 is the tangent's w as a byte of an input of its own, POSITION1 is
/// declared and never read (a value-graph fact), and the switch constant is the cloth simulation role.
#[test]
fn the_cloth_permutations_inputs_and_switch_are_stated() {
    let interface = primase_dxbc::reflection(CLOTH_PROGRAM).unwrap();
    let entity = World::default().spawn();
    let stage =
        primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(CLOTH_PROGRAM, interface), entity)
            .unwrap()
            .stage;
    let role = |name: &str| {
        stage
            .inputs
            .iter()
            .find(|i| format!("{}{}", i.label.name, i.label.index) == name)
            .map(|i| i.role.clone())
            .unwrap()
    };
    assert_eq!(
        role("BINORMAL0"),
        InputRole::Lanes(vec![LaneStream {
            semantic: Semantic::Tangent,
            first: 0,
            count: 1,
            component: 3,
        }])
    );
    assert_eq!(
        role("POSITION1"),
        InputRole::Unknown {
            why: "declared by the program and never read".into()
        }
    );
    // the streams the program does read keep theirs
    assert_eq!(role("POSITION0"), InputRole::Stream(Semantic::Position));
    let switch = stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_isEnableCloth"))
        .unwrap();
    assert_eq!(
        switch.role,
        ConstantRole::Object(ObjectParameter::ClothSimulation)
    );
}

/// The Lining_S2 family's inputs the lowered geometry cannot feed: a third coordinate set of integers (TEXCOORD1, which
/// the layered programs read as four floats, a Byte4C member) and a second tangent set (TANGENT2) are unknown, each with
/// the reason, and are not taken for the layered members.
#[test]
fn a_further_coordinate_set_and_tangent_set_are_unknown_not_layer_members() {
    let interface = primase_dxbc::reflection(LINING_PROGRAM).unwrap();
    let entity = World::default().spawn();
    let stage = primase_fromsoft::layer_bake::resolve::resolve_vertex(
        &dxbc_shader(LINING_PROGRAM, interface),
        entity,
    )
    .unwrap()
    .stage;
    let role = |name: &str| {
        stage
            .inputs
            .iter()
            .find(|i| format!("{}{}", i.label.name, i.label.index) == name)
            .map(|i| i.role.clone())
            .unwrap()
    };
    let why = |name: &str| match role(name) {
        InputRole::Unknown { why } => why,
        other => panic!("{name} is {other:?}, not unknown"),
    };
    assert!(
        why("TEXCOORD1").contains("further coordinate set"),
        "{}",
        why("TEXCOORD1")
    );
    assert!(
        why("TANGENT2").contains("further tangent set"),
        "{}",
        why("TANGENT2")
    );
    // the layered programs' float members keep their streams
    let layered = primase_dxbc::reflection(VERTEX_PROGRAM).unwrap();
    let layered =
        primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(VERTEX_PROGRAM, layered), entity)
            .unwrap()
            .stage;
    let member = layered
        .inputs
        .iter()
        .find(|i| i.label.name.eq_ignore_ascii_case("TEXCOORD") && i.label.index == 1)
        .unwrap();
    assert_eq!(member.role, InputRole::Stream(Semantic::Color(1)));
}

/// The joints' palette and the cloth-cancel palette of an Elden Ring (DXIL) skinned program are bound at 256 matrices of
/// three rows, 12,288 bytes, which the program declares as fewer (the capture `ercap3`: see
/// [`primase_fromsoft::layer_bake::resolve::PALETTE_JOINTS`]); Sekiro's (DXBC) state none, for lack of a capture of its
/// palette's size.
#[test]
fn an_elden_ring_palette_is_bound_larger_than_declared_and_sekiros_states_none() {
    let entity = World::default().spawn();
    let resolve =
        |shader: Shader| primase_fromsoft::layer_bake::resolve::resolve_vertex(&shader, entity).unwrap();
    let dxil = {
        let program = primase_dxil::lift(SKIN_DXIL_PROGRAM).unwrap();
        Shader {
            format: ShaderFormat::Dxil,
            bytecode: SKIN_DXIL_PROGRAM.to_vec(),
            interface: program.interface,
        }
    };
    let bound = resolve(dxil);
    // the bound is a policy the caller counts, not a read of the model
    assert!(bound.palette_bound);
    let stage = bound.stage;
    let buffer_of = |name: &str| {
        let c = stage
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some(name))
            .unwrap();
        c.resource
    };
    let palette = buffer_of("VC_aObjMatrix");
    let cancel = buffer_of("VC_aClothCancelObjMatrix");
    assert_ne!(palette, cancel);
    for resource in [palette, cancel] {
        assert_eq!(stage.bound_size(resource), Some(256 * 3 * 16));
    }
    // each buffer once, and only the palettes'
    assert_eq!(stage.constant_buffers.len(), 2);
    let sekiro = resolve(dxbc_shader(
        SKIN_PROGRAM,
        primase_dxbc::reflection(SKIN_PROGRAM).unwrap(),
    ));
    assert!(!sekiro.palette_bound);
    assert!(sekiro.stage.constant_buffers.is_empty());
}

/// A program the run does not decide is taken as camera-relative, said by a reason the caller counts: here a program whose
/// clip position is not the position given at the identity (a constant is added to it).
#[test]
fn an_undecided_view_projection_is_taken_as_camera_relative_with_its_reason() {
    let resolved = |bytes: &[u8]| {
        let interface = primase_dxbc::reflection(bytes).unwrap();
        let entity = World::default().spawn();
        primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(bytes, interface), entity).unwrap()
    };
    let vertex = resolved(OFFSET_PROGRAM);
    let role = vertex
        .stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_MatrixViewProj"))
        .unwrap()
        .role
        .clone();
    assert_eq!(
        role,
        ConstantRole::View(ViewParameter::CameraRelativeViewProjection)
    );
    let why = vertex.undecided.expect("the run does not decide it");
    assert!(why.contains("not the position given"), "{why}");
    // a decided program counts nothing
    assert_eq!(resolved(VERTEX_PROGRAM).undecided, None);
}

/// The skinned permutation: its joint indices and weights are the mesh's `Joints(0)` and `Weights(0)`, and the array the
/// unskinned program reads as object matrices is, in this one, the joints' palette.
#[test]
fn a_skinned_vertex_program_is_bound_to_joint_streams_and_a_palette() {
    let interface = primase_dxbc::reflection(SKIN_PROGRAM).unwrap();
    let entity = World::default().spawn();
    let stage =
        primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(SKIN_PROGRAM, interface), entity)
            .unwrap()
            .stage;
    let role = |name: &str| {
        stage
            .inputs
            .iter()
            .find(|i| format!("{}{}", i.label.name, i.label.index) == name)
            .map(|i| i.role.clone())
            .unwrap()
    };
    assert_eq!(role("BLENDINDICES0"), InputRole::Stream(Semantic::Joints(0)));
    assert_eq!(role("BLENDWEIGHT0"), InputRole::Stream(Semantic::Weights(0)));
    let palette = stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_aObjMatrix"))
        .unwrap();
    assert_eq!(palette.role, ConstantRole::Object(ObjectParameter::BonePalette));
    // the second palette the program applies first
    let cancel = stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_aClothCancelObjMatrix"))
        .unwrap();
    assert_eq!(
        cancel.role,
        ConstantRole::Object(ObjectParameter::ClothCancelPalette)
    );
    // both palettes are held by rows, as the program applies them (an unshown palette would bake at zero and collapse
    // the vertices); the clip matrix, applied as `mul(M, v)`, by columns
    assert_eq!(palette.lines, Some(MatrixLines::Rows));
    assert_eq!(cancel.lines, Some(MatrixLines::Rows));
    let clip = stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_MatrixViewProj"))
        .unwrap();
    assert_eq!(clip.lines, Some(MatrixLines::Columns));
    assert_eq!(stage.unknown_constants().count(), 0);
}

/// Sekiro's cloth permutation chooses each palette row by a flag, the joint's row or the model's VC_World: the palette is
/// still the joints', held by rows as the program applies it on the path that takes it (an unshown palette would bake at
/// zero and collapse the vertices).
#[test]
fn a_cloth_programs_palette_is_read_through_its_flag() {
    let interface = primase_dxbc::reflection(CLOTH_ROWS_PROGRAM).unwrap();
    let entity = World::default().spawn();
    let stage = primase_fromsoft::layer_bake::resolve::resolve_vertex(
        &dxbc_shader(CLOTH_ROWS_PROGRAM, interface),
        entity,
    )
    .unwrap()
    .stage;
    let palette = stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_aObjMatrix"))
        .unwrap();
    assert_eq!(palette.role, ConstantRole::Object(ObjectParameter::BonePalette));
    assert_eq!(palette.lines, Some(MatrixLines::Rows));
}

/// Elden Ring's LOD billboards read only the joint's translation of the palette and never apply its matrix, so the program
/// shows nothing of its lines: they are stated by the engine's rule (rows), and the binding says so for the caller to
/// count.
#[test]
fn a_palette_read_only_for_its_translations_is_held_by_rows_by_the_rule() {
    let interface = primase_dxbc::reflection(BILLBOARD_PROGRAM).unwrap();
    let entity = World::default().spawn();
    let vertex = primase_fromsoft::layer_bake::resolve::resolve_vertex(
        &dxbc_shader(BILLBOARD_PROGRAM, interface),
        entity,
    )
    .unwrap();
    assert!(vertex.palette_rows);
    let palette = vertex
        .stage
        .constants
        .iter()
        .find(|c| c.name.as_deref() == Some("VC_aObjMatrix"))
        .unwrap();
    assert_eq!(palette.role, ConstantRole::Object(ObjectParameter::BonePalette));
    assert_eq!(palette.lines, Some(MatrixLines::Rows));
    // a palette the program applies shows its lines itself
    let interface = primase_dxbc::reflection(CLOTH_ROWS_PROGRAM).unwrap();
    let applied = primase_fromsoft::layer_bake::resolve::resolve_vertex(
        &dxbc_shader(CLOTH_ROWS_PROGRAM, interface),
        entity,
    )
    .unwrap();
    assert!(!applied.palette_rows);
}

/// A material's vertex program follows the data of the primitives that draw it: the skinned permutation when all of them
/// carry joint indices and weights, the unskinned one when none does or when they are mixed (counted), and none, counted,
/// when the bundle has no skinned permutation.
#[test]
fn the_pass_chooses_the_vertex_permutation_by_the_primitives_streams() {
    let primitive = |material: Entity, skinned: bool| {
        let mut streams = BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
        )]);
        if skinned {
            streams.insert(Semantic::Joints(0), Stream::U8x4(vec![[0, 0, 0, 0]; 3]));
            streams.insert(Semantic::Weights(0), Stream::F32x4(vec![[1., 0., 0., 0.]; 3]));
        }
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }
    };
    // `skins`: whether each primitive of the world's one mesh is skinned; `with_skin`: the bundle holds the permutation
    let run = |skins: &[bool], with_skin: bool| {
        let mut world = World::default();
        let a = world.spawn();
        world.insert(
            a,
            Material {
                mtd: "layered.mtd".into(),
                definition: Some(primase_fromsoft::materials::Definition::Mtd(
                    material().definition.clone(),
                )),
                gx: Vec::new(),
                shader: "M[Multiple]".into(),
                class: "opaque".into(),
                skip: None,
                cutoff: 0.5,
                bindings: BTreeMap::new(),
                layered: Some(material()),
            },
        );
        world.insert_resource(ShaderPrograms(BTreeMap::from([(
            "M[Multiple]_27_Gbuf.ppo".to_owned(),
            Arc::from(PROGRAM),
        )])));
        world.insert_resource(TexturePayloads(BTreeMap::from([(
            "grass_a".to_owned(),
            dds(4, 4),
        )])));
        let mut programs = vec![ShaderProgram {
            name: "M[Multiple]_Gbuf.vpo".into(),
            stage: ShaderStage::Vertex,
            code: Arc::from(VERTEX_PROGRAM),
            format: ShaderFormat::Dxbc,
        }];
        if with_skin {
            programs.push(ShaderProgram {
                name: "M[Multiple]_Gbuf_Skin.vpo".into(),
                stage: ShaderStage::Vertex,
                code: Arc::from(SKIN_PROGRAM),
                format: ShaderFormat::Dxbc,
            });
        }
        let bundle = world.spawn();
        world.insert(
            bundle,
            ShaderBundle {
                game: primase_fromsoft::Game::Sekiro,
                name: "M[Multiple]".into(),
                programs,
                parameters: None,
            },
        );
        world.insert(a, MaterialShader(bundle));
        let mesh = world.spawn();
        world.insert(mesh, Mesh::new(skins.iter().map(|&s| primitive(a, s)).collect()));
        // joint and weight streams come with a skin: the skinned primitives bound wholly to one joint
        if skins.contains(&true) {
            let skeleton = world.spawn();
            world.insert(
                skeleton,
                Skeleton {
                    joints: vec![Joint {
                        id: JointId(0),
                        name: "root".into(),
                        parent: None,
                        rest: Rest::Affine(Transform::IDENTITY),
                    }],
                },
            );
            let slot = Slot {
                joint: JointId(0),
                inverse_bind: Transform::IDENTITY,
            };
            world.insert(
                mesh,
                Skin {
                    influences: skins
                        .iter()
                        .map(|&s| s.then_some(Influences::Streams { sets: 1 }))
                        .collect(),
                    weights: WeightRule::SumsToOne,
                    bindings: vec![primase_core::Binding {
                        id: BindingId(0),
                        skeleton,
                        palettes: skins.iter().map(|&s| s.then(|| vec![slot])).collect(),
                        deformation: Deformation::Linear,
                    }],
                },
            );
        }
        let run = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}").unwrap();
        let counts = run.steps[0].log.counts.clone();
        let vertex = world
            .get::<ShaderBinding>(a)
            .unwrap()
            .stage(Stage::Vertex)
            .map(|s| world.get::<Name>(s.shader).unwrap().0.clone());
        (vertex, counts)
    };
    let (vertex, counts) = run(&[true, true], true);
    assert_eq!(vertex.as_deref(), Some("M[Multiple]_Gbuf_Skin.vpo"));
    assert_eq!(
        counts["layered materials drawn only by skinned primitives (the skinned vertex program)"],
        1
    );
    let (vertex, counts) = run(&[false, false], true);
    assert_eq!(vertex.as_deref(), Some("M[Multiple]_Gbuf.vpo"));
    assert!(!counts.keys().any(|k| k.contains("skinned")));
    let (vertex, counts) = run(&[true, false], true);
    assert_eq!(vertex.as_deref(), Some("M[Multiple]_Gbuf.vpo"));
    assert_eq!(
        counts["layered materials drawn by skinned and unskinned primitives (bound to the unskinned vertex program)"],
        1
    );
    let (vertex, counts) = run(&[true], false);
    assert_eq!(vertex, None);
    assert_eq!(
        counts["skinned layered materials whose bundle has no <shader>_Gbuf_Skin.vpo (no vertex program bound)"],
        1
    );
}

const MTD_PROGRAM: &[u8] = include_bytes!("fixtures/layered_mtd.dxbc");

fn with_params(params: &[(&str, Value)]) -> LayeredMaterial {
    let mut layered = material();
    for (name, value) in params {
        layered.definition.parameters.retain(|p| p.name != *name);
        layered.definition.parameters.push(Parameter {
            name: (*name).into(),
            value: value.clone(),
        });
    }
    layered
}

/// The material buffer's engine-named fields come from the MTD's engine-named parameters: the diffuse map's colour from
/// `g_DiffuseMapColor` (rgb, and 0), the alpha test's reference from `g_AlphaRef` (a word over 255, -100 for none), and the
/// flag beside it, whose source is not established, is Unknown.
#[test]
fn the_material_buffers_engine_named_fields_come_from_the_definition() {
    let shader = executor(MTD_PROGRAM);
    let program = World::default().spawn();
    let at = |layered: &LayeredMaterial| {
        let stage = resolve(
            &shader,
            program,
            &layered.view(),
            primase_fromsoft::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap()
        .stage;
        constants(&stage, &shader)
            .into_iter()
            .map(|(_, name, offset, size, role)| (name, offset, size, role))
            .collect::<Vec<_>>()
    };
    let tested = with_params(&[
        ("g_DiffuseMapColor", Value::Float(vec![0.8, 0.6, 0.4])),
        ("g_AlphaRef", Value::Int(vec![128, 64])),
    ]);
    let flag = ConstantRole::Unknown {
        why: "the alpha test's flag the engine derives (zw of FC_AlphaRef): its source is not established"
            .into(),
    };
    let no_glow = ConstantRole::Unknown {
        why: "the material states no g_GlowScale: read only into the emission target's alpha".into(),
    };
    assert_eq!(
        at(&tested),
        [
            ("FC_DifMapMulCol".to_owned(), 0, 16, bits(&[0.8, 0.6, 0.4, 0.])),
            ("FC_AlphaRef".to_owned(), 80, 8, bits(&[128. / 255., 64. / 255.])),
            ("FC_AlphaRef".to_owned(), 88, 8, flag.clone()),
            ("FC_GlowScale".to_owned(), 144, 16, no_glow),
        ]
    );
    // a stated g_GlowScale is the buffer's
    let glow = with_params(&[("g_GlowScale", Value::Float(vec![1., 1., 1., 0.5]))]);
    assert_eq!(
        at(&glow).last().unwrap(),
        &("FC_GlowScale".to_owned(), 144, 16, bits(&[1., 1., 1., 0.5]))
    );
    // no alpha reference: the test never discards (-100); no diffuse colour: Unknown by name
    let plain = with_params(&[("g_AlphaRef", Value::Int(vec![0, 0]))]);
    let mut without_colour = plain.clone();
    without_colour
        .definition
        .parameters
        .retain(|p| p.name != "g_DiffuseMapColor");
    let got = at(&without_colour);
    assert_eq!(got[1], ("FC_AlphaRef".to_owned(), 80, 8, bits(&[-100., 0.])));
    assert!(
        matches!(&got[0].3, ConstantRole::Unknown { why } if why.contains("g_DiffuseMapColor")),
        "{:?}",
        got[0]
    );
    // the program the reference selects: the alpha-tested permutation, which discards; the plain one when there is none
    assert_eq!(tested.program_name().unwrap(), "M[Multiple]_27_Gbuf_[A].ppo");
    assert_eq!(plain.program_name().unwrap(), "M[Multiple]_27_Gbuf.ppo");
}

/// The alpha-tested fixture (`fixtures/alpha_ref.hlsl`): `tested` discards where the alpha it writes to target 2's w is
/// below FC_AlphaRef.x, `unrelated` by FC_AlphaRef.x against a value it writes to no target; each as fxc and dxc compile it.
const ALPHA_TESTED: [(&str, &[u8], &[u8]); 2] = [
    (
        "tested",
        include_bytes!("fixtures/alpha_ref_tested.dxbc"),
        include_bytes!("fixtures/alpha_ref_tested.dxil"),
    ),
    (
        "unrelated",
        include_bytes!("fixtures/alpha_ref_unrelated.dxbc"),
        include_bytes!("fixtures/alpha_ref_unrelated.dxil"),
    ),
];

/// The entry `entry` of the alpha-tested fixture, lifted from each bytecode, with the bytecode's name.
fn alpha_programs(entry: &str) -> Vec<(&'static str, ProgramShader)> {
    let (_, dxbc, dxil) = ALPHA_TESTED.iter().find(|f| f.0 == entry).unwrap();
    vec![
        ("dxbc", executor(dxbc)),
        (
            "dxil",
            ProgramShader::new(primase_dxil::lift(dxil).unwrap()).unwrap(),
        ),
    ]
}

/// A material stating `g_AlphaRef` as `aref` (none where `None`).
fn alpha_material(aref: Option<[i32; 2]>) -> LayeredMaterial {
    let mut m = with_params(&[("g_DiffuseMapColor", Value::Float(vec![1., 1., 1.]))]);
    m.definition.parameters.retain(|p| p.name != "g_AlphaRef");
    if let Some(a) = aref {
        m.definition.parameters.push(Parameter {
            name: "g_AlphaRef".into(),
            value: Value::Int(a.to_vec()),
        });
    }
    m
}

/// FC_AlphaRef.x is the alpha test's reference where the program's discard compares the alpha it writes with it: the
/// material's `g_AlphaRef` over 255 where above 0, the engine's -100 where 0, negative or not stated; the y beside it the
/// material's. Where the discard compares it with a value written nowhere, both words stay the material's.
#[test]
fn an_alpha_tested_programs_discard_makes_fc_alpha_refs_x_its_reference() {
    let program = World::default().spawn();
    let fc_alpha_ref = |shader: &ProgramShader, material: &LayeredMaterial| {
        let stage = resolve(
            shader,
            program,
            &material.view(),
            primase_fromsoft::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap()
        .stage;
        constants(&stage, shader)
            .into_iter()
            .filter(|c| c.1 == "FC_AlphaRef" && c.2 < 88)
            .map(|c| (c.2, c.3, c.4))
            .collect::<Vec<_>>()
    };
    for (bytecode, shader) in alpha_programs("tested") {
        for (aref, x) in [
            (Some([128, 64]), 128. / 255.),
            (Some([0, 0]), -100.),
            (Some([-5, 0]), -100.),
            (None, -100.),
        ] {
            let y = if aref == Some([128, 64]) { 64. / 255. } else { 0. };
            assert_eq!(
                fc_alpha_ref(&shader, &alpha_material(aref)),
                [
                    (
                        80,
                        4,
                        ConstantRole::AlphaReference {
                            value: x,
                            compared: primase_core::shader::AlphaComparison::Output
                        }
                    ),
                    (84, 4, bits(&[y]))
                ],
                "{bytecode} {aref:?}"
            );
        }
    }
    for (bytecode, shader) in alpha_programs("unrelated") {
        assert_eq!(
            fc_alpha_ref(&shader, &alpha_material(Some([128, 64]))),
            [(80, 8, bits(&[128. / 255., 64. / 255.]))],
            "{bytecode}"
        );
    }
}

/// Both bakes keep every texel of an alpha-tested material by the one rule its binding states: the reference frozen
/// off (-100), counted where it would cut (above 0), and neither counts a material whose program tests nothing.
#[test]
fn both_bakes_freeze_the_alpha_test_off_by_the_binding() {
    use primase_fromsoft::detail_probe::ProgramProbe;
    use primase_fromsoft::layer_bake::{ALPHA_TEST_POLICY, FrozenFrame, Prepared, Programs};
    let (_, tested, _) = ALPHA_TESTED[0];
    let shader = executor(tested);
    let program = World::default().spawn();
    for (aref, counted) in [
        (Some([128, 0]), true),
        (Some([0, 0]), false),
        (Some([-5, 0]), false),
        (None, false),
    ] {
        let material = alpha_material(aref);
        let stage = resolve(
            &shader,
            program,
            &material.view(),
            primase_fromsoft::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap()
        .stage;
        // the layered bake
        let prepared = Prepared::from_binding(
            Programs {
                shader: &shader,
                bytecode: tested,
                stage: &stage,
                vertex: None,
            },
            &material,
            &FrozenFrame::static_bake(0.),
            |_, _, _| unreachable!("no texture is bound"),
            |_, _, _| Ok(None),
        )
        .unwrap();
        // the program bake, as the detail probe freezes a program
        let probe = ProgramProbe::with_material(tested, &material).unwrap();
        assert_eq!(
            prepared.policies.contains_key(ALPHA_TEST_POLICY),
            counted,
            "{aref:?}"
        );
        assert_eq!(
            probe.policies.contains_key(ALPHA_TEST_POLICY),
            counted,
            "{aref:?}"
        );
        // FC_AlphaRef.x, cbMtdParam (b3) word 20, as the program bake runs it
        assert_eq!(probe.words(3, 20, 1), Some(vec![(-100f32).to_bits()]), "{aref:?}");
    }
    // the plain program's FC_AlphaRef is read into a colour, not tested: neither bake freezes or counts it
    let plain = with_params(&[("g_AlphaRef", Value::Int(vec![128, 0]))]);
    let shader = executor(MTD_PROGRAM);
    let stage = resolve(
        &shader,
        program,
        &plain.view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap()
    .stage;
    let prepared = Prepared::from_binding(
        Programs {
            shader: &shader,
            bytecode: MTD_PROGRAM,
            stage: &stage,
            vertex: None,
        },
        &plain,
        &FrozenFrame::static_bake(0.),
        |_, _, _| unreachable!("no texture is bound"),
        |_, _, _| Ok(None),
    )
    .unwrap();
    assert_eq!(prepared.policies.get(ALPHA_TEST_POLICY), None);
    let probe = ProgramProbe::with_material(MTD_PROGRAM, &plain).unwrap();
    assert_eq!(probe.policies.get(ALPHA_TEST_POLICY), None);
    assert_eq!(probe.words(3, 20, 1), Some(vec![(128f32 / 255.).to_bits()]));
}

/// A texture's tiling is its uv group's, the group the metaparam records for the same `Texture2D_N`, whatever the
/// record's slot (its register, which need not be that number: Sekiro's `Character_AMSN_[DetailBlend]` records its base
/// albedo `Texture2D_7` at slot 1 and a detail fabric `Texture2D_2`, tiled 51 by 17, at slot 7).
#[test]
fn a_textures_tiling_is_the_uv_group_of_its_own_texture_number() {
    use primase_fromsoft::metaparam::{Metaparam, Texture as MetaTexture};
    let shader = executor(PROGRAM);
    let record = |name: &str, slot: u8, uv_group: &str| MetaTexture {
        name: name.into(),
        default_texture: String::new(),
        uv_group: uv_group.into(),
        slot,
        kind: 1,
        unknown_08: 0,
        unknown_0c: -1,
        unknown_20: 0,
    };
    // the fixture program's Texture2D_8 is at register t8 and its Texture2D_2 at t2: each record's slot is the other's
    let metaparam = Metaparam {
        constant_buffer_size: 0,
        shader_id: 0,
        flags: 0,
        gxmd_key: 0,
        ca_param_block_size: 0,
        ca_param_components: 0,
        unknown_30: 0,
        unknown_40: 0,
        unknown_50: 0,
        textures: vec![
            record("M_Multiple__snp_Texture2D_8_AlbedoMap", 2, "group_3"),
            record("M_Multiple__snp_Texture2D_2_NormalMap", 8, "group_9"),
        ],
        passes: Vec::new(),
        ca_params: Vec::new(),
        mtd_params: Vec::new(),
    };
    let mut m = material();
    // a FLVER scale of 1 leaves the slot to the uv group
    m.bindings.get_mut("A_Texture2D_8_AlbedoMap").unwrap().scale = [1.; 2];
    for (group, scale) in [("group_3", [51., 17.]), ("group_9", [5., 5.])] {
        m.definition.parameters.push(Parameter {
            name: format!("{group}_CommonUV-UVParam"),
            value: Value::Float(scale.to_vec()),
        });
    }
    let resolution = resolve(
        &shader,
        World::default().spawn(),
        &m.view(),
        primase_fromsoft::Game::Sekiro,
        Some(&metaparam),
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let tiling: Vec<(u32, ConstantRole)> = constants(&resolution.stage, &shader)
        .into_iter()
        .filter(|c| c.1 == "FC_TextureTilingScale")
        .map(|c| (c.2, c.4))
        .collect();
    // rows 2 and 8, sixteen bytes each
    assert_eq!(
        tiling,
        [
            (32, bits(&[5., 5., -2., -2.])),
            (128, bits(&[51., 17., -25., -8.]))
        ]
    );
}

/// A texture's tiling is the FLVER's scale where it is not 1, else the scale the definition stores for the slot where it
/// is not zero, else the uv group's, centred: the rows Sekiro's capture shows for the Wolf's scarf
/// (`P_BD_M_9040_tops1_Maffler.mtd` stores 18 by 6 at `Texture2D_1`, whose uv group is 1 by 1, and zero at
/// `Texture2D_2`, whose uv group is 51 by 17: captured `18, 6, -8.5, -2.5` and `51, 17, -25, -8`). Both games' rule.
#[test]
fn a_textures_tiling_is_the_flvers_then_the_stored_then_the_uv_groups() {
    use primase_fromsoft::metaparam::{Metaparam, Texture as MetaTexture};
    let shader = executor(PROGRAM);
    let record = |name: &str, slot: u8, uv_group: &str| MetaTexture {
        name: name.into(),
        default_texture: String::new(),
        uv_group: uv_group.into(),
        slot,
        kind: 1,
        unknown_08: 0,
        unknown_0c: -1,
        unknown_20: 0,
    };
    let metaparam = Metaparam {
        constant_buffer_size: 0,
        shader_id: 0,
        flags: 0,
        gxmd_key: 0,
        ca_param_block_size: 0,
        ca_param_components: 0,
        unknown_30: 0,
        unknown_40: 0,
        unknown_50: 0,
        textures: vec![
            record("M_Multiple__snp_Texture2D_8_AlbedoMap", 8, "group_2"),
            record("M_Multiple__snp_Texture2D_2_NormalMap", 2, "group_3"),
        ],
        passes: Vec::new(),
        ca_params: Vec::new(),
        mtd_params: Vec::new(),
    };
    let rows = |flver: [f32; 2], game| {
        let mut m = material();
        m.bindings.get_mut("A_Texture2D_8_AlbedoMap").unwrap().scale = flver;
        m.definition.textures[0].extra = vec![18., 6.];
        m.definition.textures[1].extra = vec![0., 0.];
        for (group, scale) in [("group_2", [1., 1.]), ("group_3", [51., 17.])] {
            m.definition.parameters.push(Parameter {
                name: format!("{group}_CommonUV-UVParam"),
                value: Value::Float(scale.to_vec()),
            });
        }
        let resolution = resolve(
            &shader,
            World::default().spawn(),
            &m.view(),
            game,
            Some(&metaparam),
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap();
        constants(&resolution.stage, &shader)
            .into_iter()
            .filter(|c| c.1 == "FC_TextureTilingScale")
            .map(|c| (c.2, c.4))
            .collect::<Vec<(u32, ConstantRole)>>()
    };
    let scarf = [
        (32, bits(&[51., 17., -25., -8.])),
        (128, bits(&[18., 6., -8.5, -2.5])),
    ];
    assert_eq!(rows([1.; 2], primase_fromsoft::Game::Sekiro), scarf);
    assert_eq!(rows([1.; 2], primase_fromsoft::Game::EldenRing), scarf);
    // a FLVER scale other than 1 comes first
    assert_eq!(
        rows([2., 3.], primase_fromsoft::Game::Sekiro)[1],
        (128, bits(&[2., 3., -0.5, -1.]))
    );
}

/// A material field the material does not state takes the default its shader bundle's metaparam states, counted; one the
/// metaparam does not declare is still Unknown.
#[test]
fn a_missing_sat_parameter_takes_the_bundles_metaparam_default() {
    use primase_fromsoft::metaparam::{CaParam, Metaparam, MtdParam};
    let shader = executor(PROGRAM);
    let program = World::default().spawn();
    let metaparam = Metaparam {
        constant_buffer_size: 0,
        shader_id: 0,
        flags: 0,
        gxmd_key: 0,
        ca_param_block_size: 0,
        ca_param_components: 0,
        unknown_30: 0,
        unknown_40: 0,
        unknown_50: 0,
        textures: Vec::new(),
        passes: Vec::new(),
        ca_params: Vec::new(),
        mtd_params: vec![MtdParam {
            name: "M_Multiple__snp_missing".into(),
            start: 0,
            sequence_index: 0,
            value_type: 1,
            not_shader_input: false,
            srgb: false,
            colour: false,
            key: 0,
            default: [0.75, 0., 0., 0., 0.],
        }],
    };
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        Some(&metaparam),
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let missing = constants(&resolution.stage, &shader)
        .into_iter()
        .find(|c| c.1 == "M_Multiple__snp_missing")
        .unwrap();
    assert_eq!(missing.4, bits(&[0.75]));
    assert_eq!(
        resolution
            .policies
            .get("missing material field taken from the shader bundle's metaparam default"),
        Some(&1)
    );
    // a bundle that declares it as a CAParam (one component per record, by the program's own field name) gives its default
    let by_ca = Metaparam {
        mtd_params: Vec::new(),
        ca_params: vec![CaParam {
            name: "M_Multiple__snp_missing".into(),
            sequence_index: 0,
            param_id: 0,
            component: 0,
            default: 0.25,
            unknown_40: 0,
            key: 0,
        }],
        ..metaparam
    };
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        Some(&by_ca),
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let missing = constants(&resolution.stage, &shader)
        .into_iter()
        .find(|c| c.1 == "M_Multiple__snp_missing")
        .unwrap();
    assert_eq!(missing.4, bits(&[0.25]));
    assert_eq!(
        resolution
            .policies
            .get("missing material field taken from the shader bundle's CAParam default"),
        Some(&1)
    );
}

/// A colour's intensity is the material's where it states one (Sekiro's `<name>-Intensity`); a colour stated without
/// it takes the metaparam default's, else 1, counted under the path that writes it.
#[test]
fn a_colour_stated_without_its_intensity_takes_the_defaults_counted() {
    use primase_fromsoft::metaparam::{Metaparam, MtdParam};
    let shader = executor(PROGRAM);
    let program = World::default().spawn();
    let mut layered = material();
    layered
        .definition
        .parameters
        .retain(|p| p.name != "M_Multiple__snp_tint_color_0-Intensity");
    let metaparam = Metaparam {
        constant_buffer_size: 0,
        shader_id: 0,
        flags: 0,
        gxmd_key: 0,
        ca_param_block_size: 0,
        ca_param_components: 0,
        unknown_30: 0,
        unknown_40: 0,
        unknown_50: 0,
        textures: Vec::new(),
        passes: Vec::new(),
        ca_params: Vec::new(),
        mtd_params: vec![MtdParam {
            name: "M_Multiple__snp_tint_color_0".into(),
            start: 0,
            sequence_index: 0,
            value_type: 12,
            not_shader_input: false,
            srgb: false,
            colour: true,
            key: 0,
            default: [1., 1., 1., 1., 3.],
        }],
    };
    for (metaparam, intensity, why) in [
        (
            Some(&metaparam),
            3.,
            "material field colour stated without its intensity: the metaparam default's",
        ),
        (
            None,
            1.,
            "material field colour stated without its intensity: 1, as no metaparam declares it",
        ),
    ] {
        let resolution = resolve(
            &shader,
            program,
            &layered.view(),
            primase_fromsoft::Game::Sekiro,
            metaparam,
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap();
        let colour = constants(&resolution.stage, &shader)
            .into_iter()
            .find(|c| c.1 == "M_Multiple__snp_tint_color_0")
            .unwrap();
        assert_eq!(
            colour.4,
            bits(&[0.2 * intensity, 0.4 * intensity, 0.6 * intensity, 1.])
        );
        assert_eq!(
            resolution.policies.get(why),
            Some(&1),
            "{:?}",
            resolution.policies
        );
    }
    // stated, it is the material's and not counted
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        Some(&metaparam),
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    assert!(
        !resolution
            .policies
            .keys()
            .any(|k| k.contains("without its intensity")),
        "{:?}",
        resolution.policies
    );

    // A stated sRGB flag changes the encoding, not the source of the intensity. The independently attributed
    // coat cb3 words establish power before intensity; the named path uses the same rule as SAT.
    let mut srgb = metaparam.clone();
    srgb.mtd_params[0].srgb = true;
    for intensity in [1., 3.] {
        srgb.mtd_params[0].default[4] = intensity;
        let resolution = resolve(
            &shader,
            program,
            &layered.view(),
            primase_fromsoft::Game::Sekiro,
            Some(&srgb),
            Unbound::Preview,
            &mut |_, _| Ok(None),
        )
        .unwrap();
        let binding = resolution
            .stage
            .constants
            .iter()
            .find(|c| c.name.as_deref() == Some("M_Multiple__snp_tint_color_0"))
            .unwrap();
        assert_eq!(
            binding.role,
            bits(&[
                0.2_f32.powf(2.2) * intensity,
                0.4_f32.powf(2.2) * intensity,
                0.6_f32.powf(2.2) * intensity,
                1.,
            ])
        );
    }
}

/// The SAT shaders that are not layered (no static switch: one `<shader>_Gbuf.ppo`) are bound the same way, their program
/// taken from the shader's bundle; counted apart from the layered materials.
#[test]
fn the_pass_binds_the_other_sat_materials_through_their_bundle() {
    let mut world = World::default();
    let mut definition = material().definition.clone();
    definition.shader = "N:\\NTC\\data\\Material\\sat\\OutputData\\SPX\\MetallicValue.spx".into();
    definition.parameters.retain(|p| p.name != "GXFT_SATStaticSwitch");
    let plain = world.spawn();
    world.insert(
        plain,
        Material {
            mtd: "M[AMSN_light]".into(),
            definition: Some(primase_fromsoft::materials::Definition::Mtd(definition.clone())),
            gx: Vec::new(),
            shader: "MetallicValue".into(),
            class: "opaque".into(),
            skip: None,
            cutoff: 0.5,
            bindings: material().bindings,
            layered: None,
        },
    );
    // a second one whose bundle holds no such program
    let lacking = world.spawn();
    let mut other = world.get::<Material>(plain).unwrap().clone();
    other.shader = "MetallicTex".into();
    if let Some(primase_fromsoft::materials::Definition::Mtd(d)) = &mut other.definition {
        d.shader = "N:\\NTC\\data\\Material\\sat\\OutputData\\SPX\\MetallicTex.spx".into();
    }
    world.insert(lacking, other);
    world.insert_resource(ShaderPrograms(BTreeMap::new()));
    world.insert_resource(TexturePayloads(BTreeMap::from([(
        "grass_a".to_owned(),
        dds(4, 4),
    )])));
    let bundle = |name: &str, program: &str| ShaderBundle {
        game: primase_fromsoft::Game::Sekiro,
        name: name.into(),
        programs: vec![ShaderProgram {
            name: program.into(),
            stage: ShaderStage::Pixel,
            format: primase_core::shader::ShaderFormat::Dxbc,
            code: Arc::from(PROGRAM),
        }],
        parameters: None,
    };
    let (a, b) = (world.spawn(), world.spawn());
    world.insert(a, bundle("MetallicValue", "MetallicValue_Gbuf.ppo"));
    world.insert(b, bundle("MetallicTex", "MetallicTex_Gbuf_[A].ppo"));
    world.insert(plain, MaterialShader(a));
    world.insert(lacking, MaterialShader(b));
    let run = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}").unwrap();
    let counts = &run.steps[0].log.counts;
    assert_eq!(counts["other materials bound"], 1);
    assert_eq!(
        counts["other materials whose program the reader or the shader's bundle does not hold"],
        1
    );
    assert!(!counts.contains_key("layered materials bound") || counts["layered materials bound"] == 0);
    let binding = world.get::<ShaderBinding>(plain).unwrap();
    let pixel = binding.stage(Stage::Pixel).unwrap();
    assert_eq!(
        world.get::<Name>(pixel.shader).unwrap().0,
        "MetallicValue_Gbuf.ppo"
    );
    assert!(world.get::<ShaderBinding>(lacking).is_none());
}

/// A vertex program that takes one coordinate set binds it alone, not as the layered programs' packed pair.
#[test]
fn a_one_set_vertex_program_binds_its_coordinates_as_the_first_set() {
    const SINGLE: &[u8] = include_bytes!("fixtures/single_uv_vs.dxbc");
    let interface = primase_dxbc::reflection(SINGLE).unwrap();
    let entity = World::default().spawn();
    let stage =
        primase_fromsoft::layer_bake::resolve::resolve_vertex(&dxbc_shader(SINGLE, interface), entity)
            .unwrap()
            .stage;
    let uv = stage
        .inputs
        .iter()
        .find(|i| i.label.name == "TEXCOORD")
        .map(|i| i.role.clone())
        .unwrap();
    assert_eq!(uv, InputRole::Stream(Semantic::TexCoord(0)));
}

/// A fixed-function (`GXFlver_*`) material's textures are addressed by the kind's own name, as the program calls them
/// (`g_DiffuseTexture` is its albedo, in colour; `g_BumpmapTexture` its normal map), not by a slot number; the engine's
/// dither noise stays the engine's. Its program is `<shader>_Gbuf.ppo` (no static switch).
#[test]
fn a_fixed_function_materials_textures_are_bound_by_their_kind_names() {
    const FIXED: &[u8] = include_bytes!("fixtures/fixed_function.dxbc");
    let shader = executor(FIXED);
    let mut world = World::default();
    let (program, albedo, normal) = (world.spawn(), world.spawn(), world.spawn());
    let layered = LayeredMaterial {
        mtd: "M[ARSN]".into(),
        definition: MaterialDefinition {
            shader: "N:\\NTC\\data\\Material\\mtd\\GXFlver_ColDifSpcBumpIbl.spx".into(),
            description: String::new(),
            parameters: vec![
                Parameter {
                    name: "g_DiffuseMapColor".into(),
                    value: Value::Float(vec![0.5, 0.25, 1.]),
                },
                Parameter {
                    name: "g_AlphaRef".into(),
                    value: Value::Int(vec![0, 0]),
                },
            ],
            textures: vec![texture_kind("g_DiffuseTexture"), texture_kind("g_BumpmapTexture")],
            block_lengths: Vec::new(),
        },
        bindings: BTreeMap::from([
            ("g_DiffuseTexture".into(), bound("brick_a", [2., 3.], [None; 2])),
            ("g_BumpmapTexture".into(), bound("brick_n", [4., 5.], [None; 2])),
        ]),
    };
    assert_eq!(layered.gbuffer_program(), "GXFlver_ColDifSpcBumpIbl_Gbuf.ppo");
    let mut asked = Vec::new();
    let resolution = resolve(
        &shader,
        program,
        &layered.view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |stem, srgb| {
            asked.push((stem.to_owned(), srgb));
            Ok(match stem {
                "brick_a" => Some(albedo),
                "brick_n" => Some(normal),
                _ => None,
            })
        },
    )
    .unwrap();
    // the albedo as a colour image, the normal map as data
    asked.sort();
    assert_eq!(
        asked,
        [("brick_a".to_owned(), true), ("brick_n".to_owned(), false)]
    );
    let source = |name: &str| {
        let resource = shader
            .interface()
            .resources
            .iter()
            .find(|r| r.provenance.name == name)
            .unwrap();
        resolution
            .stage
            .textures
            .iter()
            .find(|t| t.resource == resource.id)
            .unwrap()
            .source
            .clone()
    };
    assert!(
        matches!(source("g_DiffuseTexture"), TextureSource::Texture { texture, .. } if texture == albedo)
    );
    assert!(
        matches!(source("g_BumpmapTexture"), TextureSource::Texture { texture, .. } if texture == normal)
    );
    assert!(matches!(
        source("g_DitherTexture"),
        TextureSource::Default { value, .. } if value == [0.5; 4]
    ));
    // the tiling of t0 and t3 (the registers) from the bindings' FLVER scales, centred; the colour from g_DiffuseMapColor
    let tiling: Vec<_> = constants(&resolution.stage, &shader)
        .into_iter()
        .filter(|c| c.1 == "FC_TextureTilingScale")
        .map(|c| (c.2, c.4))
        .collect();
    assert_eq!(
        tiling,
        [(0, bits(&[2., 3., -0.5, -1.])), (48, bits(&[4., 5., -1.5, -2.]))]
    );
}

/// The render state is what the capture of the running game settled: depth test `GreaterEqual` (reversed depth) with a write
/// unless `g_GX_bDisableDepthWrite`, and for `g_BlendMode` 2 the SrcAlpha/InvSrcAlpha blend on render target 0; the alpha
/// test, the cull, the stencil and the bias are not stated (the program discards, the face set culls).
#[test]
fn the_render_state_is_what_the_capture_settled() {
    let opaque = render_state(&with_params(&[
        ("g_BlendMode", Value::Int(vec![0])),
        ("g_AlphaRef", Value::Int(vec![128, 0])),
    ]));
    assert_eq!(opaque.verify(), Ok(()));
    let depth = opaque.depth.unwrap();
    assert_eq!((depth.test, depth.write), (Compare::GreaterEqual, true));
    assert!(opaque.blend.is_none() && opaque.targets.iter().all(Option::is_none));
    assert!(opaque.alpha_test.is_none() && opaque.cull.is_none() && opaque.stencil.is_none());
    assert!(opaque.depth_bias.is_none() && opaque.front_counter_clockwise.is_none());
    let blended = render_state(&with_params(&[
        ("g_BlendMode", Value::Int(vec![2])),
        ("g_GX_bDisableDepthWrite", Value::Bool(true)),
    ]));
    assert_eq!(blended.verify(), Ok(()));
    assert!(!blended.depth.unwrap().write);
    let blend = blended.blend.unwrap();
    assert_eq!(
        (blend.colour.source, blend.colour.destination, blend.colour.op),
        (BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha, BlendOp::Add)
    );
    assert_eq!(
        (blend.alpha.source, blend.alpha.destination),
        (BlendFactor::One, BlendFactor::Zero)
    );
    assert_eq!(blended.targets[0].unwrap().write_mask, 0xf);
    // a mode the capture matched to no draw states no blend
    assert!(
        render_state(&with_params(&[("g_BlendMode", Value::Int(vec![4]))]))
            .blend
            .is_none()
    );
}

/// An alpha-blended material (`g_BlendMode` 2) is drawn in the forward pass: its pixel program is `<shader>_<switch>_Fwd`
/// (`_[A]` with an alpha reference), not `_Gbuf`, and the forward program's render target 0 is the finished colour.
#[test]
fn a_blended_material_is_bound_to_its_forward_program() {
    let opaque = with_params(&[
        ("GXFT_SATStaticSwitch", Value::Int(vec![27])),
        ("g_AlphaRef", Value::Int(vec![128, 0])),
    ]);
    assert!(!opaque.is_forward());
    assert_eq!(opaque.pass_program(), "M[Multiple]_27_Gbuf_[A].ppo");
    let blended = with_params(&[
        ("GXFT_SATStaticSwitch", Value::Int(vec![27])),
        ("g_AlphaRef", Value::Int(vec![128, 0])),
        ("g_BlendMode", Value::Int(vec![2])),
    ]);
    assert!(blended.is_forward());
    assert_eq!(blended.pass(), "Fwd");
    assert_eq!(blended.pass_program(), "M[Multiple]_27_Fwd_[A].ppo");
    // the G-buffer name stays what the layered readers ask for
    assert_eq!(blended.gbuffer_program(), "M[Multiple]_27_Gbuf_[A].ppo");
    // a blend mode the capture matched to no draw keeps the G-buffer program
    let additive = with_params(&[
        ("GXFT_SATStaticSwitch", Value::Int(vec![27])),
        ("g_BlendMode", Value::Int(vec![4])),
    ]);
    assert_eq!(additive.pass_program(), "M[Multiple]_27_Gbuf.ppo");
}

/// The dither and camera-fade fields an alpha-tested program reads from the per-draw buffers are the engine's per-draw state
/// (roles of the object), never a material word and never unbound; a consumer that freezes them gets zero (both fades off) and
/// counts it.
#[test]
fn the_dither_and_camera_fade_fields_are_object_roles() {
    const FADE: &[u8] = include_bytes!("fixtures/fade.dxbc");
    let shader = executor(FADE);
    let program = World::default().spawn();
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let roles: BTreeMap<_, _> = resolution
        .stage
        .constants
        .iter()
        .map(|c| (c.name.clone().unwrap(), (c.offset, c.size, c.role.clone())))
        .collect();
    let object = |p| ConstantRole::Object(p);
    for (name, offset, parameter) in [
        ("FC_isDitherAlpha", 0, ObjectParameter::DitherAlphaEnabled),
        (
            "FC_isEnableCameraDitherFade",
            0,
            ObjectParameter::CameraFadeEnabled,
        ),
        ("FC_DitherEnable", 0, ObjectParameter::DitherEnabled),
        ("FC_CameraFadeEnd", 0, ObjectParameter::CameraFadeEnd),
        (
            "FC_CameraFadeInvRange",
            0,
            ObjectParameter::CameraFadeInverseRange,
        ),
    ] {
        let (_, size, role) = &roles[name];
        assert_eq!((*size, role), (4, &object(parameter)), "{name} at {offset}");
    }
}

/// The scene depth a forward program reads (fog, foam, water: the hardware depth at `t56`) is the frame's own image, bound as a
/// frame texture the consumer supplies; the other engine textures stay counted zeros.
#[test]
fn the_scene_depth_is_a_frame_texture() {
    const FORWARD: &[u8] = include_bytes!("fixtures/forward.dxbc");
    let shader = executor(FORWARD);
    let program = World::default().spawn();
    let resolution = resolve(
        &shader,
        program,
        &material().view(),
        primase_fromsoft::Game::Sekiro,
        None,
        Unbound::Preview,
        &mut |_, _| Ok(None),
    )
    .unwrap();
    let source = |name: &str| {
        let resource = shader
            .interface()
            .resources
            .iter()
            .find(|r| r.provenance.name == name)
            .unwrap();
        resolution
            .stage
            .textures
            .iter()
            .find(|t| t.resource == resource.id)
            .unwrap()
            .source
            .clone()
    };
    assert_eq!(
        source("g_DepthTexture"),
        TextureSource::Frame(FrameTexture::SceneDepth)
    );
    assert!(matches!(source("g_SSAOTexture"), TextureSource::Default { value, .. } if value == [0.; 4]));
}
