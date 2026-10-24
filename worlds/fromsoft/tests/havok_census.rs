//! Elden Ring's Havok 2018 collision against an independent reader: HKLib's reading of the same files (`havok.jsonl` of
//! `oracles.soulsformats.elden/format`, the oracle harness's `havok` mode: the map collision of chosen tiles with their
//! compendium, and chosen assets' collision). For every compressed mesh shape a file holds, the mesh tree's sections as
//! stored (codec parameters as f32 bits, first packed vertex, packed vertex count, first shared vertex, first
//! primitive, primitive count, read through the 2018 section layout), its packed and shared vertices, shared-vertex
//! index and primitives, the shape's triangle-to-shape-key table, and its extern shape instances (the transform's
//! sixteen stored floats and the scale as f32 bits, the instanced shape's class, convex radius and stored vertices),
//! compared exactly. The shapes of a file are compared as a set (each reader walks the object graph its own way).
//! Nothing here checks the triangle decode; the stored data it starts from is what is compared, and each extern
//! instance's transform as [`primase_havok::extern_shapes`] reads it is checked against its stored floats.
use primase_fromsoft::{Binder, Game, dcx::Decoder};
use primase_havok::{Compendium, Tagfile, Value, mesh_sections};

use serde_json::{Value as Json, json};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// A member's name without its folder and `.dcx` suffix.
fn leaf(name: &str) -> &str {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    name.strip_suffix(".dcx").unwrap_or(name)
}

/// One compressed mesh shape in the oracle's record shape.
fn shape_record(shape: Value<'_, '_>) -> Result<Json> {
    let data = shape.field("data")?.dereference()?;
    let tree = data.field("meshTree")?;
    let unsigned = |name: &str| -> Result<Vec<u64>> {
        let a = tree.field(name)?.array()?;
        (0..a.len()).map(|i| Ok(a.get(i)?.unsigned()?)).collect()
    };
    let primitives = tree.field("primitives")?.array()?;
    let primitives = (0..primitives.len())
        .map(|i| {
            let indices = primitives.get(i)?.field("indices")?.array()?;
            (0..indices.len())
                .map(|j| Ok(indices.get(j)?.unsigned()?))
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    let keys = if shape.type_name() == "fsnpCustomParamCompressedMeshShape" {
        let a = shape.field("triangleIndexToShapeKey")?.array()?;
        Json::from(
            (0..a.len())
                .map(|i| Ok(a.get(i)?.unsigned()?))
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        Json::Null
    };
    let bits = |v: &[f32]| {
        v.iter()
            .map(|f| format!("{:08x}", f.to_bits()))
            .collect::<Vec<_>>()
    };
    let stored = shape.field("externShapes")?.array()?;
    let mut externs = Vec::new();
    for i in 0..stored.len() {
        let instance = stored.get(i)?;
        let inner = instance.field("shape")?.dereference()?;
        let vertices = match inner.field("vertices") {
            Ok(v) => {
                let v = v.array()?;
                Json::from(
                    (0..v.len())
                        .map(|j| Ok(bits(&v.get(j)?.vector::<4>()?)))
                        .collect::<Result<Vec<_>>>()?,
                )
            }
            Err(_) => Json::Null,
        };
        externs.push(json!({
            "transform": bits(&instance.field("transform")?.vector::<16>()?),
            "scale": bits(&instance.field("scale")?.vector::<4>()?),
            "kind": inner.type_name(),
            "convex_radius": format!("{:08x}", inner.field("convexRadius")?.float()?.to_bits()),
            "vertices": vertices,
        }));
    }
    // the instances' transforms as the reader takes them: each column's xyz of the stored words
    let mesh = primase_havok::compressed_mesh(data)?;
    for (i, instance) in primase_havok::extern_shapes(shape, &mesh)?.iter().enumerate() {
        let t = stored.get(i)?.field("transform")?.vector::<16>()?;
        if (0..4).any(|c| instance.transform[c] != [t[4 * c], t[4 * c + 1], t[4 * c + 2]]) {
            return Err(format!("extern shape {i}: transform read otherwise than stored").into());
        }
    }
    let sections: Vec<Json> = mesh_sections(data)?
        .iter()
        .map(|s| {
            json!({
                "codec": s.codec.iter().map(|f| format!("{:08x}", f.to_bits())).collect::<Vec<_>>(),
                "first_packed_vertex": s.first_packed_vertex,
                "packed_vertices": s.packed_vertices,
                "first_shared_vertex": s.first_shared_vertex,
                "first_primitive": s.first_primitive,
                "primitives": s.primitives,
            })
        })
        .collect();
    Ok(json!({
        "kind": shape.type_name(),
        "bits_per_key": tree.field("bitsPerKey")?.signed()?,
        "sections": sections,
        "packed_vertices": unsigned("packedVertices")?,
        "shared_vertices": unsigned("sharedVertices")?,
        "shared_vertices_index": unsigned("sharedVerticesIndex")?,
        "primitives": primitives,
        "triangle_to_shape_key": keys,
        "extern_shapes": externs,
    }))
}

/// Every compressed mesh shape the tagfile holds, as records, sorted.
fn shapes(bytes: &[u8], compendium: Option<&Compendium>) -> Result<Vec<String>> {
    let tag = Tagfile::read(bytes, compendium)?;
    let mut out = Vec::new();
    for index in 1..tag.items.len() {
        let item = tag.item(index)?;
        for j in 0..item.len() {
            let value = item.get(j)?;
            if matches!(
                value.type_name(),
                "fsnpCustomParamCompressedMeshShape" | "hknpCompressedMeshShape"
            ) {
                out.push(shape_record(value)?.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

#[test]
fn compressed_meshes_match_an_independent_reading() -> Result<()> {
    let (Some(archive), Some(oracle), Some(dll)) = (
        common::archive(Game::EldenRing)?,
        primase_testdata::oracle(
            primase_testdata::OracleSet::SoulsFormats,
            primase_testdata::Game::Elden,
            "format",
        ),
        primase_testdata::oodle(primase_testdata::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let (mut files, mut compared, mut sections) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    let mut binders =
        std::collections::BTreeMap::<String, (Vec<primase_fromsoft::BinderFile>, Option<Compendium>)>::new();
    for line in std::fs::read_to_string(oracle.join("havok.jsonl"))?.lines() {
        let record: Json = serde_json::from_str(line)?;
        let file = record["file"].as_str().ok_or("file")?;
        let (binder, member) = file.rsplit_once(':').ok_or("file")?;
        if !binders.contains_key(binder) {
            let members = match binder.strip_suffix(".hkxbhd") {
                Some(stem) => {
                    Binder::read_split(
                        &archive.read(binder, &decoder)?,
                        &archive.read(&format!("{stem}.hkxbdt"), &decoder)?,
                        &decoder,
                    )?
                    .files
                }
                None => Binder::read(&archive.read(binder, &decoder)?, &decoder)?.files,
            };
            let compendium = match members
                .iter()
                .find(|f| leaf(f.name.as_deref().unwrap_or("")).ends_with(".compendium"))
            {
                Some(f) => Some(Compendium::read(&decoder.decode_if_dcx(&f.bytes)?)?),
                None => None,
            };
            binders.insert(binder.to_owned(), (members, compendium));
        }
        let (members, compendium) = &binders[binder];
        let bytes = members
            .iter()
            .find(|f| f.name.as_deref().is_some_and(|n| leaf(n) == leaf(member)))
            .ok_or_else(|| format!("{file}: not in the binder"))?;
        let bytes = decoder.decode_if_dcx(&bytes.bytes)?;
        files += 1;
        let ours = match (shapes(&bytes, compendium.as_ref()), record.get("error")) {
            (Ok(_), Some(e)) => {
                failures.push(format!("{file}: read, the reference refuses: {e}"));
                continue;
            }
            (Err(e), None) => {
                failures.push(format!("{file}: the reference reads it, ours refuses: {e}"));
                continue;
            }
            (Err(e), Some(theirs)) => {
                println!("AGREED REJECTION {file}: ours {e}; reference {theirs}");
                continue;
            }
            (Ok(ours), None) => ours,
        };
        let mut theirs: Vec<String> = record["shapes"]
            .as_array()
            .ok_or("shapes")?
            .iter()
            .map(Json::to_string)
            .collect();
        theirs.sort();
        if ours.len() != theirs.len() {
            failures.push(format!(
                "{file}: {} shapes, the reference {}",
                ours.len(),
                theirs.len()
            ));
            continue;
        }
        for (o, t) in ours.iter().zip(&theirs) {
            compared += 1;
            sections += serde_json::from_str::<Json>(o)?["sections"]
                .as_array()
                .map_or(0, Vec::len);
            if serde_json::from_str::<Json>(o)? != serde_json::from_str::<Json>(t)? {
                failures.push(format!("{file}: a shape differs"));
            }
        }
    }
    println!(
        "files {files}, compressed mesh shapes {compared}, sections {sections}, mismatches {}",
        failures.len()
    );
    for f in failures.iter().take(20) {
        println!("MISMATCH {f}");
    }
    assert!(files > 0 && failures.is_empty());
    Ok(())
}
