//! One archive entry read as itself ([`GtaSource::read_entry`]) against an independent reader's decoding of the same
//! files, and the structure the entry kinds promise.
//!
//! Oracle (`oracles.codewalker.gta/resources`, CodeWalker.Core 485d56b through a dump harness; schema in
//! `resource_oracle.rs`):
//! - a drawable: its box and sphere bit for bit, its shaders (one material each), and per geometry of the high level
//!   its vertex count and index count (one primitive each, in order);
//! - a texture dictionary: per texture its name, size, levels and data length (one `core.texture` each, in order).
//!
//! Also: the drawable's prototype is the first live archetype's (`gta.archetype`) and its asset (`core.asset`) holds
//! it as a part; a file that is not a drawable, texture dictionary or bound is refused.
//!
//! Needs `installs.gta`, `keys.gta` and `oracles.codewalker.gta/resources`.
use primase_core::asset::{Asset, Role};
use primase_core::{Mesh, Name, Texture, Transform};
use primase_gta::components::collision::GtaBounds;
use primase_gta::components::{GtaArchetype, GtaDrawable, GtaShader};
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, OracleSet, install, keys};
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

/// Standalone vehicle modifications have no map archetype or embedded textures. CodeWalker's InitGtxds preloads
/// vehshare, and TryGetRenderable consults that resident lookup after the local dictionary chain. These stock
/// fragments name vehicle_generic_* maps held there, without a texture-parent relationship of their own.
#[test]
fn standalone_vehicle_modifications_resolve_their_resident_textures() {
    use primase_core::Source;
    use primase_gta::components::GtaParam;

    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let cases = [
        ("polcaracara_wingb.yft", "polcaracara_mods.rpf"),
        ("tulip_bumf_b.yft", "tulip_mods.rpf"),
        ("rapidgt3_bumf_b.yft", "rapidgt3_mods.rpf"),
        ("thrax_hoodbdg.yft", "thrax_mods.rpf"),
        ("kamacho_fbumper_1a.yft", "kamacho_mods.rpf"),
    ];
    for (name, archive) in cases {
        let file = source
            .install()
            .listing()
            .find(|f| f.name == name && f.path.contains(archive))
            .expect("the stock modification");
        let (world, report) = source.read_entry(file.file).expect("the fragment");
        let mut found = std::collections::BTreeSet::new();
        let mut missing = Vec::new();
        for (_, shader) in world.iter::<GtaShader>() {
            for (_, value) in &shader.params {
                if let GtaParam::Texture {
                    name: Some(name),
                    texture,
                } = value
                {
                    if let Some(texture) = texture {
                        found.insert(name.as_str());
                        if name.starts_with("vehicle_generic") {
                            let path = &world.get::<Source>(*texture).expect("texture source").0;
                            assert!(path.contains("\\vehshare.ytd#"), "{name}: {path}");
                        }
                    } else {
                        missing.push(name.clone());
                    }
                }
            }
        }
        println!("{name}: resolved names {found:?}; missing {missing:?}; {report:?}");
        // All five fragments name these four maps. Other references (such as blank or the Tulip mesh maps) are
        // not in the resident dictionaries: they remain reported, outside the resident lookup's coverage.
        for shared in [
            "vehicle_generic_detail2",
            "vehicle_generic_detail2_normal",
            "vehicle_generic_detail_spec",
            "vehicle_genericmud_car",
        ] {
            assert!(found.contains(shared), "{name}: resident {shared} is missing");
        }
    }
}

#[test]
fn drawables_textures_and_the_rest_read_as_themselves() {
    let (Some(dir), Some(keys), Some(resources)) = (
        install(Game::Gta),
        keys(Game::Gta),
        primase_testdata::oracle(OracleSet::CodeWalker, Game::Gta, "resources"),
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

/// A map file read as itself holds the placements a region read puts for that map: for every entity of the map the
/// region read kept, the file read has the same entity (path, index, archetype, flags, tint and transform) with the
/// same prototype (archetype name and map types file), and its LOD parent. The file read is the whole map: no
/// geometry yet (the loading is the region's), the script-managed map that governs it, no entity out of the file's
/// own list. (Both reads decode the file the way `placements.rs` checks against CodeWalker; this checks that the file
/// read places what a region does.)
#[test]
fn a_map_file_places_what_a_region_read_places_of_it() {
    use primase_core::{Instance, Transform};
    use primase_gta::components::GtaEntity;
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let (region, _) = source.read([95.0, -1034.0, 295.0, -834.0], 0.0).expect("region");
    // the first map the region read placed from, and what it placed
    let (_, first) = region.iter::<GtaEntity>().next().expect("an entity");
    let path = first.ymap.clone();
    let from_region: std::collections::BTreeMap<u32, _> = region
        .iter::<GtaEntity>()
        .filter(|(_, g)| g.ymap == path && g.interior.is_none())
        .map(|(e, g)| {
            let proto = region.get::<Instance>(e).expect("a placement").prototype;
            (
                g.index,
                (
                    g.clone(),
                    region.get::<Transform>(e).copied().expect("a transform"),
                    region
                        .get::<GtaArchetype>(proto)
                        .map(|a| (a.name, a.ytyp.clone())),
                ),
            )
        })
        .collect();
    assert!(!from_region.is_empty());

    let file = source.file(&path).expect("the map in the install");
    let (w, report) = source.read_entry(file).expect("the map");
    assert_eq!(report.extension, "ymap");
    assert_eq!(report.region.maps, 1);
    let mut compared = 0;
    for (e, g) in w.iter::<GtaEntity>().filter(|(_, g)| g.interior.is_none()) {
        let Some((want, transform, proto)) = from_region.get(&g.index) else {
            continue;
        };
        let p = w.get::<Instance>(e).expect("a placement").prototype;
        assert_eq!(g, want, "{path}#{}", g.index);
        assert_eq!(w.get::<Transform>(e), Some(transform), "{path}#{}", g.index);
        assert_eq!(
            &w.get::<GtaArchetype>(p).map(|a| (a.name, a.ytyp.clone())),
            proto,
            "{path}#{}",
            g.index
        );
        compared += 1;
    }
    assert_eq!(
        compared,
        from_region.len(),
        "every placement of the region read is in the file read"
    );
    // nothing loaded: no mesh on any prototype
    assert_eq!(w.iter::<Mesh>().count(), 0);
}
