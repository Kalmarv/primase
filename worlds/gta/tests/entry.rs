//! One archive entry read as itself ([`GtaSource::read_entry`]) against an independent reader's decoding of the same
//! files, and the structure the entry kinds promise.
//!
//! Oracle (`PRIMASE_GTA_RESOURCES`, CodeWalker.Core 485d56b through a dump harness; schema in `resource_oracle.rs`):
//! - a drawable: its box and sphere bit for bit, its shaders (one material each), and per geometry of the high level
//!   its vertex count and index count (one primitive each, in order);
//! - a texture dictionary: per texture its name, size, levels and data length (one `core.texture` each, in order).
//!
//! Also: the drawable's prototype is the first live archetype's (`gta.archetype`) and its asset (`core.asset`) holds
//! it as a part; a file that is not a drawable, texture dictionary or bound is refused.
//!
//! Needs `PRIMASE_GTA_DIR`, `PRIMASE_GTA_KEYS` and `PRIMASE_GTA_RESOURCES`.
use primase_core::asset::{Asset, Role};
use primase_core::{Mesh, Name, Texture, Transform};
use primase_gta::components::collision::GtaBounds;
use primase_gta::components::{GtaArchetype, GtaDrawable, GtaShader};
use primase_gta::read::source::GtaSource;
use primase_testdata::real_data;
use serde_json::Value;

fn oracle(dir: &std::path::Path, file: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(file)).expect("oracle file")).expect("oracle json")
}

fn f32s(v: &Value) -> Vec<u32> {
    v.as_array()
        .expect("floats")
        .iter()
        .map(|x| (x.as_f64().expect("float") as f32).to_bits())
        .collect()
}

#[test]
fn drawables_textures_and_the_rest_read_as_themselves() {
    let (Some(dir), Some(keys), Some(resources)) = (
        real_data("PRIMASE_GTA_DIR"),
        real_data("PRIMASE_GTA_KEYS"),
        real_data("PRIMASE_GTA_RESOURCES"),
    ) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");

    // a drawable
    let o = oracle(&resources, "dt1_23_build2.ydr.json");
    let file = source
        .file(o["path"].as_str().expect("path"))
        .expect("the drawable in the install");
    let (w, report) = source.read_entry(file).expect("the drawable");
    let v = &o["value"];
    assert_eq!(report.extension, "ydr");
    assert_eq!(
        (report.archetypes_found, report.archetypes_made),
        (1, 0),
        "{report:?}"
    );
    let protos: Vec<_> = w.iter::<GtaDrawable>().collect();
    assert_eq!(protos.len(), 1);
    let (proto, drawable) = protos[0];
    assert_eq!(
        drawable
            .bbox
            .iter()
            .flatten()
            .map(|f| f.to_bits())
            .collect::<Vec<_>>(),
        [f32s(&v["bbox_min"]), f32s(&v["bbox_max"])].concat()
    );
    assert_eq!(
        w.iter::<GtaShader>().count(),
        v["shaders"].as_array().expect("shaders").len()
    );
    let geometries = v["high"][0]["geometries"].as_array().expect("geometries");
    let mesh = w.get::<Mesh>(proto).expect("the high level's mesh");
    assert_eq!(mesh.primitives().len(), geometries.len());
    for (p, g) in mesh.primitives().iter().zip(geometries) {
        assert_eq!(p.vertex_count() as u64, g["vertices"].as_u64().expect("vertices"));
        assert_eq!(
            p.indices.as_ref().map(Vec::len),
            Some(g["indices"].as_u64().expect("indices") as usize)
        );
    }
    assert!(w.get::<GtaArchetype>(proto).is_some_and(|a| !a.ytyp.is_empty()));
    let asset = w.iter::<Asset>().next().expect("the asset").1;
    assert_eq!(asset.members, [(proto, Role::Part)]);
    assert_eq!(w.get::<Name>(proto).map(|n| n.0.as_str()), Some("dt1_23_build2"));
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "{} components fail verification, e.g. {:?}",
        bad.len(),
        bad.first()
    );
    println!("{report:?}");

    // a texture dictionary
    let o = oracle(&resources, "dt1_23_build.ytd.json");
    let file = source
        .file(o["path"].as_str().expect("path"))
        .expect("the dictionary in the install");
    let (w, report) = source.read_entry(file).expect("the dictionary");
    let textures = o["value"]["textures"].as_array().expect("textures");
    assert_eq!(
        (report.entries, report.textures),
        (textures.len(), textures.len())
    );
    let ours: Vec<_> = w
        .iter::<Texture>()
        .map(|(e, t)| {
            (
                w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default(),
                t.width,
                t.height,
                t.levels.len(),
                t.levels.iter().map(Vec::len).sum::<usize>(),
            )
        })
        .collect();
    let theirs: Vec<_> = textures
        .iter()
        .map(|t| {
            (
                t["name"].as_str().expect("name").to_owned(),
                t["width"].as_u64().expect("width") as u32,
                t["height"].as_u64().expect("height") as u32,
                t["levels"].as_u64().expect("levels") as usize,
                t["data_len"].as_u64().expect("data length") as usize,
            )
        })
        .collect();
    assert_eq!(ours, theirs);

    // static collision: one bound at the origin, the finer copies named by their prefix, and the material names (the
    // bound itself is the decoder `embedded_bounds.rs` checks against CodeWalker; no oracle lists map bounds)
    for finer in [false, true] {
        let ybn = source
            .install()
            .listing()
            .find(|f| f.extension == "ybn" && f.name.contains('@') == finer)
            .expect("a ybn file");
        let (w, report) = source.read_entry(ybn.file).expect("the bound");
        assert_eq!(
            (report.bounds.ybn_read, report.bounds.ybn_finer_read),
            (1, usize::from(finer))
        );
        let bounds: Vec<_> = w.iter::<GtaBounds>().collect();
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].1.path, ybn.path);
        assert_eq!(bounds[0].1.detail.is_some(), finer);
        assert_eq!(w.get::<Transform>(bounds[0].0), Some(&Transform::IDENTITY));
        assert!(report.bounds.material_names > 0);
    }

    // a kind the reader has no way to read as itself
    let readme = source
        .install()
        .listing()
        .find(|f| f.extension == "xml")
        .expect("an xml file");
    assert!(source.read_entry(readme.file).is_err());
}
