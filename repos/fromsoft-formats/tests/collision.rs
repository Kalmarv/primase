//! Compare raw collision soups with an independent decoder's (`havok-soup`, one per `havok` tagfile). No smoothing,
//! surface selection, or frame lowering is part of this raw-file comparison.
//!
//! Triangle count and order are compared exactly, and so is every material this reader determines. Where a mesh holds
//! triangle-data rows no key takes, the oracle places them by vertex-id agreement, which does not determine their place
//! (a row of new ids agrees with any triangle); this reader gives a key whose candidate rows name different materials
//! none. Such a triangle must be -1 here and a material in the oracle, and their number must be the soup's count of
//! placement-dependent keys. A position is compared within a bound, because the two
//! decoders do the dequantization's arithmetic differently: this one in f32 as the stored values are (a shared vertex's
//! `min + q * (max - min) / (2^21 - 1)`, three roundings over the domain's extent and one over the result; a packed vertex's
//! `q * scale + offset`, two), the oracle in f64 rounded once to f32. Neither is shown to be the engine's arithmetic. Over a
//! domain whose largest coordinate magnitude is `m`, that is at most `4 * EPSILON * m` apart, and the body's pose (composed
//! in f64, applied in f32) adds at most as much again: [`BOUND`] `* EPSILON * m`. The oracle's positions lie within the domain
//! padded by its 0.01 margin, so `m` is taken as the file's largest oracle coordinate magnitude plus [`PAD`]. A decoding
//! difference (a wrong bit width, domain, codec or index) moves a position by a quantization step or more, at least
//! `m / 2^22`, which is about 30 times the bound.
use fromsoft_formats::collision;
use havok_formats::Compendium;

/// The position bound in units of `EPSILON * m` (the module comment derives it).
const BOUND: f32 = 8.;
/// The domain's margin beyond its mesh, at least the 0.01 the files store.
const PAD: f32 = 0.02;

#[test]
fn castle_raw_soups() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = primase_testdata::oracle(
        primase_testdata::OracleSet::Corpus,
        primase_testdata::Game::Sekiro,
        "havok",
    ) else {
        return Ok(());
    };
    let Some(soups) = primase_testdata::oracle(
        primase_testdata::OracleSet::Corpus,
        primase_testdata::Game::Sekiro,
        "havok-soup",
    ) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|p| p.map(|p| p.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let comp = files
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "compendium"))
        .ok_or("missing compendium")?;
    let comp = Compendium::read(&std::fs::read(comp)?)?;
    let mut invalid_keys = 0;
    let mut count = 0;
    let mut triangles = 0;
    let mut unmapped = 0;
    let mut placement_dependent = 0;
    let (mut positions, mut inexact, mut widest) = (0, 0, 0f32);
    for path in files.iter().filter(|p| p.extension().is_some_and(|e| e == "hkx")) {
        let soup = collision::read_map(&std::fs::read(path)?, Some(&comp))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path.file_name().ok_or("tagfile without a name")?;
        let oracle = std::fs::read(soups.join(name).with_extension("hkx.soup"))?;
        let n = u32::from_le_bytes(oracle[..4].try_into()?) as usize;
        assert_eq!(n, soup.triangles.len(), "{}", path.display());
        assert_eq!(oracle.len(), 4 + n * 40);
        let expected: Vec<f32> = oracle[4..4 + n * 36]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect();
        let m = expected.iter().fold(0f32, |m, e| m.max(e.abs())) + PAD;
        let unit = f32::EPSILON * m;
        for (i, (&actual, &expected)) in soup
            .triangles
            .iter()
            .flatten()
            .flatten()
            .zip(&expected)
            .enumerate()
        {
            let apart = (actual - expected).abs() / unit;
            assert!(
                apart <= BOUND,
                "{} position {i}: {actual} against {expected}, {apart} * EPSILON * {m} apart",
                path.display()
            );
            inexact += usize::from(actual != expected);
            widest = widest.max(apart);
        }
        positions += expected.len();
        let mut undetermined = 0;
        for (i, &actual) in soup.materials.iter().enumerate() {
            let start = 4 + n * 36 + i * 4;
            let expected = i32::from_le_bytes(oracle[start..start + 4].try_into()?);
            if actual == -1 && expected != -1 {
                undetermined += 1;
            } else {
                assert_eq!(actual, expected, "{} material {i}", path.display());
            }
        }
        assert_eq!(undetermined, soup.placement_dependent_keys, "{}", path.display());
        placement_dependent += undetermined;
        invalid_keys += soup.invalid_material_keys;
        count += 1;
        triangles += n;
        unmapped += soup.unmapped;
    }
    assert!(count > 0);
    eprintln!(
        "collision files {count}, triangles {triangles}, unmapped {unmapped} ({placement_dependent} of them placement-dependent), invalid material keys {invalid_keys}: counts, order and determined materials exact; positions {positions}, {inexact} not exact, widest {widest} * EPSILON * m (bound {BOUND})"
    );
    Ok(())
}
