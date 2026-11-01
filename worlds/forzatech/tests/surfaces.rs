//! `lower-forzatech-materials` on hand-built materials of the signage, glass, cable, flag and flipbook families: what each
//! rule makes of its maps, cells and constants, and what it counts as left out.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology, World,
};
use primase_forzatech::components::{
    ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures, Parameter, ShaderLevel,
    Value,
};
use primase_forzatech::passes::LowerForzaMaterials;
use std::collections::BTreeMap;

fn texture(world: &mut World, name: &str, space: ColorSpace, width: u32, texels: &[[u8; 4]]) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name.to_owned()));
    world.insert(
        e,
        Texture {
            width,
            height: u32::try_from(texels.len()).unwrap() / width,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texels.iter().flatten().copied().collect()],
            color_space: space,
            role: TexRole::Other,
            normal: None,
        },
    );
    e
}

fn parameter(name: &str, hash: u32, value: Value) -> Parameter {
    Parameter {
        hash,
        flag_hash: None,
        name: Some(name.to_owned()),
        value,
    }
}

fn material(family: &str, atst: [u8; 2], parameters: Vec<Parameter>) -> ForzaMaterial {
    ForzaMaterial {
        instance: InstanceLevel {
            name: "instance".to_owned(),
            atst,
            xtra: Vec::new(),
            parent: String::new(),
            parameters,
            trailer: None,
        },
        material: Some(MaterialLevel {
            entry: String::new(),
            source: String::new(),
            name: "material".to_owned(),
            atst,
            blst: [0; 5],
            xtra: None,
            shader: String::new(),
            technique: "Standard".to_owned(),
            third: String::new(),
            user_data: Vec::new(),
            parameters: Vec::new(),
            trailer: None,
        }),
        shader: Some(ShaderLevel {
            entry: String::new(),
            source: String::new(),
            family: family.to_owned(),
            declared: Vec::new(),
            constants: Vec::new(),
            textures: Vec::new(),
            samplers: Vec::new(),
            constants_size: 0,
            defaults: Vec::new(),
            trailer: None,
        }),
    }
}

fn tex(name: &str, hash: u32) -> Parameter {
    parameter(
        name,
        hash,
        Value::Texture {
            path: String::new(),
            path_hash: hash,
        },
    )
}

fn bound(path_hash: u32, texture: Entity) -> MaterialTexture {
    MaterialTexture {
        path_hash,
        texture: Some(texture),
    }
}

/// A mesh of one triangle drawn with `material`, carrying the texture coordinate sets `sets`.
fn triangle(world: &mut World, material: Entity, sets: &[u8]) {
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
    );
    for set in sets {
        streams.insert(Semantic::TexCoord(*set), Stream::F32x2(vec![[0.0, 0.0]; 3]));
    }
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }]),
    );
}

fn run(world: &mut World) -> PassLog {
    let mut log = PassLog::default();
    LowerForzaMaterials.run(world, &mut log).unwrap();
    log
}

fn colour(v: [f32; 4]) -> Value {
    Value::Colour(v)
}

/// A colour-masked sign: the atlas cell, the paint through the mask, the sign face's coordinate set.
#[test]
fn signage_paints_the_mask_in_its_cell_on_the_sign_face() {
    let mut world = World::new();
    // a 2 x 1 atlas of two cells; the second cell's mask is full, the first's empty
    let atlas = texture(
        &mut world,
        "atlas",
        ColorSpace::Linear,
        2,
        &[[0, 0, 0, 255], [255, 0, 0, 255]],
    );
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment_signage",
            [1, 0],
            vec![
                tex("Signage_DiffTex", 1),
                parameter("Signage", 10, Value::Int(1)),
                parameter("ColorMask", 11, Value::Int(1)),
                parameter("AtlasImages", 12, Value::Vector2([2.0, 1.0])),
                parameter("AtlasIndex", 13, Value::Vector2([2.0, 1.0])),
                parameter("Signage_BackGroundColor", 14, colour([0.0, 0.0, 0.0, 1.0])),
                parameter("Signage_RColor", 15, colour([1.0, 0.5, 0.2, 1.0])),
                parameter("Signage_GColor", 16, colour([0.0; 4])),
                parameter("Signage_BColor", 17, colour([0.0; 4])),
                parameter("Signage_AColor", 18, colour([0.0; 4])),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(1, atlas)]));
    triangle(&mut world, e, &[0, 3]);
    let log = run(&mut world);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let base = pbr.base_color_texture.unwrap();
    assert_eq!(base.texcoord, 3);
    assert_eq!(base.wrap, [Wrap::Clamp; 2]);
    // cell 2 of 2 columns: scale 1/2, offset 1/2
    assert_eq!(
        base.transform,
        Some(UvTransform {
            offset: [0.5, 0.0],
            scale: [0.5, 1.0],
            rotation: 0.0
        })
    );
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
    let painted = world.get::<Texture>(base.texture).unwrap();
    assert_eq!(painted.color_space, ColorSpace::Srgb);
    // an empty mask is the background; a full one is the red colour (linear 1, 0.5, 0.2 as sRGB bytes)
    assert_eq!(&painted.levels[0][..4], &[0, 0, 0, 255]);
    assert_eq!(&painted.levels[0][4..8], &[255, 188, 124, 255]);
    assert_eq!(log.counts["sign atlases baked as colour maps"], 1);
    assert!(
        log.approximations
            .contains_key("wetness, lighting and scene occlusion (run-time) left out: dry")
    );
}

/// A colour atlas stored sRGB is the base colour as it is.
#[test]
fn signage_colour_atlas_is_used_as_it_is() {
    let mut world = World::new();
    let atlas = texture(&mut world, "atlas", ColorSpace::Srgb, 1, &[[10, 20, 30, 255]]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment_signage",
            [1, 0],
            vec![
                tex("Signage_DiffTex", 1),
                parameter("Signage", 10, Value::Int(1)),
                parameter("ColorMask", 11, Value::Int(0)),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(1, atlas)]));
    triangle(&mut world, e, &[0, 3]);
    run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.base_color_texture.unwrap().texture, atlas);
    assert_eq!(world.get::<Texture>(atlas).unwrap().role, TexRole::Color);
}

/// A primitive without the sign face cannot sample the atlas: the background colour and a counted approximation.
#[test]
fn signage_without_the_sign_face_takes_its_background() {
    let mut world = World::new();
    let atlas = texture(&mut world, "atlas", ColorSpace::Linear, 1, &[[255, 0, 0, 255]]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment_signage",
            [1, 0],
            vec![
                tex("Signage_DiffTex", 1),
                parameter("Signage", 10, Value::Int(1)),
                parameter("ColorMask", 11, Value::Int(1)),
                parameter("Signage_BackGroundColor", 14, colour([0.25, 0.5, 0.75, 1.0])),
                parameter("Signage_RColor", 15, colour([1.0; 4])),
                parameter("Signage_GColor", 16, colour([0.0; 4])),
                parameter("Signage_BColor", 17, colour([0.0; 4])),
                parameter("Signage_AColor", 18, colour([0.0; 4])),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(1, atlas)]));
    triangle(&mut world, e, &[0]);
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert!(pbr.base_color_texture.is_none());
    assert_eq!(pbr.base_color, [0.25, 0.5, 0.75, 1.0]);
    assert_eq!(
        log.approximations["a primitive without the sign face (TEXCOORD3): the background colour, no sign texture"],
        1
    );
}

#[test]
fn pylon_cables_are_their_colour() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        material(
            "pylon_cables",
            [1, 0],
            vec![
                parameter("Color", 1, colour([0.1, 0.2, 0.3, 1.0])),
                parameter("LineInner", 2, Value::Int(1)),
                parameter("LightmapActive", 3, Value::Int(0)),
            ],
        ),
    );
    run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.base_color, [0.1, 0.2, 0.3, 1.0]);
    assert_eq!((pbr.metallic, pbr.roughness), (0.0, 1.0));
    assert!(pbr.double_sided);
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
}

/// The albedo target holds 0 to 1: a colour factor above 1 is stored as 1, counted, and what was brighter stays emission.
#[test]
fn colour_factors_above_one_are_stored_as_one_and_the_excess_is_emission() {
    let mut world = World::new();
    let cable = world.spawn();
    world.insert(
        cable,
        material(
            "pylon_cables",
            [1, 0],
            vec![parameter("Color", 1, colour([4.0, 0.5, 1.0, 1.0]))],
        ),
    );
    let lamp = world.spawn();
    world.insert(
        lamp,
        material(
            "emissiveonly",
            [0, 0],
            vec![
                parameter("Color", 1, colour([3.0, 1.5, 0.5, 1.0])),
                parameter("EmissiveMultiplier", 2, Value::Float(2.0)),
            ],
        ),
    );
    let panel_texture = texture(&mut world, "panel", ColorSpace::Srgb, 1, &[[200, 100, 50, 255]]);
    let panel = world.spawn();
    world.insert(
        panel,
        material(
            "anim_ledflipbookpanel",
            [0, 0],
            vec![
                tex("FlipbookTexture", 1),
                parameter("Brightness", 2, Value::Float(10.0)),
                parameter("XFrames", 3, Value::Float(1.0)),
                parameter("YFrames", 4, Value::Float(1.0)),
            ],
        ),
    );
    world.insert(panel, MaterialTextures(vec![bound(1, panel_texture)]));
    let log = run(&mut world);
    for e in [cable, lamp, panel] {
        let pbr = world.get::<PbrMaterial>(e).unwrap();
        assert!(
            pbr.base_color.iter().all(|x| (0.0..=1.0).contains(x)),
            "{:?}",
            pbr.base_color
        );
    }
    assert_eq!(
        world.get::<PbrMaterial>(cable).unwrap().base_color,
        [1.0, 0.5, 1.0, 1.0]
    );
    // the lamp's emission is its colour times the multiplier, normalised: the brightness is in the strength
    let lamp = world.get::<PbrMaterial>(lamp).unwrap();
    assert_eq!(lamp.base_color, [1.0, 1.0, 0.5, 1.0]);
    assert_eq!(lamp.emissive_strength, 6.0);
    assert_eq!(lamp.emissive, [1.0, 0.5, 1.0 / 6.0]);
    let panel = world.get::<PbrMaterial>(panel).unwrap();
    assert_eq!(panel.emissive_strength, 10.0);
    assert_eq!(
        log.approximations["base colour factor above 1 stored as 1 (the albedo target holds 0 to 1)"],
        3
    );
}

/// The glass transmittance `1 - k * tint` becomes an opacity: `k` times the tint's luminance.
#[test]
fn glass_opacity_is_the_tints_luminance_times_k() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        material(
            "glass",
            [0, 1],
            vec![
                parameter("TintColor", 1, colour([1.0, 1.0, 1.0, 1.0])),
                parameter("Alpha", 2, Value::Float(0.5)),
                parameter("Cloudyness", 3, Value::Float(0.0)),
                parameter("Roughness", 4, Value::Float(0.25)),
                parameter("F0", 5, Value::Float(0.02)),
            ],
        ),
    );
    run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Blend);
    // k = (0.4 * 0.5 + 0.5) * 0.98, luminance 1
    let k = (0.4_f32 * 0.5 + 0.5) * 0.98;
    assert!((pbr.base_color[3] - k).abs() < 1e-5, "{:?}", pbr.base_color);
    assert_eq!(pbr.roughness, 0.25);
    let specular = pbr.specular.unwrap();
    assert!((specular.factor - 0.5).abs() < 1e-6);
}

/// A glass colour texture carries the opacity in its alpha, and a cut-out texel is fully transparent.
#[test]
fn glass_colour_texture_carries_the_opacity() {
    let mut world = World::new();
    // white and black linear texels, the last one cut out
    let colours = texture(
        &mut world,
        "glass",
        ColorSpace::Linear,
        3,
        &[[255, 255, 255, 255], [0, 0, 0, 255], [255, 255, 255, 0]],
    );
    let e = world.spawn();
    world.insert(
        e,
        material(
            "glass",
            [0, 1],
            vec![
                tex("BaseColour", 1),
                parameter("BaseColorTexOnOff", 2, Value::Int(1)),
                parameter("AlphaTestTexOnOff", 3, Value::Int(1)),
                parameter("Cloudyness", 4, Value::Float(0.5)),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(1, colours)]));
    run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let baked = world
        .get::<Texture>(pbr.base_color_texture.unwrap().texture)
        .unwrap();
    // an alpha-tested glass has no Alpha factor: k = 0.98 * (1 - 0.5)
    assert_eq!(baked.levels[0][3], (0.49_f64 * 255.0).round() as u8);
    // a black texel transmits everything, so it is seen by its reflection alone: F0 0.04 (none given) + 0.96 / 21
    assert_eq!(
        baked.levels[0][7],
        ((0.04_f64 + 0.96 / 21.0) * 255.0).round() as u8
    );
    // the cut-out discards the pixel, reflection included
    assert_eq!(baked.levels[0][11], 0);
}

/// The forward pass adds the reflection whatever the transmission, so a clear pane keeps the mean fresnel as coverage.
#[test]
fn glass_never_more_clear_than_its_mean_fresnel() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        material(
            "glass",
            [0, 1],
            vec![
                parameter("TintColor", 1, colour([1.0, 1.0, 1.0, 1.0])),
                parameter("Alpha", 2, Value::Float(1.0)),
                parameter("Cloudyness", 3, Value::Float(1.0)),
                parameter("F0", 4, colour([0.1, 0.1, 0.1, 0.0])),
            ],
        ),
    );
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    // Cloudyness 1 makes the transmittance 1; the coverage is F0 + (1 - F0) / 21
    assert!(
        (f64::from(pbr.base_color[3]) - (0.1 + 0.9 / 21.0)).abs() < 1e-6,
        "{:?}",
        pbr.base_color
    );
    assert_eq!(log.approximations["reflection carried as coverage"], 1);
}

/// The flag's colour is read at `uv0 + DiffuseUVOffset`; its occlusion map (another set) is left out by name.
#[test]
fn anim_flag_offsets_its_diffuse_and_leaves_the_occlusion_out() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, 1, &[[200, 100, 50, 255]]);
    let occlusion = texture(&mut world, "ao", ColorSpace::Linear, 1, &[[128, 0, 0, 255]]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "anim_flag",
            [1, 0],
            vec![
                tex("DiffuseATexture", 1),
                tex("AOTexture", 2),
                parameter("DiffuseUVOffset", 3, Value::Vector2([0.25, 0.5])),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(1, diffuse), bound(2, occlusion)]));
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let base = pbr.base_color_texture.unwrap();
    assert_eq!(
        base.transform,
        Some(UvTransform {
            offset: [0.25, 0.5],
            scale: [1.0, 1.0],
            rotation: 0.0
        })
    );
    assert!(pbr.occlusion_texture.is_none());
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    assert_eq!(
        log.approximations["occlusion map (TEXCOORD1) left out: a material is sampled through one coordinate set"],
        1
    );
}

/// A window is its frame on TEXCOORD2: the colour over the room's back wall mean where its mask is empty, the glass
/// roughness' red, the room's emission left out by name.
#[test]
fn window_is_its_frame_on_the_second_set() {
    let mut world = World::new();
    // an sRGB frame texel of full colour with a mask of 0 (a pane) and one with a mask of 255
    let frame = texture(
        &mut world,
        "frame",
        ColorSpace::Srgb,
        2,
        &[[200, 100, 50, 0], [200, 100, 50, 255]],
    );
    let rough = texture(&mut world, "rough", ColorSpace::Linear, 1, &[[51, 9, 9, 255]]);
    // a room of one cell, 8 x 4 texels: the back wall is the lower tile of the second of four columns
    let mut cell = [[10u8, 10, 10, 255]; 32];
    for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
        cell[y * 8 + x] = [128, 64, 0, 255];
    }
    let room = texture(&mut world, "room", ColorSpace::Srgb, 8, &cell);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "window_linecastroom",
            [0, 0],
            vec![
                tex("FrameDiffuseandMask", 1),
                tex("FrameGlassRoughnessTexture", 2),
                tex("BoxDiffRoughTex", 6),
                parameter("FrameTex", 3, Value::Int(1)),
                parameter("Fittings", 4, Value::Int(1)),
                parameter("Metalness", 5, Value::Float(0.0)),
            ],
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![bound(1, frame), bound(2, rough), bound(6, room)]),
    );
    triangle(&mut world, e, &[0, 1, 2]);
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.base_color, [1.0; 4]);
    let base = pbr.base_color_texture.unwrap();
    assert_eq!(base.texcoord, 2);
    let baked = world.get::<Texture>(base.texture).unwrap();
    // the covered texel keeps its colour, the pane is the back wall's colour, both opaque
    assert_eq!(&baked.levels[0][..4], &[128, 64, 0, 255]);
    assert_eq!(&baked.levels[0][4..8], &[200, 100, 50, 255]);
    let mr = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(mr.texcoord, 2);
    // G is the roughness (red 51), B 1: the constant `Metalness` is the factor
    assert_eq!(
        world.get::<Texture>(mr.texture).unwrap().levels[0],
        vec![255, 51, 255, 255]
    );
    assert_eq!(
        log.approximations["room drawn as its back wall's mean colour (no parallax, furniture and per-placement room extents left out)"],
        1
    );
    assert_eq!(log.approximations["room emission left out"], 1);
    assert_eq!(
        log.approximations["fittings (TEXCOORD1) and their tint left out"],
        1
    );
}
