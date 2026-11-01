//! The web format's writer extension ([`primase_web::gltf_extension`]) gives its tiles the extras they carry, through
//! primase-gltf's writer: each object's components with a JSON form that the glTF object does not express, a
//! placement's key among its own components in name order, shadows as `extras["primase.shadows"]` (version 1: a
//! placement's own fields, else its prototype's, and a light's `cast_shadows`; what is not stated is absent), a
//! piece's per-primitive components remapped to the piece, and a data texture's image marked. A component whose view
//! would lose a value is an error naming it.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, Light, LightKind, Mesh, Name, Opaque, OpaqueData, Primitive, Semantic, ShadowCast, Shadows,
    Stream, Topology, Transform, World,
};
use primase_gltf::extend::Extensions;
use primase_gltf::{GroupBy, Piece, Selected, Selection, WriteOptions};
use primase_web::gltf_extension::TileExtension;
use serde_json::{Value, json};

fn quad(material: Option<primase_core::Entity>, indices: Vec<u32>) -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [1., 0., -1.], [0., 0., -1.]]),
        )]),
        indices: Some(indices),
        material,
    }])
}

fn written(w: &World, selection: Option<Selection>, extension: TileExtension) -> primase_gltf::Result<Value> {
    let (bytes, _) = primase_gltf::write(
        w,
        &WriteOptions {
            group_by: GroupBy::None,
            selection,
            extensions: Extensions(vec![Arc::new(extension)]),
            ..WriteOptions::default()
        },
    )?;
    Ok(serde_json::from_slice(primase_glb::parse(&bytes).unwrap().json).unwrap())
}

fn document(w: &World, selection: Option<Selection>, extension: TileExtension) -> Value {
    written(w, selection, extension).unwrap()
}

/// A prototype that casts no shadow, placed twice: once as a shadow proxy that does not receive (its own), once
/// stating nothing (the prototype's); and a light that casts.
fn shadows_world() -> World {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, quad(Some(m), vec![0, 1, 2]));
    w.insert(
        proto,
        Shadows {
            cast: Some(ShadowCast::Off),
            receive: None,
        },
    );
    for (x, shadows) in [
        (
            0.0,
            Some(Shadows {
                cast: Some(ShadowCast::Only),
                receive: Some(false),
            }),
        ),
        (2.0, None),
    ] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3][0] = x;
        w.insert(e, t);
        if let Some(s) = shadows {
            w.insert(e, s);
        }
    }
    let light = w.spawn();
    w.insert(
        light,
        Light {
            kind: LightKind::Point,
            color: [1.0; 3],
            intensity: 10.0,
            range: None,
            cast_shadows: Some(true),
            radius: None,
            specular: None,
        },
    );
    w.insert(light, Transform::IDENTITY);
    w
}

#[test]
fn shadows_are_node_extras_the_placements_else_the_prototypes() {
    let doc = document(&shadows_world(), None, TileExtension::default());
    let nodes = doc["nodes"].as_array().unwrap();
    let placed: Vec<&Value> = nodes.iter().filter(|n| n.get("mesh").is_some()).collect();
    assert_eq!(placed.len(), 2);
    assert_eq!(
        placed[0]["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "only", "receive": false})
    );
    // stating nothing of its own: the prototype's
    assert_eq!(
        placed[1]["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "off"})
    );
    // consumed: not also a generic component in the extras
    assert!(placed[0]["extras"]["primase"].get("core.shadows").is_none());
    assert!(
        doc["materials"][0]
            .get("extras")
            .is_none_or(|x| x.get("primase.shadows").is_none()),
        "materials state none"
    );
    let light = nodes
        .iter()
        .find(|n| n["extensions"].get("KHR_lights_punctual").is_some())
        .unwrap();
    assert_eq!(
        light["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "on"})
    );

    // nothing known writes nothing
    let mut w = shadows_world();
    let ids: Vec<_> = w.iter::<Shadows>().map(|(e, _)| e).collect();
    for e in ids {
        w.remove::<Shadows>(e);
    }
    let lights: Vec<_> = w.iter::<Light>().map(|(e, _)| e).collect();
    for e in lights {
        w.get_mut::<Light>(e).unwrap().cast_shadows = None;
    }
    let text = serde_json::to_string(&document(&w, None, TileExtension::default())).unwrap();
    assert!(!text.contains("primase.shadows"));
}

/// A placement's key among its own components, in name order; the omitted resources not in the scene's extras.
#[test]
fn a_placement_key_sits_among_its_components_and_omitted_resources_stay_out() {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, quad(Some(m), vec![0, 1, 2]));
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    let opaque = |v: Value| Opaque {
        version: 1,
        data: OpaqueData::Json(v),
    };
    w.insert_opaque(placed, "test.a_before", opaque(json!(1)))
        .unwrap();
    w.insert_opaque(placed, "zz.after", opaque(json!(2))).unwrap();
    w.insert_resource(primase_core::Environment {
        sun: None,
        ambient: None,
    });
    let selection = Selection {
        placements: BTreeMap::from([(placed, Selected::default())]),
        ..Selection::default()
    };
    let extension = TileExtension {
        placement_keys: BTreeMap::from([(placed, 7)]),
        omit_resources: ["core.environment".to_owned()].into(),
        ..TileExtension::default()
    };
    let doc = document(&w, Some(selection.clone()), extension);
    let extras = doc["nodes"][0]["extras"]["primase"].as_object().unwrap();
    assert_eq!(
        extras.keys().collect::<Vec<_>>(),
        ["test.a_before", "web.placement_key", "zz.after"]
    );
    assert_eq!(extras["web.placement_key"], json!({"version": 1, "value": 7}));
    assert!(doc["scenes"][0].get("extras").is_none());
    // not omitted: the scene's extras carry it
    let doc = document(&w, Some(selection), TileExtension::default());
    assert!(doc["scenes"][0]["extras"]["primase"]["core.environment"].is_object());
}

/// A dialect's per-primitive data, as a reader would type it.
#[derive(Clone, Debug, PartialEq)]
struct Parts(Vec<String>);
impl primase_core::Component for Parts {
    const NAME: &'static str = "test.parts";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        Ok(Some(json!(self.0)))
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(Parts))
    }
}

#[test]
fn a_piece_keeps_its_prototypes_per_primitive_components_remapped() {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    let three = Mesh::new(
        (0..3)
            .map(|_| quad(Some(m), vec![0, 1, 2, 0, 2, 3]).into_primitives().remove(0))
            .collect(),
    );
    w.insert(proto, three);
    w.insert(proto, Name("slab".into()));
    w.insert(proto, Parts(vec!["a".into(), "b".into(), "c".into()]));
    let mut provenance = primase_core::Provenance::default();
    for item in ["mesh 0", "mesh 1", "mesh 2"] {
        provenance.push_part(primase_core::Origin::new(
            Some("area.bnd"),
            "slab.flver",
            Some(item),
        ));
    }
    w.insert(proto, provenance);
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    // the piece: primitive 2's triangles, then primitive 0's
    let piece = Mesh::new(vec![
        quad(Some(m), vec![0, 1, 2]).into_primitives().remove(0),
        quad(Some(m), vec![0, 2, 3]).into_primitives().remove(0),
    ]);
    let selection = Selection {
        placements: BTreeMap::from([(
            placed,
            Selected {
                piece: Some(Piece {
                    mesh: piece,
                    name: "slab.piece".into(),
                    from: vec![2, 0],
                }),
            },
        )]),
        ..Selection::default()
    };
    let before = format!("{:?}", w.components_json(proto).unwrap());
    let doc = document(&w, Some(selection), TileExtension::default());
    assert_eq!(
        before,
        format!("{:?}", w.components_json(proto).unwrap()),
        "the world is unchanged"
    );
    assert_eq!(doc["meshes"][0]["name"], "slab.piece");
    let extras = &doc["meshes"][0]["extras"]["primase"];
    assert_eq!(extras["test.parts"]["value"], json!(["c", "a"]));
    assert_eq!(extras["core.provenance"]["value"]["parts"], json!([2, 0]));
    assert_eq!(extras["core.provenance"]["value"]["origins"][2]["item"], "mesh 2");
}

/// A component whose JSON view would lose a value (a NaN written as null) is an error naming it, never a quiet null.
#[test]
fn extras_never_lose_a_value() {
    /// A component whose JSON form is one float.
    #[derive(Clone, Debug)]
    struct Sample(f32);
    impl primase_core::Component for Sample {
        const NAME: &'static str = "test.sample";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
        fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
            primase_core::json::to_value(&self.0).map(Some)
        }
    }
    let mut w = shadows_world();
    let (placed, _) = w.iter::<Instance>().next().unwrap();
    w.insert(placed, Sample(f32::NAN));
    let err = written(&w, None, TileExtension::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("test.sample") && err.contains("NaN"), "{err}");
}

/// A data texture's image is marked, so a consumer that codes colour images lossily leaves it whole.
#[test]
fn a_data_textures_image_is_marked() {
    use primase_core::pbr::TextureRef;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let mut w = shadows_world();
    let data = w.spawn();
    w.insert(
        data,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![1, 2, 3, 4]],
            color_space: ColorSpace::Linear,
            role: TexRole::Data,
            normal: None,
        },
    );
    let (m, _) = w.iter::<PbrMaterial>().next().unwrap();
    w.get_mut::<PbrMaterial>(m).unwrap().occlusion_texture = Some(TextureRef::new(data));
    // the set the slot samples
    let (proto, mesh) = w.iter::<Mesh>().map(|(e, m)| (e, m.clone())).next().unwrap();
    let mut primitives = mesh.into_primitives();
    primitives[0]
        .streams
        .insert(Semantic::TexCoord(0), Stream::F32x2(vec![[0.0; 2]; 4]));
    w.insert(proto, Mesh::new(primitives));
    let doc = document(&w, None, TileExtension::default());
    assert_eq!(doc["images"][0]["extras"], json!({"primase.role": "data"}));
}

/// A one-detail `pbr.detail` over 2 by 2 textures, on a material drawn by one placement.
fn detail_world() -> (World, Vec<primase_core::Entity>) {
    use primase_core::detail::{
        Detail, DetailLerp, DetailMix, DetailedMaterial, Frozen, GlossScale, ReflectanceTerm, SurfaceLowering,
    };
    use primase_core::pbr::TextureRef;
    use primase_core::shader::ScalarEncoding;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let mut w = World::new();
    let mut textures = Vec::new();
    let mut texture = |w: &mut World, colour: bool| {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: 2,
                height: 2,
                format: TexFormat::Rgba8Unorm,
                levels: vec![(0..16).collect()],
                color_space: if colour {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                },
                role: if colour { TexRole::Color } else { TexRole::Mask },
                normal: None,
            },
        );
        textures.push(e);
        TextureRef::new(e)
    };
    let base_normal = texture(&mut w, false);
    let mix = DetailLerp {
        base_colour: texture(&mut w, true),
        base_normal,
        base_mask: texture(&mut w, false),
        base_saturation: 0.0,
        details: vec![Detail {
            colour: texture(&mut w, true),
            normal: texture(&mut w, false),
            tint: [1.0; 3],
            gloss_bias: 0.25,
            gloss_scale: GlossScale::BaseNormalZ,
            reflectance: ReflectanceTerm::Bias(0.5),
        }],
        weights: Vec::new(),
        gain: 1.0,
        normal_strength: 0.5,
        colour: [1.0; 3],
        gloss_encoding: ScalarEncoding {
            power: 1,
            root: 1,
            scale_bits: 1.0f32.to_bits(),
        },
        alpha: None,
    };
    let material = w.spawn();
    w.insert(material, Name("scarf".into()));
    w.insert(material, PbrMaterial::default());
    w.insert(
        material,
        DetailedMaterial {
            mix: DetailMix::DetailLerp(mix),
            lowering: SurfaceLowering::GlossReflectance { dielectric_f0: 0.04 },
            frozen: vec![Frozen::FrontFacing],
        },
    );
    let mut mesh = quad(Some(material), vec![0, 1, 2]).into_primitives();
    mesh[0]
        .streams
        .insert(Semantic::TexCoord(0), Stream::F32x2(vec![[0.0; 2]; 4]));
    let proto = w.spawn();
    w.insert(proto, Mesh::new(mesh));
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    (w, textures)
}

/// A `pbr.detail` is written as `PRIMASE_materials_detail_blend` beside its `pbr.material`, each slot a texture of the
/// file with its colour space; one whose texture cannot be written leaves the material its `pbr.material` alone,
/// counted by reason.
#[test]
fn a_detail_blend_is_written_beside_its_pbr_material() {
    let (w, _) = detail_world();
    let extension = Arc::new(TileExtension::default());
    let options = WriteOptions {
        extensions: Extensions(vec![extension.clone()]),
        ..WriteOptions::default()
    };
    let (bytes, report) = primase_gltf::write(&w, &options).unwrap();
    let doc: Value = serde_json::from_slice(primase_glb::parse(&bytes).unwrap().json).unwrap();
    assert_eq!(doc["extensionsUsed"], json!(["PRIMASE_materials_detail_blend"]));
    let x = &doc["materials"][0]["extensions"]["PRIMASE_materials_detail_blend"];
    assert_eq!(x["revision"], primase_core::detail::REVISION);
    assert_eq!(
        (&x["equation"], &x["lowering"]),
        (&json!("detail_lerp"), &json!("gloss_reflectance"))
    );
    assert_eq!(x["baseColorTexture"]["colorSpace"], "srgb");
    assert_eq!(x["baseNormalTexture"]["colorSpace"], "linear");
    assert_eq!(x["details"].as_array().unwrap().len(), 1);
    assert_eq!(x["frozen"], json!([{"frontFacing": {}}]));
    assert_eq!(doc["textures"].as_array().unwrap().len(), 5);
    assert_eq!(extension.counts().detail_materials, 1);
    assert!(!report.components_without_form.contains_key("pbr.detail"));

    // a slot's texture not loaded: the material without the extension, counted
    let (mut w, textures_of) = detail_world();
    w.remove::<primase_core::Texture>(textures_of[3]);
    w.insert(
        textures_of[3],
        primase_core::TextureFile {
            path: "detail.dds".into(),
        },
    );
    let extension = Arc::new(TileExtension::default());
    let options = WriteOptions {
        extensions: Extensions(vec![extension.clone()]),
        ..WriteOptions::default()
    };
    let (bytes, report) = primase_gltf::write(&w, &options).unwrap();
    let doc: Value = serde_json::from_slice(primase_glb::parse(&bytes).unwrap().json).unwrap();
    assert!(doc.get("extensionsUsed").is_none());
    assert_eq!(extension.counts().detail_materials_dropped.len(), 1);
    assert_eq!(report.components_without_form.get("pbr.detail"), Some(&1));
}
