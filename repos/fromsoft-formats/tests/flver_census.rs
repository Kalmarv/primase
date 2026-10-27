//! Whole-install FLVER attributes and strip ordering against an independent decoder, for each game configured:
//! SoulsFormats' census of Sekiro (`oracles.soulsformats.sekiro/census`), SoulsFormatsNEXT's of Elden Ring
//! (`oracles.soulsformats.elden/census`). A FLVER the reference refuses (a record with `error`) must be refused by ours
//! too, except a SpeedTree FLVER: the reference refuses every one of Elden Ring's (an index out of range), ours
//! reads their stored members; they are counted.
mod inputs;

use fromsoft_formats::{Binder, Game, NameDictionary, bhd::PathHash, dcx::Decoder, flver::Model};

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;
#[allow(unsafe_code)]
mod native {
    pub fn load(
        path: crate::inputs::OodleLibrary,
    ) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe {
            primase_foundations::compress::oodle::Oodle::open(
                path.path(),
                primase_foundations::compress::oodle::Version::V6,
            )
        }
    }
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}
#[derive(Default)]
struct Counts {
    models: usize,
    meshes: usize,
    normal_components: u64,
    normal_changed: u64,
    normal_max_ulp: u32,
    agreed_rejections: usize,
    /// SpeedTree FLVERs the reference refuses and ours reads.
    speedtree_read: usize,
}
/// Every mesh whose vertex hash differs from the reference's: collected so one run lists them all. The census visits
/// models on several threads, so these are process-wide.
static VERTEX_MISMATCHES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// The reference decoder in use is SoulsFormatsNEXT's (Elden Ring) rather than SoulsFormats' (Sekiro).
/// Sekiro meshes whose only difference from the reference is the `0x2e` reading.
static SEKIRO_HALF4_DIVERGENCES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HALF4_LANES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HALF4_IN_UNIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HALF4_MAX_ABS_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static REFERENCE_NEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A vertex's UVs in the reference's packing. The reader keeps each UV member as the one four-lane attribute the game's
/// vertex program receives (`Vertex::uvs`). SoulsFormatsNEXT splits two kinds of member into two UVs of two lanes: a
/// `UByte4Norm` (`0x13`) member, four unorm bytes `(b0, b1, 0)` and `(b2, b3, 0)`, and a kind `0x2e` member, whose four
/// 16-bit words it reads as int16 over the UV factor, likewise two UVs. The reader's values are what the game's programs
/// receive (the `0x13` member as unorm bytes, the `0x2e` member as half floats, both a float `TEXCOORDn xyzw` used
/// unscaled; the `0x2e` member is a pivot point in the model's space, not a texture coordinate; see `Vertex::uvs`), so
/// this projects them back onto the reference's packing of the same stored bytes:
/// the unorm lanes as they are, the half lanes through their 16-bit words. SoulsFormats' census of Sekiro has one UV
/// per member.
fn reference_uvs(
    v: &fromsoft_formats::flver::Vertex,
    kinds: &[u32],
    next: bool,
    sekiro_words: bool,
) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for (i, uv) in v.uvs.iter().enumerate() {
        if next && kinds.get(i) == Some(&0x13) {
            out.push([uv[0], uv[1], 0.]);
            out.push([uv[2], uv[3], 0.]);
        } else if next && kinds.get(i) == Some(&0x2e) {
            let word = |x: f32| f32::from(half_bits(x) as i16) / 2048.;
            out.push([word(uv[0]), word(uv[1]), 0.]);
            out.push([word(uv[2]), word(uv[3]), 0.]);
        } else if !next && kinds.get(i) == Some(&0x2e) {
            // SoulsFormats' Short4toFloat4B: three int16 over the UV factor, then an int16 it asserts is zero
            assert_eq!(half_bits(uv[3]), 0, "reference asserts the fourth word is zero");
            let word = |x: f32| {
                if sekiro_words {
                    f32::from(half_bits(x) as i16) / 2048.
                } else {
                    x
                }
            };
            out.push([word(uv[0]), word(uv[1]), word(uv[2])]);
        } else if !next && kinds.get(i) == Some(&0x13) {
            // SoulsFormats reads the four bytes as an int16 pair over the UV factor
            let raw = v
                .layer_bytes
                .iter()
                .find(|(at, _)| *at == i)
                .map(|(_, r)| *r)
                .unwrap_or_default();
            out.push([
                f32::from(i16::from_le_bytes([raw[0], raw[1]])) / 2048.,
                f32::from(i16::from_le_bytes([raw[2], raw[3]])) / 2048.,
                0.,
            ]);
        } else {
            out.push([uv[0], uv[1], uv[2]]);
        }
    }
    out
}
/// The 16 bits of the half float `x` (a value the reader decoded from a half, so exactly representable).
fn half_bits(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let magnitude = bits & 0x7fff_ffff;
    if magnitude == 0 {
        return sign;
    }
    let exponent = (magnitude >> 23) as i32 - 127;
    if exponent < -14 {
        // a subnormal half: the mantissa over 2^-24
        return sign | (f32::from_bits(magnitude) * 2f32.powi(24)).round() as u16;
    }
    sign | ((((exponent + 15) as u32) << 10) | ((magnitude & 0x7f_ffff) >> 13)) as u16
}
/// The kind of the layout member each of a mesh's UVs came from, in the reader's push order.
fn uv_member_kinds(model: &Model, layouts: &[usize]) -> Vec<u32> {
    let mut kinds = Vec::new();
    for &l in layouts {
        for member in &model.layouts[l] {
            if member.semantic == 5 {
                let n = if matches!(member.kind, 3 | 0x16) { 2 } else { 1 };
                kinds.extend(std::iter::repeat_n(member.kind, n));
            }
        }
    }
    kinds
}
/// The independent decoder divides by 127; the game multiplies by its rounded
/// reciprocal. Recover the unique encoded integer only after checking the
/// consumer result bit-for-bit. Project to the reference's value convention for
/// its digest, and separately count every one-ULP deviation. This never changes
/// production decoding or accepts a different packed attribute.
fn reference_normal(value: f32, counts: &mut Counts) -> Result<f32> {
    let integer = (value * 127.).round();
    if !(-127. ..=128.).contains(&integer)
        || (integer * f32::from_bits(0x3c01_0204)).to_bits() != value.to_bits()
    {
        return Err("normal is not a consumer-decoded packed byte".into());
    }
    let reference = integer / 127.;
    let ulp = value.to_bits().abs_diff(reference.to_bits());
    if ulp > 1 {
        return Err("packed normal differs from reference by more than one ULP".into());
    }
    counts.normal_components += 1;
    counts.normal_changed += u64::from(ulp != 0);
    counts.normal_max_ulp = counts.normal_max_ulp.max(ulp);
    Ok(reference)
}
fn visit(
    bytes: &[u8],
    id: &str,
    decoder: &Decoder<'_>,
    expected: &mut BTreeMap<String, serde_json::Value>,
    counts: &mut Counts,
    depth: usize,
) -> Result<()> {
    if depth > 8 {
        return Err("nested binder limit".into());
    }
    if bytes.starts_with(b"FLVER\0") {
        let want = expected
            .remove(id)
            .ok_or_else(|| format!("unexpected FLVER {id}"))?;
        assert_eq!(sha(bytes), want["sha256"].as_str().ok_or("sha")?, "{id}");
        if let Some(theirs) = want.get("error") {
            return match Model::read(bytes) {
                Ok(m) if m.speedtree => {
                    counts.speedtree_read += 1;
                    Ok(())
                }
                Ok(_) => Err(format!("{id}: read, the reference refuses it: {theirs}").into()),
                Err(e) => {
                    counts.agreed_rejections += 1;
                    println!("Agreed FLVER rejection {id}: ours {e}; reference {theirs}");
                    Ok(())
                }
            };
        }
        let model = Model::read(bytes)?;
        assert_eq!(
            u64::from(model.version),
            want["version"].as_u64().ok_or("version")?
        );
        let meshes = want["meshes"].as_array().ok_or("meshes")?;
        assert_eq!(model.meshes.len(), meshes.len(), "{id}");
        for (mesh_index, (m, w)) in model.meshes.iter().zip(meshes).enumerate() {
            let uv_kinds = uv_member_kinds(&model, &m.layouts);
            for &layout in &m.layouts {
                for member in &model.layouts[layout] {
                    if matches!(member.semantic, 3 | 6) && member.kind != 0x11 {
                        return Err("reference normal projection only covers uint4 attributes".into());
                    }
                }
            }
            assert_eq!(m.material as u64, w["material"].as_u64().ok_or("material")?);
            assert_eq!(m.vertices.len() as u64, w["vertices"].as_u64().ok_or("vertices")?);
            let build = |sekiro_words: bool, counts: &mut Counts| -> Result<Vec<u8>> {
                let mut stream = Vec::new();
                for v in &m.vertices {
                    for x in v.position {
                        stream.extend(x.to_le_bytes());
                    }
                    for x in v.normal {
                        stream.extend(reference_normal(x, counts)?.to_le_bytes());
                    }
                    stream.extend(v.normal_w.to_le_bytes());
                    let uvs = reference_uvs(
                        v,
                        &uv_kinds,
                        REFERENCE_NEXT.load(std::sync::atomic::Ordering::Relaxed),
                        sekiro_words,
                    );
                    stream.extend((uvs.len() as u32).to_le_bytes());
                    for x in uvs.iter().flatten() {
                        stream.extend(x.to_le_bytes());
                    }
                    stream.extend((v.tangents.len() as u32).to_le_bytes());
                    for x in v.tangents.iter().flatten() {
                        stream.extend(reference_normal(*x, counts)?.to_le_bytes());
                    }
                    stream.extend((v.colors.len() as u32).to_le_bytes());
                    for x in v.colors.iter().flatten() {
                        stream.extend(x.to_le_bytes());
                    }
                }
                Ok(stream)
            };
            let want_hash = w["vertex_hash"].as_str().ok_or("vertex hash")?;
            let stored = sha(&build(false, counts)?);
            if stored != want_hash {
                // SoulsFormats reads a Sekiro `0x2e` member as int16 words over the UV factor; ours reads halves (see
                // the module notes). A mesh counts as that named divergence only when reading its `0x2e` members the
                // reference's way reproduces the reference's hash exactly; any other difference is a mismatch.
                let sekiro_half4 =
                    !REFERENCE_NEXT.load(std::sync::atomic::Ordering::Relaxed) && uv_kinds.contains(&0x2e);
                if sekiro_half4 && sha(&build(true, &mut Counts::default())?) == want_hash {
                    SEKIRO_HALF4_DIVERGENCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    for v in &m.vertices {
                        for (i, uv) in v.uvs.iter().enumerate() {
                            if uv_kinds.get(i) == Some(&0x2e) {
                                for x in &uv[..3] {
                                    HALF4_LANES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    HALF4_IN_UNIT.fetch_add(
                                        u64::from((0. ..=1.).contains(x)),
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    HALF4_MAX_ABS_BITS
                                        .fetch_max(x.abs().to_bits(), std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                        }
                    }
                } else {
                    VERTEX_MISMATCHES
                        .lock()
                        .expect("mismatch list lock")
                        .push(format!("{id} mesh {mesh_index} uv member kinds {uv_kinds:x?}"));
                }
            }
            let faces = w["faces"].as_array().ok_or("faces")?;
            assert_eq!(m.faces.len(), faces.len());
            for (f, expected) in m.faces.iter().zip(faces) {
                assert_eq!(u64::from(f.flags), expected["flags"].as_u64().ok_or("flags")?);
                assert_eq!(f.strip, expected["strip"].as_bool().ok_or("strip")?);
                assert_eq!(f.cull, expected["cull"].as_bool().ok_or("cull")?);
                assert_eq!(
                    sha(&f.indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>()),
                    expected["hash"].as_str().ok_or("index hash")?,
                    "{id}"
                );
                assert_eq!(
                    sha(&f
                        .triangulate()
                        .iter()
                        .flat_map(|i| i.to_le_bytes())
                        .collect::<Vec<_>>()),
                    expected["triangulated"].as_str().ok_or("triangle hash")?,
                    "{id}"
                );
            }
            counts.meshes += 1;
        }
        counts.models += 1;
    } else if bytes.starts_with(b"BND4") {
        let b = Binder::read(bytes, decoder)?;
        for (i, f) in b.files.iter().enumerate() {
            visit(
                &f.bytes,
                &format!("{id}/b{i}"),
                decoder,
                expected,
                counts,
                depth + 1,
            )?;
        }
    }
    Ok(())
}
#[test]
fn all_flver_models() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, oracle, names) in [
        (
            Game::Sekiro,
            inputs::oracle(inputs::OracleSet::SoulsFormats, inputs::Game::Sekiro, "census"),
            inputs::oracle(inputs::OracleSet::Uxm, inputs::Game::Sekiro, "dictionary.txt"),
        ),
        (
            Game::EldenRing,
            inputs::oracle(inputs::OracleSet::SoulsFormats, inputs::Game::Elden, "census"),
            inputs::oracle(inputs::OracleSet::Uxm, inputs::Game::Elden, "dictionary.txt"),
        ),
    ] {
        let (Some(archive), Some(oracle), Some(names)) = (common::archive(game)?, oracle, names) else {
            continue;
        };
        flver_models(
            game,
            &archive,
            &oracle,
            &NameDictionary::read_for(names, game)?,
            &decoder,
        )?;
    }
    Ok(())
}

fn flver_models(
    game: Game,
    archive: &fromsoft_formats::Archive,
    oracle: &std::path::Path,
    names: &NameDictionary,
    decoder: &Decoder<'_>,
) -> Result<()> {
    let decoder = *decoder;
    REFERENCE_NEXT.store(game == Game::EldenRing, std::sync::atomic::Ordering::Relaxed);
    // the reference writes a hash at its game's width
    let width = match game.archives().hash {
        PathHash::Base37 => 8,
        PathHash::Base133 => 16,
    };
    let mut expected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("flver/models.jsonl"))?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        expected.insert(value["id"].as_str().ok_or("id")?.to_owned(), value);
    }
    // shard, hash, SoulsFormats' message: the entries the reference cannot decode
    let mut rejected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("errors.tsv"))?.lines() {
        let f: Vec<_> = line.splitn(3, '\t').collect();
        let [shard, hash, message] = f[..] else {
            return Err(format!("errors.tsv row: {line}").into());
        };
        rejected.insert(format!("{shard}/{hash}"), message.to_owned());
    }
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    let mut split = 0;
    for (&hash, entry) in &archive.entries {
        let name = names.names.get(&hash).and_then(|x| x.first());
        if name.is_some_and(|n| n.ends_with("bdt")) {
            continue;
        }
        let id = format!("{}/{hash:0width$x}", entry.shard);
        let result = (|| -> Result<()> {
            let bytes = archive.read_hash(hash, &decoder)?;
            if bytes.starts_with(b"BHF4") {
                let name = name.ok_or("unnamed split binder")?;
                let payload = archive.read(&format!("{}bdt", &name[..name.len() - 3]), &decoder)?;
                let binder = Binder::read_split(&bytes, &payload, &decoder)?;
                for (i, f) in binder.files.iter().enumerate() {
                    visit(
                        &f.bytes,
                        &format!("{id}/s{i}"),
                        &decoder,
                        &mut expected,
                        &mut counts,
                        0,
                    )?;
                }
                split += 1;
            } else {
                visit(&bytes, &id, &decoder, &mut expected, &mut counts, 0)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            // both decoders reject it: agreement on the entry, the reasons reported side by side
            if let Some(theirs) = rejected.remove(&id) {
                println!("Agreed rejection {id}: ours {e}; reference {theirs}");
            } else {
                failures.push(format!("{id}: {e}"));
            }
        }
    }
    println!(
        "{}: FLVER models {}, agreed rejections {}, SpeedTree models the reference refuses read {}, meshes {}, split \
         binders {}, remaining oracle {}, failures {}",
        game.name(),
        counts.models,
        counts.agreed_rejections,
        counts.speedtree_read,
        counts.meshes,
        split,
        expected.len(),
        failures.len()
    );
    println!(
        "Packed normal/tangent components {}, reference-convention differences {}, maximum ULP {}",
        counts.normal_components, counts.normal_changed, counts.normal_max_ulp
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    println!(
        "Sekiro 0x2e (SoulsFormats int16 words) divergences: {} meshes; x/y/z lanes read as halves {}, in [0, 1] {}, largest magnitude {}",
        SEKIRO_HALF4_DIVERGENCES.load(std::sync::atomic::Ordering::Relaxed),
        HALF4_LANES.load(std::sync::atomic::Ordering::Relaxed),
        HALF4_IN_UNIT.load(std::sync::atomic::Ordering::Relaxed),
        f32::from_bits(HALF4_MAX_ABS_BITS.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let mismatches = std::mem::take(&mut *VERTEX_MISMATCHES.lock().expect("mismatch list lock"));
    println!(
        "meshes whose vertex hash differs from the reference: {}",
        mismatches.len()
    );
    for m in mismatches.iter().take(40) {
        println!("VERTEX MISMATCH {m}");
    }
    assert!(
        mismatches.is_empty(),
        "{} meshes differ from the reference's vertex hash",
        mismatches.len()
    );
    assert!(failures.is_empty());
    assert!(expected.is_empty());
    assert!(rejected.is_empty(), "reference rejection not reproduced");
    Ok(())
}
