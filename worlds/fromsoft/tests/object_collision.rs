//! Object bodies against an independent Havok reader: `oracles.soulsformats.sekiro/objects` holds the game's object
//! `.hkx` files and, for each, Havoc's reading of it (`<name>.hkx.json`: every object the file holds, its class and
//! fields as the file's own type information names them, floats as their bits). Every body's name, motion type and
//! shape class must be Havoc's; a convex shape's stored core (vertices, planes, faces, convex radius), a capsule's
//! segment and the body's pose must be Havoc's bit for bit; and the convex tessellation must lie on Havoc's planes
//! offset by the radius (the intersection [`primase_fromsoft::collision::BodyGeometry::triangles`] claims), within
//! 0.1 mm. A compressed mesh's triangles and materials have no independent reference (Havoc gives the mesh tree as
//! stored, and no third-party decoder reads Sekiro's Havok 2016.2): those bodies are counted as uncovered.
//!
//! Havoc's reading gives a name's UTF-8 bytes as one character each (`本` read as `æ\u{9c}¬`): such a name is compared
//! with its characters taken back to bytes and read as UTF-8, and counted.
mod inputs;

use inputs::{Game, OracleSet, oracle};
use primase_fromsoft::collision::{self, BodyGeometry};
use serde_json::Value;
use std::fs;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A float stored as its bits in hex.
fn float(v: &Value) -> Result<f32> {
    let s = v.as_str().ok_or("a float that is not hex bits")?;
    Ok(f32::from_bits(u32::from_str_radix(s, 16)?))
}

/// A vector of floats stored as bits.
fn floats(v: &Value) -> Result<Vec<f32>> {
    v.as_array()
        .ok_or("a vector that is not an array")?
        .iter()
        .map(float)
        .collect()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// A name as Havoc's reading gives it, its characters taken back to the bytes they widen: `None` for a name with a
/// character past one byte or bytes that are not UTF-8.
fn narrowed(widened: &str) -> Option<String> {
    let bytes = widened
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// The object a field's pointer names.
fn deref<'a>(objects: &'a [Value], pointer: &Value) -> Result<&'a Value> {
    let i = pointer["ref"].as_u64().ok_or("a pointer without ref")? as usize;
    Ok(objects.get(i).ok_or("a pointer past the objects")?)
}

/// `q * v * q⁻¹` for a unit quaternion `q` (x, y, z, w), in f64.
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = q;
    let t = [
        2. * (y * v[2] - z * v[1]),
        2. * (z * v[0] - x * v[2]),
        2. * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// Havoc's planes of a shape (`n . x + w = 0` on a face, outward `n`) moved to the file frame by the body's pose.
fn posed_planes(planes: &[Vec<f32>], position: [f64; 3], orientation: [f64; 4]) -> Vec<([f64; 3], f64)> {
    planes
        .iter()
        .map(|p| {
            let n = rotate(orientation, [p[0], p[1], p[2]].map(f64::from));
            let w = f64::from(p[3]) - (n[0] * position[0] + n[1] * position[1] + n[2] * position[2]);
            (n, w)
        })
        .collect()
}

#[derive(Default)]
struct Census {
    files: usize,
    bodies: usize,
    convex: usize,
    exact_cores: usize,
    probes: usize,
    worst: f64,
    widened_names: usize,
    uncovered_meshes: usize,
    uncovered_triangles: usize,
    extern_instances: usize,
}

#[test]
fn castle_object_bodies() -> Result<()> {
    let Some(dir) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "objects") else {
        return Ok(());
    };
    let mut files = fs::read_dir(&dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    let mut c = Census::default();
    let mut failures = Vec::new();
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "hkx")) {
        let ours =
            collision::read_bodies(&fs::read(path)?, None).map_err(|e| format!("{}: {e}", path.display()))?;
        let dump: Value = serde_json::from_slice(&fs::read(path.with_extension("hkx.json"))?)?;
        assert_eq!(dump["sdk"], "20160200", "{}", path.display());
        let objects = dump["objects"].as_array().ok_or("objects")?;
        // Havoc's bodies, in the file's order: every system of every physics scene
        let mut theirs = Vec::new();
        for o in objects.iter().filter(|o| o["class"] == "hknpPhysicsSceneData") {
            for system in o["fields"]["systemDatas"].as_array().ok_or("systemDatas")? {
                let system = deref(objects, system)?;
                theirs.extend(system["fields"]["bodyCinfos"].as_array().ok_or("bodyCinfos")?);
            }
        }
        let mut next = ours.iter().peekable();
        for body in theirs {
            let f = &body["fields"];
            let name = f["name"].as_str().ok_or("body name")?;
            let motion = f["motionType"].as_u64().ok_or("motionType")?;
            let shape = deref(objects, &f["shape"])?;
            let class = shape["class"].as_str().ok_or("shape class")?;
            let at = format!("{} {name}", path.display());
            c.bodies += 1;
            // a compressed mesh is our mesh body (where it has triangles) and its extern shapes' bodies
            let run: Vec<_> = if class.contains("CompressedMesh") {
                let mut run = Vec::new();
                if let Some(b) = next.next_if(|b| matches!(b.geometry, BodyGeometry::Mesh(_))) {
                    run.push(b);
                }
                while let Some(b) = next.next_if(|b| {
                    matches!(
                        b.geometry,
                        BodyGeometry::Convex {
                            instance: Some(_),
                            ..
                        }
                    )
                }) {
                    run.push(b);
                }
                run
            } else {
                next.next().into_iter().collect()
            };
            if run.is_empty() {
                failures.push(format!("{at}: no body of ours"));
                continue;
            }
            let widened = run[0].name != name && narrowed(name).as_deref() == Some(run[0].name.as_str());
            c.widened_names += usize::from(widened);
            let name = if widened { run[0].name.as_str() } else { name };
            for b in &run {
                if b.name != name || b.motion_type != motion {
                    failures.push(format!(
                        "{at}: ours {:?} motion {}, Havoc's motion {motion}",
                        b.name, b.motion_type
                    ));
                }
            }
            if class.contains("CompressedMesh") {
                if run[0].shape != class && !matches!(run[0].geometry, BodyGeometry::Convex { .. }) {
                    failures.push(format!("{at}: shape {} where Havoc reads {class}", run[0].shape));
                }
                for b in &run {
                    match &b.geometry {
                        BodyGeometry::Mesh(soup) => {
                            c.uncovered_meshes += 1;
                            c.uncovered_triangles += soup.triangles.len();
                        }
                        BodyGeometry::Convex { .. } => c.extern_instances += 1,
                    }
                }
                continue;
            }
            let ours = run[0];
            if ours.shape != class {
                failures.push(format!("{at}: shape {} where Havoc reads {class}", ours.shape));
                continue;
            }
            let BodyGeometry::Convex {
                core,
                capsule,
                instance: None,
                position,
                orientation,
            } = &ours.geometry
            else {
                failures.push(format!(
                    "{at}: a {class} that is not a placed convex body of ours"
                ));
                continue;
            };
            c.convex += 1;
            let s = &shape["fields"];
            let vertices = s["vertices"]
                .as_array()
                .ok_or("vertices")?
                .iter()
                .map(floats)
                .collect::<Result<Vec<_>>>()?;
            let planes = s["planes"]
                .as_array()
                .ok_or("planes")?
                .iter()
                .map(floats)
                .collect::<Result<Vec<_>>>()?;
            let indices: Vec<usize> = s["indices"]
                .as_array()
                .ok_or("indices")?
                .iter()
                .map(|i| i.as_u64().map(|i| i as usize).ok_or("index"))
                .collect::<std::result::Result<_, _>>()?;
            let faces = s["faces"]
                .as_array()
                .ok_or("faces")?
                .iter()
                .map(|face| {
                    let first = face["fields"]["firstIndex"].as_u64().ok_or("firstIndex")? as usize;
                    let n = face["fields"]["numIndices"].as_u64().ok_or("numIndices")? as usize;
                    Ok(indices
                        .get(first..first + n)
                        .ok_or("a face past the indices")?
                        .to_vec())
                })
                .collect::<Result<Vec<_>>>()?;
            let radius = float(&s["convexRadius"])?;
            let p = floats(&f["position"])?;
            let q = floats(&f["orientation"])?;
            let stored_core = core.vertices.iter().map(|v| bits(v)).collect::<Vec<_>>()
                == vertices.iter().map(|v| bits(&v[..3])).collect::<Vec<_>>()
                && core.planes.iter().map(|p| bits(p)).collect::<Vec<_>>()
                    == planes.iter().map(|p| bits(p)).collect::<Vec<_>>()
                && core.faces == faces
                && core.radius.to_bits() == radius.to_bits();
            let segment = match capsule {
                Some([a, b]) => {
                    bits(a) == bits(&floats(&s["a"])?[..3]) && bits(b) == bits(&floats(&s["b"])?[..3])
                }
                None => s["a"].is_null() && s["b"].is_null(),
            };
            let pose = bits(position) == bits(&p[..3]) && bits(orientation) == bits(&q);
            if !(stored_core && segment && pose) {
                failures.push(format!(
                    "{at}: stored core {stored_core}, capsule segment {segment}, pose {pose} differ from Havoc's"
                ));
                continue;
            }
            c.exact_cores += 1;
            // the tessellation is the core's planes offset by the radius: every vertex on that boundary
            let posed = posed_planes(
                &planes,
                [p[0], p[1], p[2]].map(f64::from),
                [q[0], q[1], q[2], q[3]].map(f64::from),
            );
            for t in ours.geometry.triangles()? {
                for v in t {
                    let v = v.map(f64::from);
                    let residual = posed
                        .iter()
                        .map(|(n, w)| n[0] * v[0] + n[1] * v[1] + n[2] * v[2] + w)
                        .fold(f64::NEG_INFINITY, f64::max)
                        - f64::from(radius);
                    c.probes += 1;
                    c.worst = c.worst.max(residual.abs());
                    if residual.abs() > 1e-4 {
                        failures.push(format!(
                            "{at}: a tessellation vertex {residual} m off the offset planes"
                        ));
                    }
                }
            }
        }
        if next.next().is_some() {
            failures.push(format!("{}: bodies of ours Havoc does not read", path.display()));
        }
        c.files += 1;
    }
    assert!(c.files > 0);
    eprintln!(
        "Havoc: files {}, bodies {} (names Havoc widened byte by byte {}); convex bodies {}, stored cores, segments and \
         poses exact {}; tessellation vertices on the offset planes {} (worst {} m, bound 0.0001 m)",
        c.files, c.bodies, c.widened_names, c.convex, c.exact_cores, c.probes, c.worst
    );
    eprintln!(
        "uncovered (no independent decoder for Sekiro's compressed meshes): mesh bodies {}, triangles {}; extern \
         shape instances {}",
        c.uncovered_meshes, c.uncovered_triangles, c.extern_instances
    );
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    Ok(())
}
