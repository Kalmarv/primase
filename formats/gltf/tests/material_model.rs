//! Unlit shading is written as `KHR_materials_unlit`, a texture's stated filters as its sampler's, and both read back
//! as they were through `primase_glb`'s helpers; a filter glTF cannot express is counted, not dropped silently.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::{Filter, Filters, MipFilter, PbrMaterial, Shading, TextureRef, Wrap};
use primase_core::{ColorSpace, Entity, Mesh, TexFormat, TexRole, Texture, Transform, World};
use serde_json::Value;

mod common;

fn texture(w: &mut World) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255, 0, 0, 255]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    e
}

fn world(shading: Shading, filters: Filters) -> (World, Entity) {
    let mut w = World::new();
    let t = texture(&mut w);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef {
                filters,
                wrap: [Wrap::Clamp, Wrap::Repeat],
                ..TextureRef::new(t)
            }),
            shading,
            ..PbrMaterial::default()
        },
    );
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![common::rich_primitive(Some(m), 0.0)]));
    w.insert(e, Transform::IDENTITY);
    (w, m)
}

fn write(w: &World) -> (Value, primase_gltf::WriteReport) {
    let (bytes, report) = primase_gltf::write(w, &primase_gltf::WriteOptions::default()).unwrap();
    let chunks = primase_glb::parse(&bytes).unwrap();
    (serde_json::from_slice(chunks.json).unwrap(), report)
}

/// The base colour texture's sampler, read back.
fn sampler(doc: &Value) -> (Filters, [Wrap; 2]) {
    let t = &doc["textures"][doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["index"]
        .as_u64()
        .unwrap() as usize];
    primase_glb::sampler_filters(&doc["samplers"][t["sampler"].as_u64().unwrap() as usize]).unwrap()
}

#[test]
fn unlit_is_written_and_read_back() {
    let none = Filters::default();
    let (w, _) = world(Shading::Unlit, none);
    let (doc, _) = write(&w);
    let m = &doc["materials"][0];
    assert_eq!(primase_glb::material_shading(m).unwrap(), Shading::Unlit);
    assert!(
        doc["extensionsUsed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_unlit")
    );
    assert!(
        doc.get("extensionsRequired").is_none(),
        "a viewer without it draws the fallback"
    );
    let (w, _) = world(Shading::Lit, none);
    let (doc, _) = write(&w);
    assert_eq!(
        primase_glb::material_shading(&doc["materials"][0]).unwrap(),
        Shading::Lit
    );
    assert!(doc["materials"][0].get("extensions").is_none());
}

#[test]
fn stated_filters_are_the_samplers_and_read_back() {
    for filters in [
        Filters {
            mag: Some(Filter::Nearest),
            min: Some(Filter::Nearest),
            mip: Some(MipFilter::Nearest),
        },
        Filters {
            mag: Some(Filter::Linear),
            min: Some(Filter::Linear),
            mip: Some(MipFilter::Linear),
        },
        Filters {
            mag: Some(Filter::Nearest),
            min: Some(Filter::Linear),
            mip: Some(MipFilter::None),
        },
        Filters::default(),
    ] {
        let (w, _) = world(Shading::Lit, filters);
        let (doc, report) = write(&w);
        assert_eq!(
            sampler(&doc),
            (filters, [Wrap::Clamp, Wrap::Repeat]),
            "{filters:?}"
        );
        assert_eq!(report.filters_unexpressed, 0);
    }
    // NEAREST magnification, NEAREST_MIPMAP_NEAREST minification: the codes themselves
    let (w, _) = world(
        Shading::Lit,
        Filters {
            mag: Some(Filter::Nearest),
            min: Some(Filter::Nearest),
            mip: Some(MipFilter::Nearest),
        },
    );
    let (doc, _) = write(&w);
    assert_eq!(doc["samplers"][0]["magFilter"], 9728);
    assert_eq!(doc["samplers"][0]["minFilter"], 9984);
}

/// glTF's minFilter names both the level filter and the mip filter: a texture stating one of them only has none
/// written, counted.
#[test]
fn a_filter_gltf_cannot_express_is_counted() {
    let (w, _) = world(
        Shading::Lit,
        Filters {
            mag: Some(Filter::Nearest),
            min: Some(Filter::Nearest),
            mip: None,
        },
    );
    let (doc, report) = write(&w);
    assert_eq!(report.filters_unexpressed, 1);
    let (filters, _) = sampler(&doc);
    assert_eq!(
        (filters.mag, filters.min, filters.mip),
        (Some(Filter::Nearest), None, None)
    );
}
