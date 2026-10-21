//! The reader against Khronos glTF-Sample-Assets (`PRIMASE_KHRONOS_GLTF`: one folder per model, the model's `.glb` or
//! `.gltf` with its buffers, and `expected.json` that pygltflib wrote from the same file). The files are third-party
//! and independent of this reader: every mesh must read, every attribute and morph target must have the element count
//! and, where `expected.json` gives them, the bounds pygltflib saw, the default weights, the animation samplers'
//! counts and time range, and the skins' inverse bind matrices must match.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use primase_core::{Semantic, Stream};
use primase_glb::{ElementType, Gltf};
use serde_json::Value;

/// What a model exercised, counted from `expected.json` and the reader's output.
#[derive(Default, Debug)]
struct Seen {
    meshes: usize,
    sparse_accessors: usize,
    skin_primitives: usize,
    target_sets: usize,
    default_weights: usize,
    channels: usize,
    skins: usize,
    matrices: usize,
}

fn load(dir: &std::path::Path, name: &str) -> Gltf {
    let glb = dir.join(format!("{name}.glb"));
    if glb.exists() {
        return Gltf::from_glb(&std::fs::read(glb).unwrap()).unwrap_or_else(|e| panic!("{name}.glb: {e}"));
    }
    let json = std::fs::read(dir.join(format!("{name}.gltf"))).unwrap();
    Gltf::from_gltf(&json, |_, uri| {
        std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|e| panic!("{name}.gltf: {e}"))
}

fn semantic(name: &str) -> Option<Semantic> {
    let n = |p: &str| name.strip_prefix(p).and_then(|n| n.parse().ok());
    match name {
        "POSITION" => Some(Semantic::Position),
        "NORMAL" => Some(Semantic::Normal),
        "TANGENT" => Some(Semantic::Tangent),
        _ => n("TEXCOORD_")
            .map(Semantic::TexCoord)
            .or_else(|| n("COLOR_").map(Semantic::Color))
            .or_else(|| n("JOINTS_").map(Semantic::Joints))
            .or_else(|| n("WEIGHTS_").map(Semantic::Weights)),
    }
}

/// The stream's components, element after element, widened to f64.
fn flat(s: &Stream) -> (usize, Vec<f64>) {
    fn widen<T: Copy + Into<f64>, const N: usize>(v: &[[T; N]]) -> (usize, Vec<f64>) {
        (N, v.iter().flatten().map(|&c| c.into()).collect())
    }
    match s {
        Stream::F32x2(v) => widen(v),
        Stream::F32x3(v) => widen(v),
        Stream::F32x4(v) => widen(v),
        Stream::U8x4(v) => widen(v),
        Stream::U16x4(v) => widen(v),
    }
}

fn numbers(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// Component-wise min and max of `data`, `n` components per element, equal to `expected`'s `min` and `max` (exactly:
/// FLOAT and unnormalized integers are as stored) when it gives them.
fn check_bounds(what: &str, expected: &Value, n: usize, data: &[f64]) {
    let (min, max) = (numbers(&expected["min"]), numbers(&expected["max"]));
    if min.is_empty() && max.is_empty() {
        return;
    }
    assert!(
        !expected["normalized"].as_bool().unwrap(),
        "{what}: bounds of a normalized accessor"
    );
    assert_eq!((min.len(), max.len()), (n, n), "{what}");
    for c in 0..n {
        let column = data.iter().skip(c).step_by(n);
        let lo = column.clone().copied().fold(f64::INFINITY, f64::min);
        let hi = column.copied().fold(f64::NEG_INFINITY, f64::max);
        // the file states its bounds as f32 values, written out in decimal
        assert_eq!(
            (lo as f32, hi as f32),
            (min[c] as f32, max[c] as f32),
            "{what}: component {c}"
        );
    }
}

fn check_model(root: &std::path::Path, name: &str) -> Seen {
    let dir = root.join(name);
    let expected: Value = serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
    let g = load(&dir, name);
    let mut seen = Seen::default();

    let meshes = expected["meshes"].as_array().unwrap();
    assert_eq!(g.mesh_count(), meshes.len(), "{name}: mesh count");
    for (mi, em) in meshes.iter().enumerate() {
        let mesh = g
            .read_mesh(mi)
            .unwrap_or_else(|e| panic!("{name}: mesh {mi}: {e}"));
        seen.meshes += 1;
        let eprims = em["primitives"].as_array().unwrap();
        assert_eq!(
            mesh.primitives.len(),
            eprims.len(),
            "{name}: mesh {mi} primitives"
        );

        let default_weights = numbers(&em["defaultWeights"]);
        match &mesh.weights {
            None => assert!(
                default_weights.is_empty(),
                "{name}: mesh {mi}: weights {default_weights:?} not read"
            ),
            Some(w) => {
                let w: Vec<f64> = w.iter().map(|&x| f64::from(x)).collect();
                assert_eq!(w, default_weights, "{name}: mesh {mi} default weights");
                seen.default_weights += 1;
            }
        }

        for (pi, ep) in eprims.iter().enumerate() {
            let p = &mesh.primitives[pi];
            let at = format!("{name}: mesh {mi} primitive {pi}");
            for (attr, ea) in ep["attributes"].as_object().unwrap() {
                let count = usize::try_from(ea["count"].as_u64().unwrap()).unwrap();
                if !ea["sparse"].is_null() {
                    seen.sparse_accessors += 1;
                }
                match semantic(attr) {
                    Some(sem) => {
                        let stream = p
                            .primitive
                            .streams
                            .get(&sem)
                            .unwrap_or_else(|| panic!("{at}: {attr} not read"));
                        assert_eq!(stream.len(), count, "{at}: {attr} count");
                        let (n, data) = flat(stream);
                        // normalized streams are dequantized or kept as stored, whichever the reader's rules say:
                        // the bounds are pygltflib's raw values, compared where the data is as stored
                        if !ea["normalized"].as_bool().unwrap() {
                            check_bounds(&format!("{at}: {attr}"), ea, n, &data);
                        }
                    }
                    None => assert_eq!(p.custom[attr].count, count, "{at}: {attr} count"),
                }
            }
            let pairs = ["JOINTS_", "WEIGHTS_"].map(|k| {
                ep["attributes"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .filter(|a| a.starts_with(k))
                    .count()
            });
            assert_eq!(pairs[0], pairs[1], "{at}: joint sets and weight sets");
            seen.skin_primitives += usize::from(pairs[0] > 0);

            let etargets = ep["morphTargets"].as_array().unwrap();
            assert_eq!(p.targets.len(), etargets.len(), "{at}: target count");
            assert_eq!(
                etargets.len(),
                usize::try_from(ep["morphTargetCount"].as_u64().unwrap()).unwrap(),
                "{at}"
            );
            let position_count = p.primitive.streams[&Semantic::Position].len();
            for (ti, et) in etargets.iter().enumerate() {
                let t = &p.targets[ti];
                seen.target_sets += 1;
                let attrs = et.as_object().unwrap();
                for (attr, got) in [
                    ("POSITION", &t.position),
                    ("NORMAL", &t.normal),
                    ("TANGENT", &t.tangent),
                ] {
                    let tat = format!("{at}: target {ti} {attr}");
                    let Some(ea) = attrs.get(attr) else {
                        assert!(got.is_none(), "{tat}: read but not in the file");
                        continue;
                    };
                    let got = got.as_ref().unwrap_or_else(|| panic!("{tat}: not read"));
                    assert_eq!(
                        got.len(),
                        usize::try_from(ea["count"].as_u64().unwrap()).unwrap(),
                        "{tat}"
                    );
                    assert_eq!(got.len(), position_count, "{tat}");
                    if !ea["sparse"].is_null() {
                        seen.sparse_accessors += 1;
                    }
                    if !ea["normalized"].as_bool().unwrap() {
                        let data: Vec<f64> = got.iter().flatten().map(|&c| f64::from(c)).collect();
                        check_bounds(&tat, ea, 3, &data);
                    }
                }
                assert!(
                    attrs
                        .keys()
                        .all(|k| matches!(k.as_str(), "POSITION" | "NORMAL" | "TANGENT")),
                    "{at}"
                );
            }
        }
    }

    // Skins: the inverse bind matrices decode to the values pygltflib read, exactly (FLOAT).
    let skins = g.json()["skins"].as_array().cloned().unwrap_or_default();
    let eskins = expected["skins"].as_array().unwrap();
    assert_eq!(skins.len(), eskins.len(), "{name}: skin count");
    for (si, es) in eskins.iter().enumerate() {
        seen.skins += 1;
        let want: Vec<f64> = es["inverseBindMatrices"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(numbers)
            .collect();
        match skins[si].get("inverseBindMatrices") {
            None => assert!(want.is_empty(), "{name}: skin {si}"),
            Some(a) => {
                let (element, got) = g.floats(usize::try_from(a.as_u64().unwrap()).unwrap()).unwrap();
                assert_eq!(element, ElementType::Mat4, "{name}: skin {si}");
                let got: Vec<f64> = got.iter().map(|&x| f64::from(x)).collect();
                assert_eq!(got, want, "{name}: skin {si} inverse bind matrices");
                seen.matrices += got.len() / 16;
            }
        }
    }

    // Animations: each channel's sampler input and output decode to the counts and time range pygltflib saw.
    let animations = g.json()["animations"].as_array().cloned().unwrap_or_default();
    let eanims = expected["animations"].as_array().unwrap();
    assert_eq!(animations.len(), eanims.len(), "{name}: animation count");
    for (ai, ea) in eanims.iter().enumerate() {
        let echannels = ea["channels"].as_array().unwrap();
        let channels = animations[ai]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), echannels.len(), "{name}: animation {ai} channels");
        for (ci, ec) in echannels.iter().enumerate() {
            let at = format!("{name}: animation {ai} channel {ci}");
            let sampler = &animations[ai]["samplers"]
                [usize::try_from(channels[ci]["sampler"].as_u64().unwrap()).unwrap()];
            let acc = |key: &str| usize::try_from(sampler[key].as_u64().unwrap()).unwrap();
            let (element, times) = g.floats(acc("input")).unwrap();
            assert_eq!(element, ElementType::Scalar, "{at}");
            assert_eq!(
                times.len(),
                usize::try_from(ec["inputCount"].as_u64().unwrap()).unwrap(),
                "{at}"
            );
            assert_eq!(
                f64::from(times[0]),
                ec["timeMin"].as_f64().unwrap(),
                "{at}: first time"
            );
            assert_eq!(
                f64::from(*times.last().unwrap()),
                ec["timeMax"].as_f64().unwrap(),
                "{at}: last time"
            );
            let (element, out) = g.floats(acc("output")).unwrap();
            assert_eq!(
                out.len() / element.components(),
                usize::try_from(ec["outputCount"].as_u64().unwrap()).unwrap(),
                "{at}"
            );
            assert_eq!(out.len() % element.components(), 0, "{at}");
            let kind = match element {
                ElementType::Scalar => "SCALAR",
                ElementType::Vec2 => "VEC2",
                ElementType::Vec3 => "VEC3",
                ElementType::Vec4 => "VEC4",
                ElementType::Mat4 => "MAT4",
            };
            assert_eq!(kind, ec["outputType"].as_str().unwrap(), "{at}");
            seen.channels += 1;
        }
    }
    eprintln!("{name}: {seen:?}");
    seen
}

fn root() -> Option<PathBuf> {
    primase_testdata::real_data("PRIMASE_KHRONOS_GLTF")
}

macro_rules! model {
    ($test:ident, $name:literal, |$s:ident| $check:expr) => {
        #[test]
        fn $test() {
            let Some(root) = root() else { return };
            let $s = check_model(&root, $name);
            assert!($s.meshes > 0);
            $check;
        }
    };
}

model!(simple_sparse_accessor, "SimpleSparseAccessor", |s| assert_eq!(
    s.sparse_accessors,
    1
));
model!(simple_skin, "SimpleSkin", |s| assert!(
    s.skin_primitives == 1 && s.matrices == 2 && s.channels > 0
));
model!(cesium_man, "CesiumMan", |s| assert!(
    s.skin_primitives == 1 && s.matrices == 19 && s.channels > 0
));
model!(rigged_simple, "RiggedSimple", |s| assert!(
    s.skin_primitives == 1 && s.matrices == 2
));
model!(rigged_figure, "RiggedFigure", |s| assert!(
    s.skin_primitives == 1 && s.matrices == 19
));
model!(recursive_skeletons, "RecursiveSkeletons", |s| assert!(
    s.skins == 84 && s.matrices > 0
));
model!(animated_morph_cube, "AnimatedMorphCube", |s| assert!(
    s.target_sets == 2 && s.default_weights == 1 && s.channels == 1
));
model!(morph_primitives_test, "MorphPrimitivesTest", |s| assert!(
    s.target_sets > 0 && s.default_weights == 1
));
model!(simple_morph, "SimpleMorph", |s| assert!(
    s.target_sets == 2 && s.channels == 1
));
model!(interpolation_test, "InterpolationTest", |s| assert_eq!(
    s.channels,
    9
));
